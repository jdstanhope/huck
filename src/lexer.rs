#[derive(Debug, PartialEq, Eq)]
pub enum LexError {
    UnterminatedQuote,
    InvalidVarName,
    UnterminatedBrace,
    UnterminatedSubstitution,
    UnterminatedArith,
    ArithParse(String),
    InvalidBraceModifier(String),
    EmptyParamName,
    InvalidBraceOperand,
    Substitution(Box<LexError>),
    SubstitutionParseError(crate::command::ParseError),
    UnterminatedHeredoc,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Operator {
    Pipe,           // |
    RedirOut,       // >
    RedirAppend,    // >>
    RedirIn,        // <
    RedirErr,       // 2>
    RedirErrAppend, // 2>>
    And,            // &&
    Or,             // ||
    Semi,           // ;
    Background,     // &
    LParen,         // (
    RParen,         // )
    DoubleSemi,     // ;;
    SemiAmp,        // ;&
    DoubleSemiAmp,  // ;;&
    HereString,     // <<<
    DupOut,         // >&
    DupErr,         // 2>&
    AndRedirOut,    // &>
    AndRedirAppend, // &>>
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum TildeSpec {
    Home,
    User(String),
    Pwd,
    OldPwd,
}

#[derive(Debug, Clone, PartialEq, Eq, Copy)]
pub enum SubstAnchor {
    None,    // ${var/pat/repl} and ${var//pat/repl}
    Prefix,  // ${var/#pat/repl}
    Suffix,  // ${var/%pat/repl}
}

#[allow(dead_code)]  // removed when lexer emits Case in Task 2
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseDirection {
    Upper,  // ^ / ^^
    Lower,  // , / ,,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParamModifier {
    Length,
    UseDefault    { word: Word, colon: bool },
    AssignDefault { word: Word, colon: bool },
    ErrorIfUnset  { word: Word, colon: bool },
    UseAlternate  { word: Word, colon: bool },
    RemovePrefix  { pattern: Word, longest: bool },
    RemoveSuffix  { pattern: Word, longest: bool },
    Substitute {
        pattern: Word,
        replacement: Word,
        anchor: SubstAnchor,
        all: bool,
    },
    Substring {
        offset: Word,
        length: Option<Word>,
    },
    #[allow(dead_code)]  // constructed by lexer in Task 2; read in Task 4
    Case {
        direction: CaseDirection,
        all: bool,
        pattern: Option<Word>,
    },
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum WordPart {
    Literal { text: String, quoted: bool },
    Tilde(TildeSpec),
    Var { name: String, quoted: bool },
    LastStatus { quoted: bool },
    CommandSub { sequence: crate::command::Sequence, quoted: bool },
    Arith { expr: crate::arith::ArithExpr, quoted: bool },
    ParamExpansion { name: String, modifier: ParamModifier, quoted: bool },
    /// `$@` (joined=false) or `$*` (joined=true). `quoted` reflects whether
    /// this was inside double quotes.
    AllArgs { quoted: bool, joined: bool },
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Word(pub Vec<WordPart>);

#[derive(Debug, PartialEq, Eq)]
pub enum Token {
    Word(Word),
    Op(Operator),
    Newline,
    /// A complete here-doc with its body already collected. The lexer
    /// builds this in two phases: the `<<DELIM` opener is seen on one
    /// line, the body lines are consumed after the line's `\n`. The
    /// resulting Token::Heredoc occupies the position where `<<DELIM`
    /// appeared (the delim word itself is not emitted).
    Heredoc { body: Word, expand: bool, strip_tabs: bool },
}

/// State for a heredoc whose body hasn't been collected yet.
struct PendingHeredoc {
    delim: String,
    expand: bool,
    strip_tabs: bool,
    /// Index into `tokens` of the `Token::Heredoc` placeholder to patch.
    token_idx: usize,
}

pub fn tokenize(input: &str) -> Result<Vec<Token>, LexError> {
    let mut tokens: Vec<Token> = Vec::new();
    let mut parts: Vec<WordPart> = Vec::new();
    let mut current = String::new();
    let mut quoted_current = String::new();
    let mut has_token = false;
    let mut in_assignment_value = false;
    let mut chars = input.chars().peekable();
    let mut pending_heredocs: std::collections::VecDeque<PendingHeredoc> = std::collections::VecDeque::new();

    while let Some(c) = chars.next() {
        if c.is_whitespace() {
            if has_token {
                flush_literal(&mut parts, &mut current, false);
                debug_assert!(
                    !parts.is_empty(),
                    "lexer invariant: has_token was true but no parts were emitted"
                );
                tokens.push(Token::Word(Word(std::mem::take(&mut parts))));
                has_token = false;
                in_assignment_value = false;
            }
            if c == '\n' {
                // If there are pending heredocs, collect their bodies now
                // before emitting the Newline token.
                if !pending_heredocs.is_empty() {
                    collect_heredoc_bodies(&mut chars, &mut pending_heredocs, &mut tokens)?;
                }
                tokens.push(Token::Newline);
            }
            continue;
        }

        match c {
            '\'' => {
                has_token = true;
                flush_literal(&mut parts, &mut current, false);
                let parts_len_before = parts.len();
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(ch) => quoted_current.push(ch),
                        None => return Err(LexError::UnterminatedQuote),
                    }
                }
                flush_literal(&mut parts, &mut quoted_current, true);
                if parts.len() == parts_len_before {
                    // Empty `''` — preserve the empty-token contract by
                    // emitting an empty quoted Literal.
                    parts.push(WordPart::Literal { text: String::new(), quoted: true });
                }
            }
            '"' => {
                has_token = true;
                flush_literal(&mut parts, &mut current, false);
                let parts_len_before = parts.len();
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            // POSIX: inside `"..."`, backslash is special only
                            // before `$`, `, `"`, `\`, and newline. For other
                            // characters, the backslash is retained literally.
                            Some(esc @ ('"' | '\\' | '$' | '`')) => quoted_current.push(esc),
                            // POSIX 2.2.3: `\<NL>` inside double quotes is also
                            // line continuation — both characters deleted.
                            Some('\n') => {}
                            Some(other) => {
                                quoted_current.push('\\');
                                quoted_current.push(other);
                            }
                            None => return Err(LexError::UnterminatedQuote),
                        },
                        Some('$') => {
                            // Expansion inside double quotes (quoted: true).
                            flush_literal(&mut parts, &mut quoted_current, true);
                            read_dollar_expansion(&mut chars, &mut parts, true)?;
                        }
                        Some('`') => {
                            // Backtick substitution inside double quotes (quoted: true).
                            flush_literal(&mut parts, &mut quoted_current, true);
                            let sequence = scan_backtick_substitution(&mut chars)?;
                            parts.push(WordPart::CommandSub { sequence, quoted: true });
                        }
                        Some(ch) => quoted_current.push(ch),
                        None => return Err(LexError::UnterminatedQuote),
                    }
                }
                flush_literal(&mut parts, &mut quoted_current, true);
                if parts.len() == parts_len_before {
                    // Empty `""` — preserve the empty-token contract by
                    // emitting an empty quoted Literal.
                    parts.push(WordPart::Literal { text: String::new(), quoted: true });
                }
            }
            '\\' => match chars.next() {
                Some('\n') => {
                    // POSIX 2.2.1: `\<NL>` is line continuation — both chars
                    // are deleted. `has_token` stays at its current value, so
                    // `echo\<NL>foo` becomes the single word "echofoo" while
                    // `echo \<NL>foo` keeps the space-driven separation.
                }
                Some(ch) => {
                    // Flush any accumulated unquoted text, then push the
                    // escaped char as a one-char quoted Literal. This is
                    // what makes `\*` survive pathname expansion as a
                    // literal `*` (the `quoted` flag inhibits globbing).
                    has_token = true;
                    flush_literal(&mut parts, &mut current, false);
                    parts.push(WordPart::Literal { text: ch.to_string(), quoted: true });
                }
                None => {
                    has_token = true;
                    current.push('\\');
                }
            },
            '$' => {
                // Expansion outside any quotes (quoted: false).
                has_token = true;
                flush_literal(&mut parts, &mut current, false);
                read_dollar_expansion(&mut chars, &mut parts, false)?;
            }
            '#' if !has_token => {
                // POSIX: an unquoted `#` that begins a word starts a comment
                // to end-of-line. `#` mid-word (has_token=true) falls through
                // to the catch-all as a literal char.
                while let Some(&ch) = chars.peek() {
                    if ch == '\n' { break; }
                    chars.next();
                }
                // The trailing newline (if any) is handled by the outer loop.
            }
            '~' if !has_token || tilde_eligible_in_assignment(in_assignment_value, &current) => {
                if let Some(spec) = try_parse_tilde(&mut chars, in_assignment_value) {
                    flush_literal(&mut parts, &mut current, false);
                    has_token = true;
                    parts.push(WordPart::Tilde(spec));
                } else {
                    // Fall through: treat '~' as literal.
                    current.push('~');
                    has_token = true;
                }
            }
            '`' => {
                has_token = true;
                flush_literal(&mut parts, &mut current, false);
                let sequence = scan_backtick_substitution(&mut chars)?;
                parts.push(WordPart::CommandSub { sequence, quoted: false });
            }
            '|' => {
                if has_token {
                    flush_literal(&mut parts, &mut current, false);
                    tokens.push(Token::Word(Word(std::mem::take(&mut parts))));
                    has_token = false;
                }
                if chars.peek() == Some(&'|') {
                    chars.next();
                    tokens.push(Token::Op(Operator::Or));
                } else {
                    tokens.push(Token::Op(Operator::Pipe));
                }
                in_assignment_value = false;
            }
            '&' => {
                if has_token {
                    flush_literal(&mut parts, &mut current, false);
                    tokens.push(Token::Word(Word(std::mem::take(&mut parts))));
                    has_token = false;
                }
                if chars.peek() == Some(&'&') {
                    chars.next();
                    tokens.push(Token::Op(Operator::And));
                } else if chars.peek() == Some(&'>') {
                    chars.next();
                    if chars.peek() == Some(&'>') {
                        chars.next();
                        tokens.push(Token::Op(Operator::AndRedirAppend));
                    } else {
                        tokens.push(Token::Op(Operator::AndRedirOut));
                    }
                } else {
                    tokens.push(Token::Op(Operator::Background));
                }
                in_assignment_value = false;
            }
            ';' => {
                if has_token {
                    flush_literal(&mut parts, &mut current, false);
                    tokens.push(Token::Word(Word(std::mem::take(&mut parts))));
                    has_token = false;
                }
                let op = if chars.peek() == Some(&';') {
                    chars.next();
                    if chars.peek() == Some(&'&') {
                        chars.next();
                        Operator::DoubleSemiAmp
                    } else {
                        Operator::DoubleSemi
                    }
                } else if chars.peek() == Some(&'&') {
                    chars.next();
                    Operator::SemiAmp
                } else {
                    Operator::Semi
                };
                tokens.push(Token::Op(op));
                in_assignment_value = false;
            }
            '(' => {
                if has_token {
                    flush_literal(&mut parts, &mut current, false);
                    tokens.push(Token::Word(Word(std::mem::take(&mut parts))));
                    has_token = false;
                }
                tokens.push(Token::Op(Operator::LParen));
                in_assignment_value = false;
            }
            ')' => {
                if has_token {
                    flush_literal(&mut parts, &mut current, false);
                    tokens.push(Token::Word(Word(std::mem::take(&mut parts))));
                    has_token = false;
                }
                tokens.push(Token::Op(Operator::RParen));
                in_assignment_value = false;
            }
            '<' => {
                if has_token {
                    flush_literal(&mut parts, &mut current, false);
                    tokens.push(Token::Word(Word(std::mem::take(&mut parts))));
                    has_token = false;
                }
                if chars.peek() == Some(&'<') {
                    chars.next(); // consume second '<'
                    if chars.peek() == Some(&'<') {
                        chars.next(); // consume third '<' — here-string
                        tokens.push(Token::Op(Operator::HereString));
                    } else {
                        let strip_tabs = if chars.peek() == Some(&'-') {
                            chars.next(); // consume '-'
                            true
                        } else {
                            false
                        };
                        // Parse the delimiter word and detect literal vs expanding mode.
                        let (delim, expand) = parse_heredoc_delim(&mut chars)?;
                        // Push a placeholder Token::Heredoc with empty body.
                        // The body is back-patched after the line's \n.
                        let placeholder_idx = tokens.len();
                        tokens.push(Token::Heredoc {
                            body: Word(Vec::new()),
                            expand,
                            strip_tabs,
                        });
                        pending_heredocs.push_back(PendingHeredoc {
                            delim,
                            expand,
                            strip_tabs,
                            token_idx: placeholder_idx,
                        });
                    }
                } else {
                    tokens.push(Token::Op(Operator::RedirIn));
                }
                in_assignment_value = false;
            }
            '>' => {
                if has_token {
                    flush_literal(&mut parts, &mut current, false);
                    tokens.push(Token::Word(Word(std::mem::take(&mut parts))));
                    has_token = false;
                }
                if chars.peek() == Some(&'>') {
                    chars.next();
                    tokens.push(Token::Op(Operator::RedirAppend));
                } else if chars.peek() == Some(&'&') {
                    chars.next();
                    tokens.push(Token::Op(Operator::DupOut));
                } else {
                    tokens.push(Token::Op(Operator::RedirOut));
                }
                in_assignment_value = false;
            }
            '1' if !has_token && chars.peek() == Some(&'>') => {
                chars.next();
                if chars.peek() == Some(&'>') {
                    chars.next();
                    tokens.push(Token::Op(Operator::RedirAppend));
                } else if chars.peek() == Some(&'&') {
                    chars.next();
                    tokens.push(Token::Op(Operator::DupOut));
                } else {
                    tokens.push(Token::Op(Operator::RedirOut));
                }
                in_assignment_value = false;
            }
            '2' if !has_token && chars.peek() == Some(&'>') => {
                chars.next();
                if chars.peek() == Some(&'>') {
                    chars.next();
                    tokens.push(Token::Op(Operator::RedirErrAppend));
                } else if chars.peek() == Some(&'&') {
                    chars.next();
                    tokens.push(Token::Op(Operator::DupErr));
                } else {
                    tokens.push(Token::Op(Operator::RedirErr));
                }
                in_assignment_value = false;
            }
            '=' if !in_assignment_value && word_is_identifier_so_far(&current, &parts) => {
                in_assignment_value = true;
                has_token = true;
                current.push('=');
            }
            other => {
                has_token = true;
                current.push(other);
            }
        }
    }

    if has_token {
        flush_literal(&mut parts, &mut current, false);
        tokens.push(Token::Word(Word(parts)));
    }
    // If there are unresolved pending heredocs after end-of-input, it's an error.
    if !pending_heredocs.is_empty() {
        return Err(LexError::UnterminatedHeredoc);
    }
    Ok(tokens)
}

fn flush_literal(parts: &mut Vec<WordPart>, current: &mut String, quoted: bool) {
    if !current.is_empty() {
        parts.push(WordPart::Literal {
            text: std::mem::take(current),
            quoted,
        });
    }
}

/// Parses the heredoc delimiter word following `<<` or `<<-`.
/// Returns `(delim_text, expand)` where `expand` is false if any character
/// of the delimiter word was quoted (per POSIX 2.7.4: any quoting in the
/// delimiter word forces literal-mode body collection).
fn parse_heredoc_delim(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
) -> Result<(String, bool), LexError> {
    // Skip leading whitespace (POSIX: `<< EOF` is allowed).
    while matches!(chars.peek(), Some(&' ') | Some(&'\t')) {
        chars.next();
    }
    let mut delim = String::new();
    let mut any_quoted = false;
    while let Some(&c) = chars.peek() {
        match c {
            '\n' | ' ' | '\t' | ';' | '&' | '|' | '<' | '>' => break,
            '\'' => {
                chars.next();
                any_quoted = true;
                while let Some(&ch) = chars.peek() {
                    chars.next();
                    if ch == '\'' { break; }
                    delim.push(ch);
                }
            }
            '"' => {
                chars.next();
                any_quoted = true;
                while let Some(&ch) = chars.peek() {
                    chars.next();
                    if ch == '"' { break; }
                    if ch == '\\' && let Some(&next) = chars.peek() { chars.next(); delim.push(next); continue; }
                    delim.push(ch);
                }
            }
            '\\' => {
                chars.next();
                any_quoted = true;
                if let Some(&next) = chars.peek() {
                    chars.next();
                    delim.push(next);
                }
            }
            _ => {
                chars.next();
                delim.push(c);
            }
        }
    }
    if delim.is_empty() {
        return Err(LexError::UnterminatedHeredoc);
    }
    Ok((delim, !any_quoted))
}

/// Collects bodies for all pending heredocs in queue order.
/// After each heredoc's body is collected, it is patched back into the
/// placeholder `Token::Heredoc` at `token_idx`.
fn collect_heredoc_bodies(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    pending: &mut std::collections::VecDeque<PendingHeredoc>,
    tokens: &mut [Token],
) -> Result<(), LexError> {
    while let Some(ph) = pending.pop_front() {
        let body = collect_one_heredoc_body(chars, &ph)?;
        if let Some(Token::Heredoc { body: slot, expand, strip_tabs }) = tokens.get_mut(ph.token_idx) {
            *slot = body;
            *expand = ph.expand;
            *strip_tabs = ph.strip_tabs;
        } else {
            unreachable!("placeholder token at index was not Token::Heredoc");
        }
    }
    Ok(())
}

/// True when `s` ends with an odd-length run of backslashes — the final
/// backslash is unescaped and acts as a line-continuation marker.
pub(crate) fn ends_with_continuation_backslash(s: &str) -> bool {
    s.chars().rev().take_while(|&c| c == '\\').count() % 2 == 1
}

/// Collects the body of one heredoc, reading lines until the close-delimiter
/// is matched (or end-of-input, which is an error).
fn collect_one_heredoc_body(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    ph: &PendingHeredoc,
) -> Result<Word, LexError> {
    let mut body_parts: Vec<WordPart> = Vec::new();
    loop {
        // Read one full line until \n or end of input.
        let mut current_line = String::new();
        let mut got_newline = false;
        loop {
            match chars.next() {
                Some('\n') => {
                    got_newline = true;
                    break;
                }
                Some(c) => current_line.push(c),
                None => break,
            }
        }
        // POSIX 2.7.4: in expanding heredocs, `\<NL>` is a line continuation —
        // both the backslash and the newline are deleted, and the next line is
        // joined directly. Literal heredocs keep `\` + NL verbatim.
        while ph.expand
            && got_newline
            && ends_with_continuation_backslash(&current_line)
            && chars.peek().is_some()
        {
            // Strip the trailing backslash (the newline is already consumed).
            current_line.pop();
            // Read the next line into the same buffer (no separator).
            got_newline = false;
            loop {
                match chars.next() {
                    Some('\n') => {
                        got_newline = true;
                        break;
                    }
                    Some(c) => current_line.push(c),
                    None => break,
                }
            }
        }
        // For <<-, strip leading tabs from both body and close-delimiter lines.
        let line_for_check = if ph.strip_tabs {
            current_line.trim_start_matches('\t').to_string()
        } else {
            current_line.clone()
        };
        // Check if this is the close-delimiter line (must match exactly).
        if line_for_check == ph.delim {
            return Ok(Word(body_parts));
        }
        // Not the close — this is a body line.
        // EOF without a matching close-delimiter is an error.
        if !got_newline {
            return Err(LexError::UnterminatedHeredoc);
        }
        let body_line = if ph.strip_tabs {
            current_line.trim_start_matches('\t').to_string()
        } else {
            current_line
        };
        if ph.expand {
            scan_expanding_body_line(&body_line, &mut body_parts)?;
        } else {
            // Literal mode: entire line verbatim as a single quoted Literal.
            body_parts.push(WordPart::Literal {
                text: body_line,
                quoted: true,
            });
        }
        // Append the line's terminating newline (literal, quoted).
        body_parts.push(WordPart::Literal {
            text: "\n".to_string(),
            quoted: true,
        });
    }
}

/// Scans one body line of an expanding heredoc for `$`, `` ` ``, and `\`
/// per POSIX 2.7.4. Pushes `WordPart`s into `parts`.
fn scan_expanding_body_line(
    line: &str,
    parts: &mut Vec<WordPart>,
) -> Result<(), LexError> {
    let mut chars = line.chars().peekable();
    let mut current = String::new();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                // POSIX 2.7.4: inside expanding heredoc, `\` is special
                // only before `$`, `` ` ``, `\`. Other backslashes are literal.
                match chars.peek().copied() {
                    Some('$') | Some('`') | Some('\\') => {
                        let next = chars.next().unwrap();
                        // Flush current as unquoted, then push escaped char as quoted Literal.
                        flush_body_literal(parts, &mut current, false);
                        parts.push(WordPart::Literal { text: next.to_string(), quoted: true });
                    }
                    _ => current.push('\\'),
                }
            }
            '$' => {
                flush_body_literal(parts, &mut current, false);
                // Heredoc bodies are quoted-context (no word-splitting).
                read_dollar_expansion(&mut chars, parts, true)?;
            }
            '`' => {
                flush_body_literal(parts, &mut current, false);
                let sequence = scan_backtick_substitution(&mut chars)?;
                parts.push(WordPart::CommandSub { sequence, quoted: true });
            }
            other => current.push(other),
        }
    }
    flush_body_literal(parts, &mut current, false);
    Ok(())
}

fn flush_body_literal(parts: &mut Vec<WordPart>, current: &mut String, quoted: bool) {
    if !current.is_empty() {
        parts.push(WordPart::Literal {
            text: std::mem::take(current),
            quoted,
        });
    }
}

/// Reads what follows a `$`. Pushes the resulting WordPart onto `parts` or
/// (for an unrecognized form) pushes a literal `$` and lets the caller
/// continue tokenizing.
fn read_dollar_expansion(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    parts: &mut Vec<WordPart>,
    quoted: bool,
) -> Result<(), LexError> {
    match chars.peek().copied() {
        Some('(') => {
            chars.next(); // consume first '('
            if chars.peek() == Some(&'(') {
                chars.next(); // consume second '(' — this is `$((`
                let inner = scan_arith_body(chars)?;
                let expr = crate::arith::parse(&inner)
                    .map_err(|e| LexError::ArithParse(e.to_string()))?;
                parts.push(WordPart::Arith { expr, quoted });
            } else {
                let sequence = scan_paren_substitution(chars)?;
                parts.push(WordPart::CommandSub { sequence, quoted });
            }
        }
        Some('{') => {
            chars.next();
            read_braced_param_expansion(chars, parts, quoted)?;
        }
        Some('?') => {
            chars.next();
            parts.push(WordPart::LastStatus { quoted });
        }
        Some('@') => {
            chars.next();
            parts.push(WordPart::AllArgs { joined: false, quoted });
        }
        Some('*') => {
            chars.next();
            parts.push(WordPart::AllArgs { joined: true, quoted });
        }
        Some('#') => {
            chars.next();
            parts.push(WordPart::Var { name: "#".to_string(), quoted });
        }
        Some('$') => {
            chars.next();
            parts.push(WordPart::Var { name: "$".to_string(), quoted });
        }
        Some('!') => {
            chars.next();
            parts.push(WordPart::Var { name: "!".to_string(), quoted });
        }
        Some(c) if c.is_ascii_digit() => {
            let d = chars.next().unwrap();
            parts.push(WordPart::Var { name: d.to_string(), quoted });
        }
        Some(c) if is_name_start(c) => {
            let name = read_var_name(chars);
            parts.push(WordPart::Var { name, quoted });
        }
        _ => {
            parts.push(WordPart::Literal { text: "$".to_string(), quoted });
        }
    }
    Ok(())
}

/// Reads the inner text of a `$((...))` arithmetic expansion. The opening
/// `$((` has already been consumed; this function scans forward until the
/// matching `))` at depth 0. Returns the inner text (without the closing
/// `))`). Tracks paren depth so that nested `(` / `)` inside the
/// expression do not prematurely close the expansion.
fn scan_arith_body(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
) -> Result<String, LexError> {
    let mut body = String::new();
    let mut depth: u32 = 1; // we are inside the outer `((`
    loop {
        match chars.next() {
            None => return Err(LexError::UnterminatedArith),
            Some('(') => {
                depth += 1;
                body.push('(');
            }
            Some(')') => {
                if depth == 1 {
                    // The next char must be `)` to close `))`.
                    match chars.next() {
                        Some(')') => return Ok(body),
                        Some(_) | None => return Err(LexError::UnterminatedArith),
                    }
                } else {
                    depth -= 1;
                    body.push(')');
                }
            }
            Some(c) => body.push(c),
        }
    }
}

/// Reads the inner text of a `${...}` operand. The opening `{` has already
/// been consumed; this function consumes through the matching `}` at depth 0.
/// Tracks brace-depth, plus `'...'` and `"..."` so a stray `}` inside a
/// quoted span doesn't close the expansion. Returns the inner text (without
/// the closing `}`).
fn scan_braced_operand(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
) -> Result<String, LexError> {
    // Known limitation: a `${...}` nested *inside* a double-quoted span of
    // the operand (e.g. `${X:-"${Y}}"}`) is not depth-tracked — the inner
    // `}` chars are consumed literally by the quote loop. Real scripts very
    // rarely nest this way, and bash's own handling here is murky. Plain
    // nesting like `${X:-${Y}}` IS handled (depth tracking outside quotes).
    let mut body = String::new();
    let mut depth: u32 = 1;
    loop {
        match chars.next() {
            None => return Err(LexError::UnterminatedBrace),
            Some('\\') => {
                body.push('\\');
                if let Some(c) = chars.next() { body.push(c); }
            }
            Some('"') => {
                body.push('"');
                loop {
                    match chars.next() {
                        None => return Err(LexError::UnterminatedBrace),
                        Some('"') => { body.push('"'); break; }
                        Some('\\') => {
                            body.push('\\');
                            if let Some(c) = chars.next() { body.push(c); }
                        }
                        Some(c) => body.push(c),
                    }
                }
            }
            Some('\'') => {
                body.push('\'');
                loop {
                    match chars.next() {
                        None => return Err(LexError::UnterminatedBrace),
                        Some('\'') => { body.push('\''); break; }
                        Some(c) => body.push(c),
                    }
                }
            }
            Some('{') => { depth += 1; body.push('{'); }
            Some('}') => {
                if depth == 1 { return Ok(body); }
                depth -= 1;
                body.push('}');
            }
            Some(c) => body.push(c),
        }
    }
}

/// Tokenizes the operand body and merges the resulting words into a single
/// `Word`, inserting a literal space between adjacent words to preserve
/// IFS-split-relevant whitespace.
fn parse_braced_operand(body: &str) -> Result<Word, LexError> {
    let tokens = tokenize(body)
        .map_err(|e| LexError::Substitution(Box::new(e)))?;
    let mut parts: Vec<WordPart> = Vec::new();
    let mut first = true;
    for tok in tokens {
        match tok {
            Token::Word(Word(ps)) => {
                if !first {
                    parts.push(WordPart::Literal {
                        text: " ".to_string(),
                        quoted: false,
                    });
                }
                parts.extend(ps);
                first = false;
            }
            Token::Op(_) | Token::Newline | Token::Heredoc { .. } => return Err(LexError::InvalidBraceOperand),
        }
    }
    Ok(Word(parts))
}

/// Reads the body of a `$(...)` substitution. The opening `$(` is already
/// consumed; this function consumes through the matching `)` at depth 0.
/// Tracks quote and escape state so that `)` inside `'...'`, `"..."`, or
/// after `\` does not close the substitution, and nested `$(...)` increments
/// the depth.
fn scan_paren_substitution(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
) -> Result<crate::command::Sequence, LexError> {
    let mut body = String::new();
    let mut depth: usize = 0;
    while let Some(c) = chars.next() {
        match c {
            ')' if depth == 0 => {
                return parse_substitution_body(&body);
            }
            ')' => {
                depth -= 1;
                body.push(c);
            }
            '(' => {
                // Bare `(` is just a character. huck has no subshell
                // `(cmd)` syntax — only `$(` increments depth (handled in
                // the `$` arm below).
                body.push(c);
            }
            '\\' => {
                body.push(c);
                if let Some(next) = chars.next() {
                    body.push(next);
                } else {
                    return Err(LexError::UnterminatedSubstitution);
                }
            }
            '\'' => {
                body.push(c);
                loop {
                    match chars.next() {
                        Some('\'') => {
                            body.push('\'');
                            break;
                        }
                        Some(ch) => body.push(ch),
                        None => return Err(LexError::UnterminatedSubstitution),
                    }
                }
            }
            '"' => {
                body.push(c);
                loop {
                    match chars.next() {
                        Some('"') => {
                            body.push('"');
                            break;
                        }
                        Some('\\') => {
                            body.push('\\');
                            if let Some(next) = chars.next() {
                                body.push(next);
                            } else {
                                return Err(LexError::UnterminatedSubstitution);
                            }
                        }
                        Some(ch) => body.push(ch),
                        None => return Err(LexError::UnterminatedSubstitution),
                    }
                }
            }
            '$' => {
                body.push(c);
                if let Some(&next) = chars.peek()
                    && next == '('
                {
                    chars.next();
                    body.push('(');
                    depth += 1;
                }
            }
            _ => body.push(c),
        }
    }
    Err(LexError::UnterminatedSubstitution)
}

/// Tokenizes and parses a substitution body, wrapping any errors with the
/// substitution-context `LexError` variants. Empty bodies (whitespace only)
/// produce an empty `Sequence`.
fn parse_substitution_body(body: &str) -> Result<crate::command::Sequence, LexError> {
    let tokens = tokenize(body).map_err(|e| LexError::Substitution(Box::new(e)))?;
    let parsed = crate::command::parse(tokens).map_err(LexError::SubstitutionParseError)?;
    Ok(parsed.unwrap_or_else(empty_sequence))
}

/// Reads the body of a `` `...` `` substitution. The opening backtick is
/// already consumed; this function consumes through the matching unescaped
/// backtick. Applies bash's backtick escape rules:
/// - `\` + `` ` `` -> literal `` ` `` in the body
/// - `\` + `\` -> literal `\` in the body
/// - `\` + `$` -> literal `$` in the body
/// - `\` + any other char `c` -> both `\` and `c` are preserved verbatim
fn scan_backtick_substitution(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
) -> Result<crate::command::Sequence, LexError> {
    let mut body = String::new();
    while let Some(c) = chars.next() {
        match c {
            '`' => {
                return parse_substitution_body(&body);
            }
            '\\' => match chars.next() {
                Some('`') => body.push('`'),
                Some('\\') => body.push('\\'),
                Some('$') => body.push('$'),
                Some(other) => {
                    body.push('\\');
                    body.push(other);
                }
                None => return Err(LexError::UnterminatedSubstitution),
            },
            _ => body.push(c),
        }
    }
    Err(LexError::UnterminatedSubstitution)
}

fn empty_sequence() -> crate::command::Sequence {
    crate::command::Sequence {
        first: crate::command::Command::Pipeline(crate::command::Pipeline {
            commands: Vec::new(),
        }),
        rest: Vec::new(),
        background: false,
    }
}

fn read_var_name(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut name = String::new();
    while let Some(&c) = chars.peek() {
        if is_name_cont(c) {
            name.push(c);
            chars.next();
        } else {
            break;
        }
    }
    name
}

/// Reads a `${...}` parameter expansion. The opening `$` and `{` have
/// already been consumed. Pushes either a `WordPart::Var` (plain `${name}`)
/// or a `WordPart::ParamExpansion` (any modifier).
fn read_braced_param_expansion(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    parts: &mut Vec<WordPart>,
    quoted: bool,
) -> Result<(), LexError> {
    // Special single-char forms: ${@}, ${*}, ${#} (arg count).
    // These must be checked before the Length form (${#name}) disambiguation.
    match chars.peek().copied() {
        Some('@') => {
            chars.next();
            if chars.next() != Some('}') {
                return Err(LexError::UnterminatedBrace);
            }
            parts.push(WordPart::AllArgs { joined: false, quoted });
            return Ok(());
        }
        Some('*') => {
            chars.next();
            if chars.next() != Some('}') {
                return Err(LexError::UnterminatedBrace);
            }
            parts.push(WordPart::AllArgs { joined: true, quoted });
            return Ok(());
        }
        _ => {}
    }

    // Length form (${#name}) vs bare arg-count (${#}).
    // Peek ahead: if the char after `#` is `}`, emit Var { name: "#" }.
    // Otherwise read the identifier name and emit a Length ParamExpansion.
    if chars.peek() == Some(&'#') {
        chars.next(); // consume '#'
        let next = chars.peek().copied();
        if next == Some('}') {
            // ${#} — count of positional args.
            chars.next();
            parts.push(WordPart::Var { name: "#".to_string(), quoted });
            return Ok(());
        }
        // ${#name}: name may be a regular identifier, a digit-only
        // positional name (${#1}, ${#10}), or a special name @/* that
        // means "count of positional args" (same as ${#}).
        let name = match next {
            Some(c) if c.is_ascii_digit() => {
                let mut s = String::new();
                while let Some(&d) = chars.peek() {
                    if d.is_ascii_digit() { s.push(d); chars.next(); } else { break; }
                }
                s
            }
            Some('@') => { chars.next(); "@".to_string() }
            Some('*') => { chars.next(); "*".to_string() }
            _ => read_braced_name(chars)?,
        };
        if name.is_empty() {
            return Err(LexError::EmptyParamName);
        }
        if chars.next() != Some('}') {
            return Err(LexError::UnterminatedBrace);
        }
        parts.push(WordPart::ParamExpansion {
            name,
            modifier: ParamModifier::Length,
            quoted,
        });
        return Ok(());
    }

    // Digit-only positional parameter names: ${1}, ${10}, ${42}, etc.
    if matches!(chars.peek().copied(), Some(c) if c.is_ascii_digit()) {
        let mut name = String::new();
        while let Some(&c) = chars.peek() {
            if c.is_ascii_digit() {
                name.push(c);
                chars.next();
            } else {
                break;
            }
        }
        return dispatch_braced_modifier(name, quoted, chars, parts);
    }

    let name = read_braced_name(chars)?;
    if name.is_empty() {
        return Err(LexError::EmptyParamName);
    }
    dispatch_braced_modifier(name, quoted, chars, parts)
}

/// Reads identifier chars (the parameter name) inside a `${...}` until it
/// hits a non-identifier char. Does NOT consume the terminator.
fn read_braced_name(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
) -> Result<String, LexError> {
    let mut name = String::new();
    while let Some(&c) = chars.peek() {
        if c == '_' || c.is_ascii_alphanumeric() {
            if name.is_empty() && c.is_ascii_digit() {
                return Err(LexError::InvalidVarName);
            }
            name.push(c);
            chars.next();
        } else {
            break;
        }
    }
    Ok(name)
}

/// Dispatches a `${name<modifier>...}` form once `name` has been read. The
/// next char to read from `chars` is whatever follows the name (typically
/// `}`, `:`, `-`, `=`, `?`, `+`, `#`, `%`, or `/`). Pushes a single
/// `WordPart` (`Var` or `ParamExpansion`) onto `parts`.
fn dispatch_braced_modifier(
    name: String,
    quoted: bool,
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    parts: &mut Vec<WordPart>,
) -> Result<(), LexError> {
    match chars.next() {
        Some('}') => {
            parts.push(WordPart::Var { name, quoted });
            Ok(())
        }
        Some(':') => {
            match chars.peek().copied() {
                Some('-') => {
                    chars.next();
                    let modifier = modifier_with_operand(chars, |w| ParamModifier::UseDefault { word: w, colon: true })?;
                    parts.push(WordPart::ParamExpansion { name, modifier, quoted });
                    Ok(())
                }
                Some('=') => {
                    chars.next();
                    let modifier = modifier_with_operand(chars, |w| ParamModifier::AssignDefault { word: w, colon: true })?;
                    parts.push(WordPart::ParamExpansion { name, modifier, quoted });
                    Ok(())
                }
                Some('?') => {
                    chars.next();
                    let modifier = modifier_with_operand(chars, |w| ParamModifier::ErrorIfUnset { word: w, colon: true })?;
                    parts.push(WordPart::ParamExpansion { name, modifier, quoted });
                    Ok(())
                }
                Some('+') => {
                    chars.next();
                    let modifier = modifier_with_operand(chars, |w| ParamModifier::UseAlternate { word: w, colon: true })?;
                    parts.push(WordPart::ParamExpansion { name, modifier, quoted });
                    Ok(())
                }
                Some('}') => Err(LexError::InvalidBraceModifier(":".to_string())),
                Some(_) => {
                    let (offset, length) = scan_substring_operands(chars)?;
                    parts.push(WordPart::ParamExpansion {
                        name,
                        modifier: ParamModifier::Substring { offset, length },
                        quoted,
                    });
                    Ok(())
                }
                None => Err(LexError::UnterminatedBrace),
            }
        }
        Some('-') => {
            let modifier = modifier_with_operand(chars, |w| ParamModifier::UseDefault { word: w, colon: false })?;
            parts.push(WordPart::ParamExpansion { name, modifier, quoted });
            Ok(())
        }
        Some('=') => {
            let modifier = modifier_with_operand(chars, |w| ParamModifier::AssignDefault { word: w, colon: false })?;
            parts.push(WordPart::ParamExpansion { name, modifier, quoted });
            Ok(())
        }
        Some('?') => {
            let modifier = modifier_with_operand(chars, |w| ParamModifier::ErrorIfUnset { word: w, colon: false })?;
            parts.push(WordPart::ParamExpansion { name, modifier, quoted });
            Ok(())
        }
        Some('+') => {
            let modifier = modifier_with_operand(chars, |w| ParamModifier::UseAlternate { word: w, colon: false })?;
            parts.push(WordPart::ParamExpansion { name, modifier, quoted });
            Ok(())
        }
        Some('#') => {
            let longest = chars.peek() == Some(&'#');
            if longest { chars.next(); }
            let modifier = modifier_with_operand(chars, |w| ParamModifier::RemovePrefix { pattern: w, longest })?;
            parts.push(WordPart::ParamExpansion { name, modifier, quoted });
            Ok(())
        }
        Some('%') => {
            let longest = chars.peek() == Some(&'%');
            if longest { chars.next(); }
            let modifier = modifier_with_operand(chars, |w| ParamModifier::RemoveSuffix { pattern: w, longest })?;
            parts.push(WordPart::ParamExpansion { name, modifier, quoted });
            Ok(())
        }
        Some('/') => {
            let all = chars.peek() == Some(&'/');
            if all { chars.next(); }
            let anchor = match chars.peek().copied() {
                Some('#') if !all => { chars.next(); SubstAnchor::Prefix }
                Some('%') if !all => { chars.next(); SubstAnchor::Suffix }
                _ => SubstAnchor::None,
            };
            let (pattern, replacement) = scan_substitution_operand(chars)?;
            parts.push(WordPart::ParamExpansion {
                name,
                modifier: ParamModifier::Substitute { pattern, replacement, anchor, all },
                quoted,
            });
            Ok(())
        }
        Some(c) => Err(LexError::InvalidBraceModifier(c.to_string())),
        None => Err(LexError::UnterminatedBrace),
    }
}

/// Scans the operand text until the matching `}` and parses it as a single
/// `Word`. Builds the `ParamModifier` via the caller's closure.
fn modifier_with_operand<F>(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    build: F,
) -> Result<ParamModifier, LexError>
where
    F: FnOnce(Word) -> ParamModifier,
{
    let body = scan_braced_operand(chars)?;
    let word = parse_braced_operand(&body)?;
    Ok(build(word))
}

/// Walks the chars iterator from just after the leading `/` of a
/// substitution operand. Delegates to `scan_braced_operand` to collect the
/// raw body (which depth-tracks nested `${...}` and protects `}` inside
/// quoted spans), then splits pattern from replacement on the first
/// unescaped `/` at brace-depth zero outside any quoted span. `\/` becomes
/// a literal `/`; `\\` becomes a literal `\`; any other `\x` passes
/// through unchanged so the inner operand tokenizer sees it.
fn scan_substitution_operand(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
) -> Result<(Word, Word), LexError> {
    let body = scan_braced_operand(chars)?;
    let (pattern_src, replacement_src) = split_substitution_body(&body);
    let pattern = parse_braced_operand(&pattern_src)?;
    let replacement = parse_braced_operand(&replacement_src)?;
    Ok((pattern, replacement))
}

/// Splits a substitution-operand body (as returned by `scan_braced_operand`)
/// on the first unescaped `/` that sits at brace-depth zero outside any
/// quoted span. Returns `(pattern_src, replacement_src)`. If no delimiter
/// is found, the whole body is the pattern and the replacement is empty
/// (the bash `${var/pat}` form).
fn split_substitution_body(body: &str) -> (String, String) {
    let mut pattern = String::new();
    let mut replacement = String::new();
    let mut delim_seen = false;
    let mut depth: u32 = 0;
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                let lit = match chars.peek().copied() {
                    Some('/') => { chars.next(); '/' }
                    Some('\\') => { chars.next(); '\\' }
                    _ => '\\',
                };
                if delim_seen { replacement.push(lit); } else { pattern.push(lit); }
            }
            '"' => {
                let dst = if delim_seen { &mut replacement } else { &mut pattern };
                dst.push('"');
                while let Some(qc) = chars.next() {
                    dst.push(qc);
                    if qc == '\\' {
                        if let Some(nc) = chars.next() { dst.push(nc); }
                    } else if qc == '"' {
                        break;
                    }
                }
            }
            '\'' => {
                let dst = if delim_seen { &mut replacement } else { &mut pattern };
                dst.push('\'');
                for qc in chars.by_ref() {
                    dst.push(qc);
                    if qc == '\'' { break; }
                }
            }
            '{' => {
                depth += 1;
                if delim_seen { replacement.push('{'); } else { pattern.push('{'); }
            }
            '}' => {
                depth = depth.saturating_sub(1);
                if delim_seen { replacement.push('}'); } else { pattern.push('}'); }
            }
            '/' if depth == 0 && !delim_seen => { delim_seen = true; }
            _ => {
                if delim_seen { replacement.push(c); } else { pattern.push(c); }
            }
        }
    }
    (pattern, replacement)
}

/// Scans a `${var:offset}` / `${var:offset:length}` operand pair. Delegates
/// to `scan_braced_operand` + `split_substring_body` + `parse_braced_operand`
/// to collect and parse the offset and optional length Words.
fn scan_substring_operands(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
) -> Result<(Word, Option<Word>), LexError> {
    let body = scan_braced_operand(chars)?;
    let (offset_src, length_src) = split_substring_body(&body);
    let offset = parse_braced_operand(&offset_src)?;
    let length = match length_src {
        Some(s) => Some(parse_braced_operand(&s)?),
        None => None,
    };
    Ok((offset, length))
}

/// Splits a substring-operand body (as returned by `scan_braced_operand`)
/// on the first unescaped `:` that sits at brace-depth zero outside any
/// quoted span. Returns `(offset_src, Some(length_src))` if a delimiter
/// was found, or `(offset_src, None)` otherwise (the no-length form).
fn split_substring_body(body: &str) -> (String, Option<String>) {
    let mut offset = String::new();
    let mut length = String::new();
    let mut delim_seen = false;
    let mut depth: u32 = 0;
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                let lit = match chars.peek().copied() {
                    Some(':') => { chars.next(); ':' }
                    Some('\\') => { chars.next(); '\\' }
                    _ => '\\',
                };
                if delim_seen { length.push(lit); } else { offset.push(lit); }
            }
            '"' => {
                let dst = if delim_seen { &mut length } else { &mut offset };
                dst.push('"');
                while let Some(qc) = chars.next() {
                    dst.push(qc);
                    if qc == '\\' {
                        if let Some(nc) = chars.next() { dst.push(nc); }
                    } else if qc == '"' {
                        break;
                    }
                }
            }
            '\'' => {
                let dst = if delim_seen { &mut length } else { &mut offset };
                dst.push('\'');
                for qc in chars.by_ref() {
                    dst.push(qc);
                    if qc == '\'' { break; }
                }
            }
            '{' => {
                depth += 1;
                if delim_seen { length.push('{'); } else { offset.push('{'); }
            }
            '}' => {
                depth = depth.saturating_sub(1);
                if delim_seen { length.push('}'); } else { offset.push('}'); }
            }
            ':' if depth == 0 && !delim_seen => { delim_seen = true; }
            _ => {
                if delim_seen { length.push(c); } else { offset.push(c); }
            }
        }
    }
    if delim_seen {
        (offset, Some(length))
    } else {
        (offset, None)
    }
}

fn is_name_start(c: char) -> bool {
    c == '_' || c.is_ascii_alphabetic()
}

fn is_name_cont(c: char) -> bool {
    c == '_' || c.is_ascii_alphanumeric()
}

/// Tries to consume a tilde construct starting just after the `~`.
/// On success, returns the `TildeSpec` (consuming any extra chars, e.g.
/// the `+` in `~+`). On failure, leaves the iterator untouched and
/// returns `None` (the caller treats `~` as a literal).
fn try_parse_tilde(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    in_assignment_value: bool,
) -> Option<TildeSpec> {
    let term = |c: char| is_tilde_terminator(c) || (in_assignment_value && c == ':');
    match chars.peek().copied() {
        // Bare ~ at end of word.
        None => Some(TildeSpec::Home),
        Some(c) if term(c) => Some(TildeSpec::Home),
        // ~+, ~- — must be followed by terminator (or nothing).
        Some('+') => {
            let mut lookahead = chars.clone();
            lookahead.next(); // consume the +
            match lookahead.peek().copied() {
                None => { chars.next(); Some(TildeSpec::Pwd) }
                Some(c) if term(c) => { chars.next(); Some(TildeSpec::Pwd) }
                _ => None,
            }
        }
        Some('-') => {
            let mut lookahead = chars.clone();
            lookahead.next();
            match lookahead.peek().copied() {
                None => { chars.next(); Some(TildeSpec::OldPwd) }
                Some(c) if term(c) => { chars.next(); Some(TildeSpec::OldPwd) }
                _ => None,
            }
        }
        Some(c) if is_user_name_start(c) => {
            // Scan a maximal identifier; the tail after must be a terminator.
            let mut lookahead = chars.clone();
            let mut name = String::new();
            while let Some(&nc) = lookahead.peek() {
                if is_user_name_continue(nc) {
                    name.push(nc);
                    lookahead.next();
                } else {
                    break;
                }
            }
            let tail_ok = match lookahead.peek().copied() {
                None => true,
                Some(c) => term(c),
            };
            if tail_ok && !name.is_empty() {
                // Consume the scanned chars from the real iterator.
                // Safe: is_user_name_start/continue only accept ASCII, so
                // name.len() (bytes) equals the char count.
                for _ in 0..name.len() {
                    chars.next();
                }
                Some(TildeSpec::User(name))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn is_tilde_terminator(c: char) -> bool {
    c == '/'
        || c.is_whitespace()
        || matches!(c, '|' | '<' | '>' | '&' | ';')
}

fn tilde_eligible_in_assignment(in_assignment_value: bool, current: &str) -> bool {
    if !in_assignment_value {
        return false;
    }
    matches!(current.chars().last(), Some(':') | Some('='))
}

/// True iff the unquoted text accumulated so far for the current word
/// forms a valid shell identifier (matches [A-Za-z_]\w*).
fn word_is_identifier_so_far(current: &str, parts: &[WordPart]) -> bool {
    // The word so far must be exactly `parts ++ current` where every
    // WordPart is a Literal (no Var/Tilde/CommandSub etc), AND the
    // concatenation is a non-empty identifier.
    let mut joined = String::new();
    for p in parts {
        if let WordPart::Literal { text, quoted: false } = p {
            joined.push_str(text);
        } else {
            return false;
        }
    }
    joined.push_str(current);
    if joined.is_empty() {
        return false;
    }
    let mut iter = joined.chars();
    let first = iter.next().unwrap();
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return false;
    }
    iter.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

fn is_user_name_start(c: char) -> bool {
    c == '_' || c.is_ascii_alphabetic()
}

fn is_user_name_continue(c: char) -> bool {
    c == '_' || c.is_ascii_alphanumeric()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a Token that holds a single-Literal Word.
    fn w(s: &str) -> Token {
        Token::Word(Word(vec![WordPart::Literal { text: s.to_string(), quoted: false }]))
    }

    /// Builds a Token that holds a single quoted-Literal Word.
    fn wq(s: &str) -> Token {
        Token::Word(Word(vec![WordPart::Literal { text: s.to_string(), quoted: true }]))
    }

    /// Builds a Vec<Token> of all-Literal words.
    fn words(parts: &[&str]) -> Vec<Token> {
        parts.iter().map(|s| w(s)).collect()
    }

    /// Test alias so the v32 substitution tests read more naturally.
    fn tokenize_words(input: &str) -> Result<Vec<Token>, LexError> {
        tokenize(input)
    }

    /// Pops the first token from `tokens`, asserts it's a single-part Word,
    /// and returns that `WordPart`.
    fn single_param_expansion(tokens: &mut Vec<Token>) -> WordPart {
        let word = match tokens.remove(0) {
            Token::Word(w) => w,
            other => panic!("expected Word, got {:?}", other),
        };
        word.0.into_iter().next().expect("non-empty word")
    }

    /// Flattens the literal text parts of a `Word`, ignoring non-literal
    /// parts. Useful for asserting on simple operand bodies in tests.
    fn word_to_literal(w: &Word) -> String {
        let mut s = String::new();
        for p in &w.0 {
            if let WordPart::Literal { text, .. } = p {
                s.push_str(text);
            }
        }
        s
    }

    #[test]
    fn tokenize_simple_command() {
        assert_eq!(tokenize("ls -la").unwrap(), words(&["ls", "-la"]));
    }

    #[test]
    fn tokenize_empty_input() {
        assert_eq!(tokenize("").unwrap(), Vec::<Token>::new());
    }

    #[test]
    fn tokenize_only_whitespace() {
        assert_eq!(tokenize("   \t  ").unwrap(), Vec::<Token>::new());
    }

    #[test]
    fn tokenize_full_line_comment() {
        assert_eq!(tokenize("# just a comment").unwrap(), Vec::<Token>::new());
    }

    #[test]
    fn tokenize_comment_to_newline() {
        assert_eq!(
            tokenize("# comment\necho hi").unwrap(),
            vec![Token::Newline, w("echo"), w("hi")]
        );
    }

    #[test]
    fn tokenize_trailing_comment() {
        assert_eq!(
            tokenize("echo hi # trailing").unwrap(),
            vec![w("echo"), w("hi")]
        );
    }

    #[test]
    fn tokenize_trailing_comment_then_next_line() {
        assert_eq!(
            tokenize("echo a # comment\necho b").unwrap(),
            vec![w("echo"), w("a"), Token::Newline, w("echo"), w("b")]
        );
    }

    #[test]
    fn tokenize_hash_inside_word_is_literal() {
        // bash: `echo foo#bar` outputs `foo#bar` (# mid-word is not a comment).
        assert_eq!(
            tokenize("echo foo#bar").unwrap(),
            vec![w("echo"), w("foo#bar")]
        );
    }

    #[test]
    fn tokenize_hash_after_semicolon_is_comment() {
        assert_eq!(
            tokenize("echo a; # comment").unwrap(),
            vec![w("echo"), w("a"), Token::Op(Operator::Semi)]
        );
    }

    #[test]
    fn tokenize_hash_inside_single_quotes_is_literal() {
        assert_eq!(
            tokenize("echo '# inside'").unwrap(),
            vec![w("echo"), wq("# inside")]
        );
    }

    #[test]
    fn tokenize_hash_inside_double_quotes_is_literal() {
        assert_eq!(
            tokenize("echo \"# inside\"").unwrap(),
            vec![w("echo"), wq("# inside")]
        );
    }

    #[test]
    fn tokenize_backslash_newline_is_line_continuation_with_space() {
        // POSIX: \<NL> is deleted; surrounding whitespace still separates words.
        assert_eq!(
            tokenize("echo \\\nfoo").unwrap(),
            vec![w("echo"), w("foo")]
        );
    }

    #[test]
    fn tokenize_backslash_newline_joins_adjacent_chars_into_one_word() {
        // No separator on either side: result is one word "echofoo".
        assert_eq!(
            tokenize("echo\\\nfoo").unwrap(),
            vec![w("echofoo")]
        );
    }

    #[test]
    fn tokenize_backslash_newline_inside_double_quotes_is_line_continuation() {
        // POSIX 2.2.3: \<NL> retains its special meaning inside "...".
        assert_eq!(
            tokenize("\"foo\\\nbar\"").unwrap(),
            vec![wq("foobar")]
        );
    }

    #[test]
    fn tokenize_backslash_newline_inside_single_quotes_is_literal() {
        // POSIX 2.2.2: no escape interpretation inside '...'.
        assert_eq!(
            tokenize("'foo\\\nbar'").unwrap(),
            vec![wq("foo\\\nbar")]
        );
    }

    #[test]
    fn tokenize_lone_backslash_newline_is_empty() {
        assert_eq!(tokenize("\\\n").unwrap(), Vec::<Token>::new());
    }

    #[test]
    fn tokenize_escaped_backtick_in_double_quotes_is_literal() {
        // POSIX: inside double quotes, `\\\`` is a literal backtick.
        // Was a bug: huck only recognized `\"`, `\\`, `\$` as escapes.
        assert_eq!(
            tokenize(r#""\`""#).unwrap(),
            vec![wq("`")]
        );
    }

    #[test]
    fn tokenize_escaped_hash_is_literal() {
        // `\#` at word start: backslash escape, # is literal
        assert_eq!(
            tokenize(r"echo \#hash").unwrap(),
            vec![w("echo"), Token::Word(Word(vec![
                WordPart::Literal { text: "#".to_string(), quoted: true },
                WordPart::Literal { text: "hash".to_string(), quoted: false },
            ]))]
        );
    }

    #[test]
    fn tokenize_single_quotes() {
        assert_eq!(
            tokenize("echo 'hello world'").unwrap(),
            vec![w("echo"), wq("hello world")]
        );
    }

    #[test]
    fn tokenize_double_quotes() {
        assert_eq!(
            tokenize("echo \"hello world\"").unwrap(),
            vec![w("echo"), wq("hello world")]
        );
    }

    #[test]
    fn tokenize_double_quote_escape() {
        assert_eq!(tokenize(r#"echo "a\"b""#).unwrap(), vec![w("echo"), wq("a\"b")]);
    }

    #[test]
    fn tokenize_backslash_escape_outside_quotes() {
        // Backslash flushes the unquoted run and pushes the escaped char as a
        // quoted single-char Literal. So `a\ b` is one Word made of three parts:
        // unquoted "a", quoted " ", unquoted "b". This preserves the quoting
        // information that pathname expansion needs (the escaped char must not
        // be treated as a glob metachar).
        assert_eq!(
            tokenize(r"echo a\ b").unwrap(),
            vec![
                w("echo"),
                Token::Word(Word(vec![
                    WordPart::Literal { text: "a".to_string(), quoted: false },
                    WordPart::Literal { text: " ".to_string(), quoted: true },
                    WordPart::Literal { text: "b".to_string(), quoted: false },
                ])),
            ]
        );
    }

    #[test]
    fn tokenize_trailing_backslash_is_literal() {
        assert_eq!(tokenize(r"echo a\").unwrap(), words(&["echo", r"a\"]));
    }

    #[test]
    fn backslash_escaped_metachar_is_quoted_literal() {
        let tokens = tokenize("\\*").unwrap();
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        assert_eq!(parts, &[WordPart::Literal { text: "*".to_string(), quoted: true }]);
    }

    #[test]
    fn backslash_in_middle_of_word_flushes_and_quotes() {
        // `foo\*bar` → unquoted "foo", quoted "*", unquoted "bar"
        let tokens = tokenize("foo\\*bar").unwrap();
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        assert_eq!(parts, &[
            WordPart::Literal { text: "foo".to_string(), quoted: false },
            WordPart::Literal { text: "*".to_string(), quoted: true },
            WordPart::Literal { text: "bar".to_string(), quoted: false },
        ]);
    }

    #[test]
    fn tokenize_adjacent_runs_concatenate() {
        // `foo"bar baz"` flushes at the quote boundary: one Word with two
        // parts, the unquoted `foo` and the quoted `bar baz`.
        assert_eq!(
            tokenize(r#"foo"bar baz""#).unwrap(),
            vec![Token::Word(Word(vec![
                WordPart::Literal { text: "foo".to_string(), quoted: false },
                WordPart::Literal { text: "bar baz".to_string(), quoted: true },
            ]))]
        );
    }

    #[test]
    fn tokenize_single_quotes_preserve_backslash() {
        assert_eq!(tokenize(r"echo 'a\b'").unwrap(), vec![w("echo"), wq(r"a\b")]);
    }

    #[test]
    fn tokenize_empty_quotes_produce_empty_token() {
        assert_eq!(tokenize("''").unwrap(), vec![wq("")]);
    }

    #[test]
    fn tokenize_unterminated_single_quote() {
        assert_eq!(
            tokenize("echo 'oops").unwrap_err(),
            LexError::UnterminatedQuote
        );
    }

    #[test]
    fn tokenize_unterminated_double_quote() {
        assert_eq!(
            tokenize("echo \"oops").unwrap_err(),
            LexError::UnterminatedQuote
        );
    }

    #[test]
    fn tokenize_pipe_with_spaces() {
        assert_eq!(
            tokenize("a | b").unwrap(),
            vec![w("a"), Token::Op(Operator::Pipe), w("b")]
        );
    }

    #[test]
    fn tokenize_pipe_without_spaces() {
        assert_eq!(
            tokenize("a|b").unwrap(),
            vec![w("a"), Token::Op(Operator::Pipe), w("b")]
        );
    }

    #[test]
    fn tokenize_redirect_out() {
        assert_eq!(
            tokenize("ls > f").unwrap(),
            vec![w("ls"), Token::Op(Operator::RedirOut), w("f")]
        );
    }

    #[test]
    fn tokenize_redirect_out_without_spaces() {
        assert_eq!(
            tokenize("ls>f").unwrap(),
            vec![w("ls"), Token::Op(Operator::RedirOut), w("f")]
        );
    }

    #[test]
    fn tokenize_redirect_append() {
        assert_eq!(
            tokenize("ls >> f").unwrap(),
            vec![w("ls"), Token::Op(Operator::RedirAppend), w("f")]
        );
    }

    #[test]
    fn tokenize_redirect_in() {
        assert_eq!(
            tokenize("cat < f").unwrap(),
            vec![w("cat"), Token::Op(Operator::RedirIn), w("f")]
        );
    }

    #[test]
    fn tokenize_redirect_stderr() {
        assert_eq!(
            tokenize("cmd 2> f").unwrap(),
            vec![w("cmd"), Token::Op(Operator::RedirErr), w("f")]
        );
    }

    #[test]
    fn tokenize_redirect_stderr_append() {
        assert_eq!(
            tokenize("cmd 2>> f").unwrap(),
            vec![w("cmd"), Token::Op(Operator::RedirErrAppend), w("f")]
        );
    }

    #[test]
    fn tokenize_two_in_word_is_not_stderr_operator() {
        assert_eq!(
            tokenize("x2>f").unwrap(),
            vec![w("x2"), Token::Op(Operator::RedirOut), w("f")]
        );
    }

    #[test]
    fn tokenize_two_not_followed_by_redirect_is_a_word() {
        assert_eq!(tokenize("2 foo").unwrap(), words(&["2", "foo"]));
    }

    #[test]
    fn tokenize_quoted_operators_stay_words() {
        assert_eq!(
            tokenize(r#"echo "|" ">""#).unwrap(),
            vec![w("echo"), wq("|"), wq(">")]
        );
    }

    #[test]
    fn tokenize_escaped_operators_stay_words() {
        // Escaped operators become quoted single-char Literals (one Word each).
        assert_eq!(
            tokenize(r"echo \| \>").unwrap(),
            vec![w("echo"), wq("|"), wq(">")]
        );
    }

    #[test]
    fn tokenize_pipeline_with_redirects() {
        assert_eq!(
            tokenize("a < in | b > out").unwrap(),
            vec![
                w("a"),
                Token::Op(Operator::RedirIn),
                w("in"),
                Token::Op(Operator::Pipe),
                w("b"),
                Token::Op(Operator::RedirOut),
                w("out"),
            ]
        );
    }

    #[test]
    fn tokenize_or_with_spaces() {
        assert_eq!(
            tokenize("a || b").unwrap(),
            vec![w("a"), Token::Op(Operator::Or), w("b")]
        );
    }

    #[test]
    fn tokenize_or_without_spaces() {
        assert_eq!(
            tokenize("a||b").unwrap(),
            vec![w("a"), Token::Op(Operator::Or), w("b")]
        );
    }

    #[test]
    fn tokenize_and_with_spaces() {
        assert_eq!(
            tokenize("a && b").unwrap(),
            vec![w("a"), Token::Op(Operator::And), w("b")]
        );
    }

    #[test]
    fn tokenize_and_without_spaces() {
        assert_eq!(
            tokenize("a&&b").unwrap(),
            vec![w("a"), Token::Op(Operator::And), w("b")]
        );
    }

    #[test]
    fn tokenize_bare_ampersand_is_background_op() {
        assert_eq!(
            tokenize("a & b").unwrap(),
            vec![w("a"), Token::Op(Operator::Background), w("b")]
        );
    }

    #[test]
    fn tokenize_bare_ampersand_at_end_is_background_op() {
        assert_eq!(
            tokenize("a &").unwrap(),
            vec![w("a"), Token::Op(Operator::Background)]
        );
    }

    #[test]
    fn tokenize_double_ampersand_still_and_op() {
        assert_eq!(
            tokenize("a && b").unwrap(),
            vec![w("a"), Token::Op(Operator::And), w("b")]
        );
    }

    #[test]
    fn tokenize_two_separate_backgrounds() {
        assert_eq!(
            tokenize("a & &").unwrap(),
            vec![
                w("a"),
                Token::Op(Operator::Background),
                Token::Op(Operator::Background),
            ]
        );
    }

    #[test]
    fn tokenize_semicolon_with_spaces() {
        assert_eq!(
            tokenize("a ; b").unwrap(),
            vec![w("a"), Token::Op(Operator::Semi), w("b")]
        );
    }

    #[test]
    fn tokenize_semicolon_without_spaces() {
        assert_eq!(
            tokenize("a;b").unwrap(),
            vec![w("a"), Token::Op(Operator::Semi), w("b")]
        );
    }

    #[test]
    fn tokenize_quoted_sequencing_operators_stay_words() {
        assert_eq!(
            tokenize(r#"echo "&&" "||" ";""#).unwrap(),
            vec![w("echo"), wq("&&"), wq("||"), wq(";")]
        );
    }

    #[test]
    fn tokenize_escaped_sequencing_operators_stay_words() {
        // Each `\X` becomes its own quoted single-char Literal part. Adjacent
        // escapes within the same token concatenate into one Word with N parts.
        let two_quoted = |a: &str, b: &str| {
            Token::Word(Word(vec![
                WordPart::Literal { text: a.to_string(), quoted: true },
                WordPart::Literal { text: b.to_string(), quoted: true },
            ]))
        };
        assert_eq!(
            tokenize(r"echo \&\& \|\| \;").unwrap(),
            vec![w("echo"), two_quoted("&", "&"), two_quoted("|", "|"), wq(";")]
        );
    }

    #[test]
    fn tokenize_combined_sequencing_operators() {
        assert_eq!(
            tokenize("a && b || c ; d").unwrap(),
            vec![
                w("a"),
                Token::Op(Operator::And),
                w("b"),
                Token::Op(Operator::Or),
                w("c"),
                Token::Op(Operator::Semi),
                w("d"),
            ]
        );
    }

    fn vword_unquoted(name: &str) -> Token {
        Token::Word(Word(vec![WordPart::Var {
            name: name.to_string(),
            quoted: false,
        }]))
    }

    fn vword_quoted(name: &str) -> Token {
        Token::Word(Word(vec![WordPart::Var {
            name: name.to_string(),
            quoted: true,
        }]))
    }

    #[test]
    fn tokenize_dollar_var_unquoted() {
        assert_eq!(tokenize("$FOO").unwrap(), vec![vword_unquoted("FOO")]);
    }

    #[test]
    fn tokenize_dollar_var_braced() {
        assert_eq!(tokenize("${FOO}").unwrap(), vec![vword_unquoted("FOO")]);
    }

    #[test]
    fn tokenize_dollar_var_in_double_quotes_is_quoted() {
        assert_eq!(tokenize("\"$FOO\"").unwrap(), vec![vword_quoted("FOO")]);
    }

    #[test]
    fn tokenize_dollar_var_in_single_quotes_is_literal() {
        assert_eq!(tokenize("'$FOO'").unwrap(), vec![wq("$FOO")]);
    }

    #[test]
    fn tokenize_last_status() {
        assert_eq!(
            tokenize("$?").unwrap(),
            vec![Token::Word(Word(vec![WordPart::LastStatus {
                quoted: false
            }]))]
        );
    }

    #[test]
    fn tokenize_dollar_then_digit_is_positional_param() {
        // Since v22 Task 4: $<digit> is a positional parameter, not a literal $.
        assert_eq!(
            tokenize("$5").unwrap(),
            vec![Token::Word(Word(vec![
                WordPart::Var { name: "5".to_string(), quoted: false },
            ]))]
        );
    }

    #[test]
    fn tokenize_double_dollar_is_var_name_dollar() {
        // v26: $$ is the shell PID special parameter, not two literal dollars.
        assert_eq!(
            tokenize("$$").unwrap(),
            vec![Token::Word(Word(vec![
                WordPart::Var { name: "$".to_string(), quoted: false },
            ]))]
        );
    }

    #[test]
    fn tokenize_tilde_alone() {
        assert_eq!(
            tokenize("~").unwrap(),
            vec![Token::Word(Word(vec![WordPart::Tilde(TildeSpec::Home)]))]
        );
    }

    #[test]
    fn tokenize_tilde_slash_path() {
        assert_eq!(
            tokenize("~/foo").unwrap(),
            vec![Token::Word(Word(vec![
                WordPart::Tilde(TildeSpec::Home),
                WordPart::Literal { text: "/foo".to_string(), quoted: false },
            ]))]
        );
    }

    #[test]
    fn tokenize_tilde_mid_word_is_literal() {
        assert_eq!(tokenize("a~b").unwrap(), words(&["a~b"]));
    }

    #[test]
    fn tokenize_tilde_followed_by_name_is_user_form() {
        assert_eq!(
            tokenize("~foo").unwrap(),
            vec![Token::Word(Word(vec![
                WordPart::Tilde(TildeSpec::User("foo".to_string())),
            ]))]
        );
    }

    #[test]
    fn tokenize_tilde_user_alone() {
        assert_eq!(
            tokenize("~alice").unwrap(),
            vec![Token::Word(Word(vec![
                WordPart::Tilde(TildeSpec::User("alice".to_string())),
            ]))]
        );
    }

    #[test]
    fn tokenize_tilde_user_slash_path() {
        assert_eq!(
            tokenize("~alice/bin").unwrap(),
            vec![Token::Word(Word(vec![
                WordPart::Tilde(TildeSpec::User("alice".to_string())),
                WordPart::Literal { text: "/bin".to_string(), quoted: false },
            ]))]
        );
    }

    #[test]
    fn tokenize_tilde_user_with_underscore_and_digits() {
        assert_eq!(
            tokenize("~alice_123").unwrap(),
            vec![Token::Word(Word(vec![
                WordPart::Tilde(TildeSpec::User("alice_123".to_string())),
            ]))]
        );
    }

    #[test]
    fn tokenize_tilde_in_quotes_is_literal() {
        assert_eq!(tokenize("\"~\"").unwrap(), vec![wq("~")]);
    }

    #[test]
    fn tokenize_braced_var_invalid_name() {
        // ${1foo}: digits are consumed as a positional name, then `f` is
        // found which is not a valid modifier → InvalidBraceModifier (v33:
        // digit branch now routes through dispatch_braced_modifier).
        assert!(matches!(tokenize("${1foo}").unwrap_err(), LexError::InvalidBraceModifier(_)));
    }

    #[test]
    fn tokenize_braced_var_empty_name() {
        assert_eq!(tokenize("${}").unwrap_err(), LexError::EmptyParamName);
    }

    #[test]
    fn tokenize_unterminated_brace() {
        assert_eq!(tokenize("${FOO").unwrap_err(), LexError::UnterminatedBrace);
    }

    #[test]
    fn tokenize_var_concatenates_with_literal() {
        assert_eq!(
            tokenize("a$FOOb").unwrap(),
            vec![Token::Word(Word(vec![
                WordPart::Literal { text: "a".to_string(), quoted: false },
                WordPart::Var { name: "FOOb".to_string(), quoted: false },
            ]))]
        );
    }

    #[test]
    fn tokenize_braced_var_separates_from_following_word() {
        assert_eq!(
            tokenize("${FOO}bar").unwrap(),
            vec![Token::Word(Word(vec![
                WordPart::Var { name: "FOO".to_string(), quoted: false },
                WordPart::Literal { text: "bar".to_string(), quoted: false },
            ]))]
        );
    }

    #[test]
    fn tokenize_escaped_dollar_in_double_quotes_is_literal() {
        assert_eq!(tokenize(r#""\$FOO""#).unwrap(), vec![wq("$FOO")]);
    }

    #[test]
    fn tokenize_two_adjacent_vars() {
        assert_eq!(
            tokenize("$FOO$BAR").unwrap(),
            vec![Token::Word(Word(vec![
                WordPart::Var { name: "FOO".to_string(), quoted: false },
                WordPart::Var { name: "BAR".to_string(), quoted: false },
            ]))]
        );
    }

    fn sub_word(parts: Vec<WordPart>) -> Token {
        Token::Word(Word(parts))
    }

    fn echo_seq(args: &[&str]) -> crate::command::Sequence {
        use crate::command::{Command, ExecCommand, Pipeline, Sequence, SimpleCommand};
        Sequence {
            first: Command::Pipeline(Pipeline {
                commands: vec![Command::Simple(SimpleCommand::Exec(ExecCommand {
                    inline_assignments: Vec::new(),
                    program: Word(vec![WordPart::Literal { text: "echo".to_string(), quoted: false }]),
                    args: args
                        .iter()
                        .map(|a| Word(vec![WordPart::Literal { text: a.to_string(), quoted: false }]))
                        .collect(),
                    stdin: None,
                    stdout: None,
                    stderr: None,
                }))],
            }),
            rest: vec![],
            background: false,
        }
    }

    #[test]
    fn tokenize_command_sub_basic() {
        assert_eq!(
            tokenize("$(echo hi)").unwrap(),
            vec![sub_word(vec![WordPart::CommandSub {
                sequence: echo_seq(&["hi"]),
                quoted: false,
            }])]
        );
    }

    #[test]
    fn tokenize_command_sub_quoted_in_double_quotes() {
        assert_eq!(
            tokenize("\"$(echo hi)\"").unwrap(),
            vec![sub_word(vec![WordPart::CommandSub {
                sequence: echo_seq(&["hi"]),
                quoted: true,
            }])]
        );
    }

    #[test]
    fn tokenize_command_sub_in_single_quotes_is_literal() {
        assert_eq!(
            tokenize("'$(echo hi)'").unwrap(),
            vec![wq("$(echo hi)")]
        );
    }

    #[test]
    fn tokenize_command_sub_empty() {
        assert_eq!(
            tokenize("$()").unwrap(),
            vec![sub_word(vec![WordPart::CommandSub {
                sequence: crate::command::Sequence {
                    first: crate::command::Command::Pipeline(
                        crate::command::Pipeline { commands: vec![] },
                    ),
                    rest: vec![],
                    background: false,
                },
                quoted: false,
            }])]
        );
    }

    #[test]
    fn tokenize_command_sub_with_quoted_paren_in_body() {
        // The `)` inside `"..."` does not close the substitution. The inner
        // `")"` arg is quoted, so the inner Literal carries quoted: true.
        use crate::command::{Command, ExecCommand, Pipeline, Sequence, SimpleCommand};
        let inner = Sequence {
            first: Command::Pipeline(Pipeline {
                commands: vec![Command::Simple(SimpleCommand::Exec(ExecCommand {
                    inline_assignments: Vec::new(),
                    program: Word(vec![WordPart::Literal { text: "echo".to_string(), quoted: false }]),
                    args: vec![Word(vec![WordPart::Literal { text: ")".to_string(), quoted: true }])],
                    stdin: None,
                    stdout: None,
                    stderr: None,
                }))],
            }),
            rest: vec![],
            background: false,
        };
        assert_eq!(
            tokenize("$(echo \")\")").unwrap(),
            vec![sub_word(vec![WordPart::CommandSub {
                sequence: inner,
                quoted: false,
            }])]
        );
    }

    #[test]
    fn tokenize_command_sub_nested() {
        // Outer body is `echo $(echo hi)`; inner is `echo hi`.
        let inner = echo_seq(&["hi"]);
        let inner_word = Word(vec![WordPart::CommandSub {
            sequence: inner,
            quoted: false,
        }]);
        let outer = {
            use crate::command::{Command, ExecCommand, Pipeline, Sequence, SimpleCommand};
            Sequence {
                first: Command::Pipeline(Pipeline {
                    commands: vec![Command::Simple(SimpleCommand::Exec(ExecCommand {
                        inline_assignments: Vec::new(),
                        program: Word(vec![WordPart::Literal { text: "echo".to_string(), quoted: false }]),
                        args: vec![inner_word],
                        stdin: None,
                        stdout: None,
                        stderr: None,
                    }))],
                }),
                rest: vec![],
                background: false,
            }
        };
        assert_eq!(
            tokenize("$(echo $(echo hi))").unwrap(),
            vec![sub_word(vec![WordPart::CommandSub {
                sequence: outer,
                quoted: false,
            }])]
        );
    }

    #[test]
    fn tokenize_command_sub_unterminated() {
        assert_eq!(
            tokenize("$(echo").unwrap_err(),
            LexError::UnterminatedSubstitution
        );
    }

    #[test]
    fn tokenize_command_sub_inner_lex_error() {
        // `${1foo}` inside a substitution → InvalidBraceModifier (v33:
        // digit branch routes through dispatch_braced_modifier; `f` is not
        // a valid modifier), wrapped in Substitution.
        let err = tokenize("$(echo ${1foo})").unwrap_err();
        match err {
            LexError::Substitution(inner) => {
                assert!(matches!(*inner, LexError::InvalidBraceModifier(_)));
            }
            other => panic!("expected Substitution, got {other:?}"),
        }
    }

    #[test]
    fn tokenize_command_sub_inner_parse_error() {
        // `echo |` inside the body → MissingCommand from the parser, wrapped.
        let err = tokenize("$(echo |)").unwrap_err();
        match err {
            LexError::SubstitutionParseError(inner) => {
                assert_eq!(inner, crate::command::ParseError::MissingCommand);
            }
            other => panic!("expected SubstitutionParseError, got {other:?}"),
        }
    }

    #[test]
    fn tokenize_command_sub_as_program() {
        // `$(echo ls) -la` — the program word is itself a CommandSub.
        let tokens = tokenize("$(echo ls) -la").unwrap();
        assert_eq!(tokens.len(), 2);
        match &tokens[0] {
            Token::Word(Word(parts)) => {
                assert!(matches!(&parts[0], WordPart::CommandSub { .. }));
            }
            other => panic!("expected Word, got {other:?}"),
        }
        assert_eq!(tokens[1], w("-la"));
    }

    #[test]
    fn tokenize_command_sub_concatenates_with_literal() {
        // `pre$(echo x)post` → one Word with three parts: Literal, CommandSub, Literal
        let tokens = tokenize("pre$(echo x)post").unwrap();
        assert_eq!(tokens.len(), 1);
        match &tokens[0] {
            Token::Word(Word(parts)) => {
                assert_eq!(parts.len(), 3);
                assert!(matches!(parts[0], WordPart::Literal { ref text, .. } if text == "pre"));
                assert!(matches!(parts[1], WordPart::CommandSub { .. }));
                assert!(matches!(parts[2], WordPart::Literal { ref text, .. } if text == "post"));
            }
            other => panic!("expected Word, got {other:?}"),
        }
    }

    #[test]
    fn tokenize_command_sub_in_redirect_target() {
        let tokens = tokenize("cat > $(echo /tmp/f)").unwrap();
        assert_eq!(tokens.len(), 3);
        assert_eq!(tokens[0], w("cat"));
        assert_eq!(tokens[1], Token::Op(Operator::RedirOut));
        match &tokens[2] {
            Token::Word(Word(parts)) => {
                assert!(matches!(&parts[0], WordPart::CommandSub { .. }));
            }
            other => panic!("expected Word, got {other:?}"),
        }
    }

    #[test]
    fn tokenize_backtick_basic() {
        assert_eq!(
            tokenize("`echo hi`").unwrap(),
            vec![sub_word(vec![WordPart::CommandSub {
                sequence: echo_seq(&["hi"]),
                quoted: false,
            }])]
        );
    }

    #[test]
    fn tokenize_backtick_in_double_quotes_is_quoted() {
        assert_eq!(
            tokenize("\"`echo hi`\"").unwrap(),
            vec![sub_word(vec![WordPart::CommandSub {
                sequence: echo_seq(&["hi"]),
                quoted: true,
            }])]
        );
    }

    #[test]
    fn tokenize_backtick_escape_dollar() {
        // `\$FOO` inside backticks → inner body is `$FOO` (the `\$` unescapes
        // before the inner tokenizer sees it). So the inner Sequence has a
        // single command whose first arg expands $FOO.
        let tokens = tokenize("`echo \\$FOO`").unwrap();
        assert_eq!(tokens.len(), 1);
        match &tokens[0] {
            Token::Word(Word(parts)) => {
                assert_eq!(parts.len(), 1);
                match &parts[0] {
                    WordPart::CommandSub { sequence, quoted: false } => {
                        // Inner: echo $FOO → second word's first part is a Var
                        let crate::command::Command::Pipeline(inner_pipeline) = &sequence.first
                        else {
                            panic!("expected a pipeline");
                        };
                        let inner_cmd = &inner_pipeline.commands[0];
                        match inner_cmd {
                            crate::command::Command::Simple(crate::command::SimpleCommand::Exec(e)) => {
                                assert_eq!(e.args.len(), 1);
                                match &e.args[0].0[0] {
                                    WordPart::Var { name, quoted: false } => {
                                        assert_eq!(name, "FOO");
                                    }
                                    other => panic!("expected Var(FOO), got {other:?}"),
                                }
                            }
                            other => panic!("expected Simple(Exec), got {other:?}"),
                        }
                    }
                    other => panic!("expected CommandSub, got {other:?}"),
                }
            }
            other => panic!("expected Word, got {other:?}"),
        }
    }

    #[test]
    fn tokenize_backtick_escape_backslash() {
        // `\\` inside backticks → inner body is `\`. Inner tokenize sees
        // a trailing backslash; treats it as a literal.
        let tokens = tokenize("`echo \\\\`").unwrap();
        match &tokens[0] {
            Token::Word(Word(parts)) => match &parts[0] {
                WordPart::CommandSub { sequence, .. } => {
                    let crate::command::Command::Pipeline(inner_pipeline) = &sequence.first
                    else {
                        panic!("expected a pipeline");
                    };
                    match &inner_pipeline.commands[0] {
                        crate::command::Command::Simple(crate::command::SimpleCommand::Exec(e)) => {
                            // Inner body was `echo \` — backslash at end is literal.
                            assert_eq!(e.args.len(), 1);
                            match &e.args[0].0[0] {
                                WordPart::Literal { text, .. } => assert_eq!(text, "\\"),
                                other => panic!("expected Literal(\\\\), got {other:?}"),
                            }
                        }
                        other => panic!("expected Simple(Exec), got {other:?}"),
                    }
                }
                other => panic!("expected CommandSub, got {other:?}"),
            },
            other => panic!("expected Word, got {other:?}"),
        }
    }

    #[test]
    fn tokenize_backtick_unescaped_other_backslash_preserved() {
        // `\n` inside backticks → body has `\n` (backslash + n), which the
        // inner tokenize treats as an escape (literal `n`).
        let tokens = tokenize("`echo \\n`").unwrap();
        match &tokens[0] {
            Token::Word(Word(parts)) => match &parts[0] {
                WordPart::CommandSub { sequence, .. } => {
                    let crate::command::Command::Pipeline(inner_pipeline) = &sequence.first
                    else {
                        panic!("expected a pipeline");
                    };
                    match &inner_pipeline.commands[0] {
                        crate::command::Command::Simple(crate::command::SimpleCommand::Exec(e)) => {
                            // Inner body `echo \n` — outer tokenizer's `\n` becomes `n`
                            assert_eq!(e.args.len(), 1);
                            match &e.args[0].0[0] {
                                WordPart::Literal { text, .. } => assert_eq!(text, "n"),
                                other => panic!("expected Literal(n), got {other:?}"),
                            }
                        }
                        other => panic!("expected Simple(Exec), got {other:?}"),
                    }
                }
                other => panic!("expected CommandSub, got {other:?}"),
            },
            other => panic!("expected Word, got {other:?}"),
        }
    }

    #[test]
    fn tokenize_backtick_unterminated() {
        assert_eq!(
            tokenize("`echo hi").unwrap_err(),
            LexError::UnterminatedSubstitution
        );
    }

    #[test]
    fn tokenize_backtick_in_single_quotes_is_literal() {
        assert_eq!(
            tokenize("'`echo hi`'").unwrap(),
            vec![wq("`echo hi`")]
        );
    }

    #[test]
    fn tokenize_tilde_plus_alone() {
        assert_eq!(
            tokenize("~+").unwrap(),
            vec![Token::Word(Word(vec![WordPart::Tilde(TildeSpec::Pwd)]))]
        );
    }

    #[test]
    fn tokenize_tilde_minus_alone() {
        assert_eq!(
            tokenize("~-").unwrap(),
            vec![Token::Word(Word(vec![WordPart::Tilde(TildeSpec::OldPwd)]))]
        );
    }

    #[test]
    fn tokenize_tilde_plus_slash_path() {
        assert_eq!(
            tokenize("~+/x").unwrap(),
            vec![Token::Word(Word(vec![
                WordPart::Tilde(TildeSpec::Pwd),
                WordPart::Literal { text: "/x".to_string(), quoted: false },
            ]))]
        );
    }

    #[test]
    fn tokenize_tilde_minus_slash_path() {
        assert_eq!(
            tokenize("~-/x").unwrap(),
            vec![Token::Word(Word(vec![
                WordPart::Tilde(TildeSpec::OldPwd),
                WordPart::Literal { text: "/x".to_string(), quoted: false },
            ]))]
        );
    }

    #[test]
    fn tokenize_tilde_plus_followed_by_letter_is_literal() {
        // ~+abc is not a valid form; falls back to literal.
        assert_eq!(tokenize("~+abc").unwrap(), words(&["~+abc"]));
    }

    #[test]
    fn tokenize_assignment_bare_tilde_after_equals() {
        // X=~  (just `=~` with no path after) — covers the end-of-input branch
        // of try_parse_tilde inside assignment context.
        assert_eq!(
            tokenize("X=~").unwrap(),
            vec![Token::Word(Word(vec![
                WordPart::Literal { text: "X=".to_string(), quoted: false },
                WordPart::Tilde(TildeSpec::Home),
            ]))]
        );
    }

    #[test]
    fn tokenize_assignment_value_expands_first_tilde_after_equals() {
        assert_eq!(
            tokenize("PATH=~/bin").unwrap(),
            vec![Token::Word(Word(vec![
                WordPart::Literal { text: "PATH=".to_string(), quoted: false },
                WordPart::Tilde(TildeSpec::Home),
                WordPart::Literal { text: "/bin".to_string(), quoted: false },
            ]))]
        );
    }

    #[test]
    fn tokenize_assignment_value_expands_each_tilde_after_colon() {
        assert_eq!(
            tokenize("PATH=~/bin:~/lib").unwrap(),
            vec![Token::Word(Word(vec![
                WordPart::Literal { text: "PATH=".to_string(), quoted: false },
                WordPart::Tilde(TildeSpec::Home),
                WordPart::Literal { text: "/bin:".to_string(), quoted: false },
                WordPart::Tilde(TildeSpec::Home),
                WordPart::Literal { text: "/lib".to_string(), quoted: false },
            ]))]
        );
    }

    #[test]
    fn tokenize_non_assignment_colon_tilde_stays_literal() {
        // `echo` is not an assignment, so `a:~/b` does NOT expand the tilde.
        assert_eq!(
            tokenize("echo a:~/b").unwrap(),
            words(&["echo", "a:~/b"])
        );
    }

    #[test]
    fn tokenize_assignment_with_digit_first_is_not_assignment_context() {
        // `1ABC=~/x` doesn't match identifier-start; treated as literal.
        assert_eq!(
            tokenize("1ABC=~/x").unwrap(),
            words(&["1ABC=~/x"])
        );
    }

    #[test]
    fn quoted_prefix_disqualifies_assignment() {
        // `"F"OO=bar` is a command argument, not an assignment, because the
        // identifier prefix contains quoted text.
        let tokens = tokenize("\"F\"OO=bar").unwrap();
        assert_eq!(tokens.len(), 1);
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        // Expect quoted "F", unquoted "OO=bar" — no assignment split.
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0], WordPart::Literal { text: "F".to_string(), quoted: true });
        assert_eq!(parts[1], WordPart::Literal { text: "OO=bar".to_string(), quoted: false });
    }

    #[test]
    fn tokenize_assignment_value_tilde_user() {
        assert_eq!(
            tokenize("HOMES=~alice:~bob").unwrap(),
            vec![Token::Word(Word(vec![
                WordPart::Literal { text: "HOMES=".to_string(), quoted: false },
                WordPart::Tilde(TildeSpec::User("alice".to_string())),
                WordPart::Literal { text: ":".to_string(), quoted: false },
                WordPart::Tilde(TildeSpec::User("bob".to_string())),
            ]))]
        );
    }

    #[test]
    fn tokenize_tilde_user_colon_outside_assignment_is_literal() {
        // Bash: ~alice:bob outside assignment is literal (no : terminator).
        assert_eq!(
            tokenize("echo ~alice:bob").unwrap(),
            words(&["echo", "~alice:bob"])
        );
    }

    #[test]
    fn tokenize_tilde_pwd_colon_outside_assignment_is_literal() {
        assert_eq!(
            tokenize("echo ~+:foo").unwrap(),
            words(&["echo", "~+:foo"])
        );
    }

    #[test]
    fn tokenize_mixed_quoted_unquoted_flushes_at_boundaries() {
        let tokens = tokenize("foo\"bar\"baz").unwrap();
        assert_eq!(tokens.len(), 1);
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0], WordPart::Literal { text: "foo".to_string(), quoted: false });
        assert_eq!(parts[1], WordPart::Literal { text: "bar".to_string(), quoted: true });
        assert_eq!(parts[2], WordPart::Literal { text: "baz".to_string(), quoted: false });
    }

    #[test]
    fn tokenize_arith_simple() {
        use crate::arith::ArithExpr;
        let tokens = tokenize("$((1+2))").unwrap();
        assert_eq!(tokens.len(), 1);
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        assert_eq!(parts.len(), 1);
        let WordPart::Arith { expr, quoted } = &parts[0] else {
            panic!("expected Arith part, got {:?}", parts[0])
        };
        assert!(!(*quoted));
        assert_eq!(*expr, ArithExpr::Add(
            Box::new(ArithExpr::Num(1)),
            Box::new(ArithExpr::Num(2)),
        ));
    }

    #[test]
    fn tokenize_arith_with_nested_parens() {
        use crate::arith::ArithExpr;
        let tokens = tokenize("$(( (1+2) * 3 ))").unwrap();
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        let WordPart::Arith { expr, .. } = &parts[0] else { panic!() };
        assert_eq!(*expr, ArithExpr::Mul(
            Box::new(ArithExpr::Add(
                Box::new(ArithExpr::Num(1)),
                Box::new(ArithExpr::Num(2)),
            )),
            Box::new(ArithExpr::Num(3)),
        ));
    }

    #[test]
    fn tokenize_arith_inside_double_quotes_is_quoted() {
        let tokens = tokenize("\"$((1+2))\"").unwrap();
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        let WordPart::Arith { quoted, .. } = &parts[0] else { panic!() };
        assert!(*quoted);
    }

    #[test]
    fn tokenize_arith_unterminated_returns_error() {
        let err = tokenize("$((1+2").unwrap_err();
        assert_eq!(err, LexError::UnterminatedArith);
    }

    #[test]
    fn tokenize_arith_parse_error_returns_arith_parse_err() {
        let err = tokenize("$((1+))").unwrap_err();
        assert!(matches!(err, LexError::ArithParse(_)), "got {:?}", err);
    }

    #[test]
    fn tokenize_arith_and_command_sub_both_recognized() {
        let tokens = tokenize("$((1)) $(echo x)").unwrap();
        let Token::Word(Word(parts1)) = &tokens[0] else { panic!() };
        assert!(matches!(parts1[0], WordPart::Arith { .. }));
        let Token::Word(Word(parts2)) = &tokens[1] else { panic!() };
        assert!(matches!(parts2[0], WordPart::CommandSub { .. }));
    }

    #[test]
    fn tokenize_arith_var_with_dollar_prefix_inside() {
        use crate::arith::ArithExpr;
        let tokens = tokenize("$(($x))").unwrap();
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        let WordPart::Arith { expr, .. } = &parts[0] else { panic!() };
        assert_eq!(*expr, ArithExpr::Var("x".to_string()));
    }

    #[test]
    fn tokenize_arith_back_to_back_in_same_word() {
        use crate::arith::ArithExpr;
        let tokens = tokenize("$((1+2))$((3+4))").unwrap();
        assert_eq!(tokens.len(), 1);
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        assert_eq!(parts.len(), 2);
        let WordPart::Arith { expr: e1, .. } = &parts[0] else { panic!() };
        let WordPart::Arith { expr: e2, .. } = &parts[1] else { panic!() };
        assert_eq!(*e1, ArithExpr::Add(Box::new(ArithExpr::Num(1)), Box::new(ArithExpr::Num(2))));
        assert_eq!(*e2, ArithExpr::Add(Box::new(ArithExpr::Num(3)), Box::new(ArithExpr::Num(4))));
    }

    #[test]
    fn tokenize_arith_close_paren_not_followed_by_close_paren_is_unterminated() {
        // After the inner `)` at depth 1, the next char must be `)` to close `))`.
        // If it's anything else, that's an unterminated arithmetic expansion.
        let err = tokenize("$((1)x)").unwrap_err();
        assert_eq!(err, LexError::UnterminatedArith);
    }

    #[test]
    fn scan_braced_operand_simple() {
        let mut chars = "foo}".chars().peekable();
        assert_eq!(scan_braced_operand(&mut chars).unwrap(), "foo");
    }

    #[test]
    fn scan_braced_operand_nested_braces() {
        let mut chars = "${Y}}".chars().peekable();
        assert_eq!(scan_braced_operand(&mut chars).unwrap(), "${Y}");
    }

    #[test]
    fn scan_braced_operand_double_quote_protects_brace() {
        let mut chars = "\"a}b\"c}".chars().peekable();
        assert_eq!(scan_braced_operand(&mut chars).unwrap(), "\"a}b\"c");
    }

    #[test]
    fn scan_braced_operand_single_quote_protects_brace() {
        let mut chars = "'a}b'c}".chars().peekable();
        assert_eq!(scan_braced_operand(&mut chars).unwrap(), "'a}b'c");
    }

    #[test]
    fn scan_braced_operand_unterminated_is_error() {
        let mut chars = "foo".chars().peekable();
        assert_eq!(scan_braced_operand(&mut chars).unwrap_err(), LexError::UnterminatedBrace);
    }

    #[test]
    fn parse_braced_operand_single_word() {
        let w = parse_braced_operand("foo").unwrap();
        assert_eq!(w.0.len(), 1);
        assert_eq!(w.0[0], WordPart::Literal { text: "foo".to_string(), quoted: false });
    }

    #[test]
    fn parse_braced_operand_two_words_join_with_space() {
        let w = parse_braced_operand("foo bar").unwrap();
        assert_eq!(w.0.len(), 3);
        assert_eq!(w.0[0], WordPart::Literal { text: "foo".to_string(), quoted: false });
        assert_eq!(w.0[1], WordPart::Literal { text: " ".to_string(), quoted: false });
        assert_eq!(w.0[2], WordPart::Literal { text: "bar".to_string(), quoted: false });
    }

    #[test]
    fn parse_braced_operand_top_level_pipe_is_error() {
        assert_eq!(parse_braced_operand("foo | bar").unwrap_err(), LexError::InvalidBraceOperand);
    }

    #[test]
    fn parse_braced_operand_empty_returns_empty_word() {
        let w = parse_braced_operand("").unwrap();
        assert_eq!(w.0.len(), 0);
    }

    #[test]
    fn tokenize_brace_var_no_modifier_still_emits_var() {
        let tokens = tokenize("${foo}").unwrap();
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0], WordPart::Var { name: "foo".to_string(), quoted: false });
    }

    #[test]
    fn tokenize_length_modifier() {
        let tokens = tokenize("${#foo}").unwrap();
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        assert_eq!(parts.len(), 1);
        let WordPart::ParamExpansion { name, modifier, quoted } = &parts[0] else {
            panic!("expected ParamExpansion, got {:?}", parts[0]);
        };
        assert_eq!(name, "foo");
        assert!(!(*quoted));
        assert!(matches!(modifier, ParamModifier::Length));
    }

    #[test]
    fn tokenize_length_modifier_digit_leading_name_errors() {
        // `${#1foo}` — v34: digit-only positional names are now supported
        // (${#1}, ${#10}), but ${#1foo} is still invalid: after parsing the
        // positional "1", the lexer expects "}" but finds "f", so
        // UnterminatedBrace.
        let err = tokenize("${#1foo}").unwrap_err();
        assert_eq!(err, LexError::UnterminatedBrace);
    }

    #[test]
    fn tokenize_use_default_colon_dash() {
        let tokens = tokenize("${X:-w}").unwrap();
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        let WordPart::ParamExpansion { name, modifier, .. } = &parts[0] else { panic!() };
        assert_eq!(name, "X");
        match modifier {
            ParamModifier::UseDefault { word, colon } => {
                assert!(*colon);
                assert_eq!(word.0, vec![WordPart::Literal { text: "w".to_string(), quoted: false }]);
            }
            other => panic!("expected UseDefault, got {:?}", other),
        }
    }

    #[test]
    fn tokenize_use_default_no_colon() {
        let tokens = tokenize("${X-w}").unwrap();
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        let WordPart::ParamExpansion { modifier, .. } = &parts[0] else { panic!() };
        assert!(matches!(modifier, ParamModifier::UseDefault { colon: false, .. }));
    }

    #[test]
    fn tokenize_assign_default_colon_equals() {
        let tokens = tokenize("${X:=w}").unwrap();
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        let WordPart::ParamExpansion { modifier, .. } = &parts[0] else { panic!() };
        assert!(matches!(modifier, ParamModifier::AssignDefault { colon: true, .. }));
    }

    #[test]
    fn tokenize_error_if_unset_colon_question() {
        let tokens = tokenize("${X:?msg}").unwrap();
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        let WordPart::ParamExpansion { modifier, .. } = &parts[0] else { panic!() };
        assert!(matches!(modifier, ParamModifier::ErrorIfUnset { colon: true, .. }));
    }

    #[test]
    fn tokenize_use_alternate_colon_plus() {
        let tokens = tokenize("${X:+w}").unwrap();
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        let WordPart::ParamExpansion { modifier, .. } = &parts[0] else { panic!() };
        assert!(matches!(modifier, ParamModifier::UseAlternate { colon: true, .. }));
    }

    #[test]
    fn tokenize_remove_prefix_short() {
        let tokens = tokenize("${X#pat}").unwrap();
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        let WordPart::ParamExpansion { modifier, .. } = &parts[0] else { panic!() };
        assert!(matches!(modifier, ParamModifier::RemovePrefix { longest: false, .. }));
    }

    #[test]
    fn tokenize_remove_prefix_long() {
        let tokens = tokenize("${X##pat}").unwrap();
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        let WordPart::ParamExpansion { modifier, .. } = &parts[0] else { panic!() };
        assert!(matches!(modifier, ParamModifier::RemovePrefix { longest: true, .. }));
    }

    #[test]
    fn tokenize_remove_suffix_short() {
        let tokens = tokenize("${X%pat}").unwrap();
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        let WordPart::ParamExpansion { modifier, .. } = &parts[0] else { panic!() };
        assert!(matches!(modifier, ParamModifier::RemoveSuffix { longest: false, .. }));
    }

    #[test]
    fn tokenize_remove_suffix_long() {
        let tokens = tokenize("${X%%pat}").unwrap();
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        let WordPart::ParamExpansion { modifier, .. } = &parts[0] else { panic!() };
        assert!(matches!(modifier, ParamModifier::RemoveSuffix { longest: true, .. }));
    }

    #[test]
    fn brace_substitute_first_match() {
        let mut t = tokenize_words("\"${name/foo/bar}\"").unwrap();
        let part = single_param_expansion(&mut t);
        match part {
            WordPart::ParamExpansion { name, modifier, quoted } => {
                assert_eq!(name, "name");
                assert!(quoted);
                match modifier {
                    ParamModifier::Substitute { pattern, replacement, anchor, all } => {
                        assert_eq!(word_to_literal(&pattern), "foo");
                        assert_eq!(word_to_literal(&replacement), "bar");
                        assert_eq!(anchor, SubstAnchor::None);
                        assert!(!all);
                    }
                    _ => panic!("expected Substitute"),
                }
            }
            _ => panic!("expected ParamExpansion"),
        }
    }

    #[test]
    fn brace_substitute_all_matches() {
        let mut t = tokenize_words("${name//foo/bar}").unwrap();
        let part = single_param_expansion(&mut t);
        if let WordPart::ParamExpansion { modifier: ParamModifier::Substitute { all, anchor, .. }, .. } = part {
            assert!(all);
            assert_eq!(anchor, SubstAnchor::None);
        } else { panic!("expected Substitute") }
    }

    #[test]
    fn brace_substitute_anchored_prefix() {
        let mut t = tokenize_words("${name/#foo/bar}").unwrap();
        let part = single_param_expansion(&mut t);
        if let WordPart::ParamExpansion { modifier: ParamModifier::Substitute { anchor, all, .. }, .. } = part {
            assert_eq!(anchor, SubstAnchor::Prefix);
            assert!(!all);
        } else { panic!("expected Substitute") }
    }

    #[test]
    fn brace_substitute_anchored_suffix() {
        let mut t = tokenize_words("${name/%foo/bar}").unwrap();
        let part = single_param_expansion(&mut t);
        if let WordPart::ParamExpansion { modifier: ParamModifier::Substitute { anchor, all, .. }, .. } = part {
            assert_eq!(anchor, SubstAnchor::Suffix);
            assert!(!all);
        } else { panic!("expected Substitute") }
    }

    #[test]
    fn brace_substitute_missing_replacement_is_empty_word() {
        let mut t = tokenize_words("${name/foo}").unwrap();
        let part = single_param_expansion(&mut t);
        if let WordPart::ParamExpansion { modifier: ParamModifier::Substitute { pattern, replacement, .. }, .. } = part {
            assert_eq!(word_to_literal(&pattern), "foo");
            assert_eq!(word_to_literal(&replacement), "");
        } else { panic!("expected Substitute") }
    }

    #[test]
    fn brace_substitute_escaped_slash_in_pattern() {
        let mut t = tokenize_words("${path//\\//-}").unwrap();
        let part = single_param_expansion(&mut t);
        if let WordPart::ParamExpansion { modifier: ParamModifier::Substitute { pattern, replacement, all, .. }, .. } = part {
            assert_eq!(word_to_literal(&pattern), "/");
            assert_eq!(word_to_literal(&replacement), "-");
            assert!(all);
        } else { panic!("expected Substitute") }
    }

    #[test]
    fn brace_substitute_unterminated_is_error() {
        assert!(matches!(
            tokenize_words("${name/foo/bar"),
            Err(LexError::UnterminatedBrace)
        ));
    }

    #[test]
    fn brace_substitute_nested_braced_var_in_pattern() {
        // `${path/${HOME}/X}` — the inner `${HOME}`'s closing `}` must not
        // terminate the outer substitution; the depth-aware splitter must
        // pick the `/` between the closing `}` and `X` as the delimiter.
        let mut t = tokenize_words("${path/${HOME}/X}").unwrap();
        let part = single_param_expansion(&mut t);
        if let WordPart::ParamExpansion { modifier: ParamModifier::Substitute { pattern, replacement, .. }, .. } = part {
            let Word(pat_parts) = &pattern;
            assert!(
                pat_parts.iter().any(|p| matches!(p, WordPart::Var { name, .. } if name == "HOME")),
                "expected Var(HOME) in pattern, got {pat_parts:?}",
            );
            assert_eq!(word_to_literal(&replacement), "X");
        } else { panic!("expected Substitute") }
    }

    #[test]
    fn brace_substitute_nested_braced_var_in_replacement() {
        // `${name/foo/${REPL}}` — the inner `${REPL}` must be parsed as a
        // nested expansion in the replacement half.
        let mut t = tokenize_words("${name/foo/${REPL}}").unwrap();
        let part = single_param_expansion(&mut t);
        if let WordPart::ParamExpansion { modifier: ParamModifier::Substitute { pattern, replacement, .. }, .. } = part {
            assert_eq!(word_to_literal(&pattern), "foo");
            let Word(repl_parts) = &replacement;
            assert!(
                repl_parts.iter().any(|p| matches!(p, WordPart::Var { name, .. } if name == "REPL")),
                "expected Var(REPL) in replacement, got {repl_parts:?}",
            );
        } else { panic!("expected Substitute") }
    }

    #[test]
    fn tokenize_nested_param_expansion_in_operand() {
        let tokens = tokenize("${X:-${Y}}").unwrap();
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        let WordPart::ParamExpansion { modifier, .. } = &parts[0] else { panic!() };
        if let ParamModifier::UseDefault { word, .. } = modifier {
            assert_eq!(word.0.len(), 1);
            assert!(matches!(word.0[0], WordPart::Var { .. }));
        } else {
            panic!("expected UseDefault");
        }
    }

    #[test]
    fn tokenize_quoted_operand_preserves_spaces() {
        let tokens = tokenize("${X:-\"a b\"}").unwrap();
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        let WordPart::ParamExpansion { modifier, .. } = &parts[0] else { panic!() };
        if let ParamModifier::UseDefault { word, .. } = modifier {
            assert_eq!(word.0.len(), 1);
            assert_eq!(word.0[0], WordPart::Literal { text: "a b".to_string(), quoted: true });
        } else {
            panic!();
        }
    }

    #[test]
    fn tokenize_quoted_outer_param_expansion() {
        let tokens = tokenize("\"${X:-w}\"").unwrap();
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        let WordPart::ParamExpansion { quoted, .. } = &parts[0] else { panic!() };
        assert!(*quoted);
    }

    #[test]
    fn tokenize_invalid_modifier_errors() {
        // ${X:&Y}: `:` followed by `&` — `&` is not `-=?+` so falls through
        // to substring dispatch; `&` inside the brace operand is an operator
        // → InvalidBraceOperand (v33: replaced by substring fall-through).
        let err = tokenize("${X:&Y}").unwrap_err();
        assert!(matches!(err, LexError::InvalidBraceOperand | LexError::Substitution(_)));
    }

    #[test]
    fn tokenize_empty_param_name_errors() {
        let err = tokenize("${:-foo}").unwrap_err();
        assert_eq!(err, LexError::EmptyParamName);
    }

    #[test]
    fn tokenize_unterminated_brace_modifier_errors() {
        let err = tokenize("${X:-foo").unwrap_err();
        assert_eq!(err, LexError::UnterminatedBrace);
    }

    #[test]
    fn tokenize_pipe_in_operand_errors() {
        let err = tokenize("${X:-foo | bar}").unwrap_err();
        assert_eq!(err, LexError::InvalidBraceOperand);
    }

    #[test]
    fn newline_outside_quotes_emits_newline_token() {
        let tokens = tokenize("a\nb").unwrap();
        assert_eq!(
            tokens,
            vec![
                Token::Word(Word(vec![WordPart::Literal { text: "a".to_string(), quoted: false }])),
                Token::Newline,
                Token::Word(Word(vec![WordPart::Literal { text: "b".to_string(), quoted: false }])),
            ]
        );
    }

    #[test]
    fn newline_inside_double_quotes_stays_literal() {
        let tokens = tokenize("\"a\nb\"").unwrap();
        assert_eq!(
            tokens,
            vec![Token::Word(Word(vec![WordPart::Literal {
                text: "a\nb".to_string(),
                quoted: true,
            }]))]
        );
    }

    #[test]
    fn newline_inside_single_quotes_stays_literal() {
        let tokens = tokenize("'a\nb'").unwrap();
        assert_eq!(
            tokens,
            vec![Token::Word(Word(vec![WordPart::Literal {
                text: "a\nb".to_string(),
                quoted: true,
            }]))]
        );
    }

    #[test]
    fn consecutive_newlines_emit_consecutive_tokens() {
        let tokens = tokenize("a\n\nb").unwrap();
        assert_eq!(
            tokens,
            vec![
                Token::Word(Word(vec![WordPart::Literal { text: "a".to_string(), quoted: false }])),
                Token::Newline,
                Token::Newline,
                Token::Word(Word(vec![WordPart::Literal { text: "b".to_string(), quoted: false }])),
            ]
        );
    }

    #[test]
    fn carriage_return_is_still_plain_whitespace() {
        // `\r` separates words but does not emit a Newline token.
        let tokens = tokenize("a\rb").unwrap();
        assert_eq!(
            tokens,
            vec![
                Token::Word(Word(vec![WordPart::Literal { text: "a".to_string(), quoted: false }])),
                Token::Word(Word(vec![WordPart::Literal { text: "b".to_string(), quoted: false }])),
            ]
        );
    }

    #[test]
    fn tokenize_open_paren() {
        assert_eq!(tokenize("(").unwrap(), vec![Token::Op(Operator::LParen)]);
    }

    #[test]
    fn tokenize_close_paren() {
        assert_eq!(tokenize(")").unwrap(), vec![Token::Op(Operator::RParen)]);
    }

    #[test]
    fn tokenize_double_semi() {
        assert_eq!(tokenize(";;").unwrap(), vec![Token::Op(Operator::DoubleSemi)]);
    }

    #[test]
    fn tokenize_semi_amp() {
        assert_eq!(tokenize(";&").unwrap(), vec![Token::Op(Operator::SemiAmp)]);
    }

    #[test]
    fn tokenize_double_semi_amp() {
        assert_eq!(tokenize(";;&").unwrap(), vec![Token::Op(Operator::DoubleSemiAmp)]);
    }

    #[test]
    fn tokenize_double_semi_space_amp_is_two_tokens() {
        assert_eq!(
            tokenize(";; &").unwrap(),
            vec![Token::Op(Operator::DoubleSemi), Token::Op(Operator::Background)]
        );
    }

    #[test]
    fn tokenize_lone_semi_still_semi() {
        assert_eq!(
            tokenize("a;b").unwrap(),
            vec![w("a"), Token::Op(Operator::Semi), w("b")]
        );
    }

    #[test]
    fn tokenize_paren_splits_adjacent_word() {
        assert_eq!(
            tokenize("a)").unwrap(),
            vec![w("a"), Token::Op(Operator::RParen)]
        );
    }

    #[test]
    fn tokenize_quoted_paren_stays_literal() {
        // A quoted `)` is ordinary word content, not an operator.
        assert_eq!(tokenize("')'").unwrap(), vec![wq(")")]);
    }

    // ---- Positional parameter lexer tests (v22 Task 4) ----------------------

    #[test]
    fn tokenize_dollar_digit() {
        let tokens = tokenize("$1").unwrap();
        assert_eq!(
            tokens,
            vec![Token::Word(Word(vec![WordPart::Var {
                name: "1".to_string(), quoted: false
            }]))]
        );
    }

    #[test]
    fn tokenize_dollar_hash() {
        let tokens = tokenize("$#").unwrap();
        assert_eq!(
            tokens,
            vec![Token::Word(Word(vec![WordPart::Var {
                name: "#".to_string(), quoted: false
            }]))]
        );
    }

    #[test]
    fn tokenize_dollar_at() {
        let tokens = tokenize("$@").unwrap();
        assert_eq!(
            tokens,
            vec![Token::Word(Word(vec![WordPart::AllArgs {
                joined: false, quoted: false
            }]))]
        );
    }

    #[test]
    fn tokenize_dollar_star() {
        let tokens = tokenize("$*").unwrap();
        assert_eq!(
            tokens,
            vec![Token::Word(Word(vec![WordPart::AllArgs {
                joined: true, quoted: false
            }]))]
        );
    }

    #[test]
    fn tokenize_quoted_dollar_at() {
        let tokens = tokenize("\"$@\"").unwrap();
        assert_eq!(
            tokens,
            vec![Token::Word(Word(vec![WordPart::AllArgs {
                joined: false, quoted: true
            }]))]
        );
    }

    #[test]
    fn tokenize_braced_positional() {
        let tokens = tokenize("${10}").unwrap();
        assert_eq!(
            tokens,
            vec![Token::Word(Word(vec![WordPart::Var {
                name: "10".to_string(), quoted: false
            }]))]
        );
    }

    #[test]
    fn tokenize_braced_special_at() {
        let tokens = tokenize("${@}").unwrap();
        assert_eq!(
            tokens,
            vec![Token::Word(Word(vec![WordPart::AllArgs {
                joined: false, quoted: false
            }]))]
        );
    }

    // --- Here-document tests (v24) ---

    /// Helper: extract the body Word from the first Token::Heredoc in tokens.
    fn heredoc_body(tokens: &[Token]) -> &Word {
        for tok in tokens {
            if let Token::Heredoc { body, .. } = tok {
                return body;
            }
        }
        panic!("no Token::Heredoc found in tokens: {tokens:?}");
    }

    /// Helper: assert a Literal part matches expected text and quoted flag.
    fn assert_literal(part: &WordPart, expected_text: &str, expected_quoted: bool) {
        match part {
            WordPart::Literal { text, quoted } => {
                assert_eq!(text, expected_text, "literal text mismatch");
                assert_eq!(quoted, &expected_quoted, "literal quoted flag mismatch");
            }
            other => panic!("expected Literal, got {other:?}"),
        }
    }

    #[test]
    fn tokenize_heredoc_op_recognized() {
        // Verify <<EOF lexes and produces a Token::Heredoc with body.
        let result = tokenize("cat <<EOF\nhello\nEOF\n");
        let tokens = result.expect("parse ok");
        assert_eq!(tokens.len(), 3, "got: {tokens:?}"); // Word("cat"), Heredoc{...}, Newline
        assert!(matches!(tokens[0], Token::Word(_)));
        assert!(matches!(tokens[1], Token::Heredoc { .. }));
        assert!(matches!(tokens[2], Token::Newline));
    }

    #[test]
    fn tokenize_heredoc_simple_expand() {
        // cat <<EOF\nhello\nEOF → Token::Heredoc{body=Word[Literal{"hello"}, Literal{"\n"}],
        //                                         expand:true, strip_tabs:false}
        let tokens = tokenize("cat <<EOF\nhello\nEOF\n").unwrap();
        let body = heredoc_body(&tokens);
        // For an expanding heredoc, "hello" is a Literal{quoted:false} and "\n" is Literal{quoted:true}
        assert_eq!(body.0.len(), 2);
        assert_literal(&body.0[0], "hello", false);
        assert_literal(&body.0[1], "\n", true);
        if let Token::Heredoc { expand, strip_tabs, .. } = &tokens[1] {
            assert!(expand, "should be expanding");
            assert!(!strip_tabs, "should not strip tabs");
        }
    }

    #[test]
    fn tokenize_heredoc_literal_no_expand() {
        // cat <<'EOF'\n$HOME\nEOF → body is one Literal{quoted:true, text:"$HOME\n"}
        let tokens = tokenize("cat <<'EOF'\n$HOME\nEOF\n").unwrap();
        if let Token::Heredoc { body, expand, strip_tabs } = &tokens[1] {
            assert!(!expand, "quoted delim → literal mode (no expand)");
            assert!(!strip_tabs);
            // Literal mode: entire body as one quoted Literal per line, plus newline parts.
            assert_eq!(body.0.len(), 2);
            assert_literal(&body.0[0], "$HOME", true);
            assert_literal(&body.0[1], "\n", true);
        } else {
            panic!("expected Token::Heredoc, got {:?}", tokens[1]);
        }
    }

    #[test]
    fn tokenize_heredoc_strip_tabs_dash() {
        // <<-EOF\n\t\thello\n\tEOF → body "hello\n" (tabs stripped from body AND close line)
        let tokens = tokenize("<<-EOF\n\t\thello\n\tEOF\n").unwrap();
        if let Token::Heredoc { body, expand, strip_tabs } = &tokens[0] {
            assert!(strip_tabs, "<<- should strip tabs");
            assert!(expand);
            // After tab stripping, body line is "hello"
            assert_eq!(body.0.len(), 2);
            assert_literal(&body.0[0], "hello", false);
            assert_literal(&body.0[1], "\n", true);
        } else {
            panic!("expected Token::Heredoc");
        }
    }

    #[test]
    fn tokenize_heredoc_strip_tabs_with_literal_delim() {
        // <<-'EOF' composes strip + no-expansion
        let tokens = tokenize("cat <<-'EOF'\n\thello\n\tEOF\n").unwrap();
        if let Token::Heredoc { body, expand, strip_tabs } = &tokens[1] {
            assert!(strip_tabs, "<<- should strip tabs");
            assert!(!expand, "quoted delim → literal mode");
            assert_eq!(body.0.len(), 2);
            assert_literal(&body.0[0], "hello", true);
            assert_literal(&body.0[1], "\n", true);
        } else {
            panic!("expected Token::Heredoc");
        }
    }

    #[test]
    fn tokenize_heredoc_unclosed_errors() {
        // cat <<EOF\nhello → LexError::UnterminatedHeredoc
        let result = tokenize("cat <<EOF\nhello");
        assert_eq!(result, Err(LexError::UnterminatedHeredoc));
    }

    #[test]
    fn tokenize_heredoc_close_must_match_exactly() {
        // Trailing space on close line → unterminated
        let result = tokenize("cat <<EOF\nhello\nEOF \n");
        assert_eq!(result, Err(LexError::UnterminatedHeredoc));
    }

    #[test]
    fn tokenize_heredoc_close_must_not_have_leading_spaces() {
        // Leading spaces without <<- → unterminated
        let result = tokenize("cat <<EOF\nhello\n  EOF\n");
        assert_eq!(result, Err(LexError::UnterminatedHeredoc));
    }

    #[test]
    fn tokenize_heredoc_multiple_in_order() {
        // cmd <<A <<B\nbody_a\nA\nbody_b\nB
        let tokens = tokenize("cmd <<A <<B\nbody_a\nA\nbody_b\nB\n").unwrap();
        // tokens: Word("cmd"), Heredoc{A's body}, Heredoc{B's body}, Newline
        assert_eq!(tokens.len(), 4, "got: {tokens:?}");
        assert!(matches!(tokens[0], Token::Word(_)));
        assert!(matches!(tokens[3], Token::Newline));
        if let Token::Heredoc { body: body_a, .. } = &tokens[1] {
            assert_eq!(body_a.0.len(), 2);
            assert_literal(&body_a.0[0], "body_a", false);
            assert_literal(&body_a.0[1], "\n", true);
        } else {
            panic!("tokens[1] should be Token::Heredoc for A");
        }
        if let Token::Heredoc { body: body_b, .. } = &tokens[2] {
            assert_eq!(body_b.0.len(), 2);
            assert_literal(&body_b.0[0], "body_b", false);
            assert_literal(&body_b.0[1], "\n", true);
        } else {
            panic!("tokens[2] should be Token::Heredoc for B");
        }
    }

    #[test]
    fn tokenize_heredoc_body_var_part() {
        // cat <<EOF\n$USER\nEOF → body has Var{name:"USER"} part
        let tokens = tokenize("cat <<EOF\n$USER\nEOF\n").unwrap();
        let body = heredoc_body(&tokens);
        // Parts: Var{USER, quoted:true}, Literal{"\n", quoted:true}
        assert_eq!(body.0.len(), 2);
        match &body.0[0] {
            WordPart::Var { name, quoted } => {
                assert_eq!(name, "USER");
                assert!(quoted, "heredoc body vars are quoted-context");
            }
            other => panic!("expected Var, got {other:?}"),
        }
        assert_literal(&body.0[1], "\n", true);
    }

    #[test]
    fn tokenize_heredoc_body_command_sub() {
        // cat <<EOF\n$(date)\nEOF → body has CommandSub part
        let tokens = tokenize("cat <<EOF\n$(date)\nEOF\n").unwrap();
        let body = heredoc_body(&tokens);
        // Parts: CommandSub{..., quoted:true}, Literal{"\n", quoted:true}
        assert_eq!(body.0.len(), 2);
        assert!(
            matches!(body.0[0], WordPart::CommandSub { quoted: true, .. }),
            "expected CommandSub{{quoted:true}}, got {:?}", body.0[0]
        );
        assert_literal(&body.0[1], "\n", true);
    }

    #[test]
    fn tokenize_heredoc_body_escape_dollar() {
        // cat <<EOF\n\$LITERAL\nEOF → body has Literal "$LITERAL"
        // The backslash escapes the $ — result is literal text "$" followed by "LITERAL"
        let tokens = tokenize("cat <<EOF\n\\$LITERAL\nEOF\n").unwrap();
        let body = heredoc_body(&tokens);
        // \$ → Literal{"$", quoted:true}, then "LITERAL" → Literal{"LITERAL", quoted:false}
        assert!(body.0.len() >= 2, "expected at least 2 parts, got {:?}", body.0);
        // First part should be the escaped '$' as a quoted Literal
        assert_literal(&body.0[0], "$", true);
        // Second part should be the remaining text "LITERAL" (unquoted)
        assert_literal(&body.0[1], "LITERAL", false);
    }

    #[test]
    fn tokenize_heredoc_body_backslash_passthrough() {
        // cat <<EOF\n\d\nEOF → body has Literal "\\d" (POSIX: \X other than \$\`\\ is literal)
        let tokens = tokenize("cat <<EOF\n\\d\nEOF\n").unwrap();
        let body = heredoc_body(&tokens);
        // \d → kept as literal "\d" (backslash not special before 'd')
        assert_eq!(body.0.len(), 2);
        assert_literal(&body.0[0], "\\d", false);
        assert_literal(&body.0[1], "\n", true);
    }

    #[test]
    fn tokenize_heredoc_empty_body() {
        // cat <<EOF\nEOF → body Word has zero parts (empty)
        let tokens = tokenize("cat <<EOF\nEOF\n").unwrap();
        let body = heredoc_body(&tokens);
        assert_eq!(body.0.len(), 0, "empty body should have no parts, got {:?}", body.0);
    }

    #[test]
    fn tokenize_heredoc_delim_partially_quoted_is_literal_mode() {
        // cat <<E"O"F\n$X\nEOF → expand:false, delim:"EOF"
        let tokens = tokenize("cat <<E\"O\"F\n$X\nEOF\n").unwrap();
        if let Token::Heredoc { body, expand, .. } = &tokens[1] {
            assert!(!expand, "partial quoting triggers literal mode");
            // Literal body: "$X" as-is
            assert_eq!(body.0.len(), 2);
            assert_literal(&body.0[0], "$X", true);
            assert_literal(&body.0[1], "\n", true);
        } else {
            panic!("expected Token::Heredoc");
        }
    }

    #[test]
    fn tokenize_heredoc_delim_backslash_escaped_is_literal_mode() {
        // cat <<\EOF\n$X\nEOF → expand:false (backslash-escaped delim = literal mode)
        let tokens = tokenize("cat <<\\EOF\n$X\nEOF\n").unwrap();
        if let Token::Heredoc { body, expand, .. } = &tokens[1] {
            assert!(!expand, "backslash-escaped delim triggers literal mode");
            assert_eq!(body.0.len(), 2);
            assert_literal(&body.0[0], "$X", true);
            assert_literal(&body.0[1], "\n", true);
        } else {
            panic!("expected Token::Heredoc");
        }
    }

    #[test]
    fn tokenize_heredoc_expanding_backslash_newline_joins_lines() {
        // POSIX 2.7.4: \<NL> inside expanding heredoc is line continuation.
        let tokens = tokenize("cat <<EOF\nhello \\\nworld\nEOF\n").unwrap();
        // Find the Heredoc token and verify body literal is "hello world\n".
        let body_text: String = match &tokens[1] {
            Token::Heredoc { body, .. } => body.0.iter()
                .filter_map(|p| match p {
                    WordPart::Literal { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect(),
            _ => panic!("expected Heredoc at index 1, got {:?}", tokens[1]),
        };
        assert_eq!(body_text, "hello world\n");
    }

    #[test]
    fn tokenize_heredoc_literal_backslash_newline_is_literal() {
        // Inside literal heredoc, \<NL> is two literal chars (POSIX 2.2.2 / 2.7.4).
        let tokens = tokenize("cat <<'EOF'\nhello \\\nworld\nEOF\n").unwrap();
        let body_text: String = match &tokens[1] {
            Token::Heredoc { body, .. } => body.0.iter()
                .filter_map(|p| match p {
                    WordPart::Literal { text, .. } => Some(text.clone()),
                    _ => None,
                })
                .collect(),
            _ => panic!(),
        };
        // Body contains literal "hello \\\nworld\n" — backslash + newline + world.
        assert_eq!(body_text, "hello \\\nworld\n");
    }

    #[test]
    fn lexer_dollar_dollar_emits_var_name_dollar() {
        let tokens = tokenize("$$").unwrap();
        assert_eq!(tokens.len(), 1);
        let Token::Word(Word(parts)) = &tokens[0] else { panic!("expected Word, got {:?}", tokens[0]) };
        assert_eq!(parts.len(), 1);
        assert!(
            matches!(&parts[0], WordPart::Var { name, quoted: false } if name == "$"),
            "got {:?}", parts[0]
        );
    }

    #[test]
    fn lexer_dollar_bang_emits_var_name_bang() {
        let tokens = tokenize("$!").unwrap();
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        assert!(
            matches!(&parts[0], WordPart::Var { name, quoted: false } if name == "!"),
            "got {:?}", parts[0]
        );
    }

    #[test]
    fn lexer_dollar_zero_already_emits_var_name_zero() {
        // Regression test: $0 was lexed by the existing digit path pre-v26;
        // confirm it still produces Var { name: "0" }.
        let tokens = tokenize("$0").unwrap();
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        assert!(matches!(&parts[0], WordPart::Var { name, .. } if name == "0"));
    }

    #[test]
    fn lexer_dollar_dollar_inside_double_quotes() {
        let tokens = tokenize("\"$$\"").unwrap();
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        assert!(matches!(&parts[0], WordPart::Var { name, quoted: true } if name == "$"));
    }

    #[test]
    fn lexer_dollar_bang_inside_double_quotes() {
        let tokens = tokenize("\"$!\"").unwrap();
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        assert!(matches!(&parts[0], WordPart::Var { name, quoted: true } if name == "!"));
    }

    #[test]
    fn lexer_dollar_dollar_concatenates_with_literal() {
        let tokens = tokenize("pre-$$-post").unwrap();
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        assert_eq!(parts.len(), 3);
        assert!(matches!(&parts[0], WordPart::Literal { text, .. } if text == "pre-"));
        assert!(matches!(&parts[1], WordPart::Var { name, .. } if name == "$"));
        assert!(matches!(&parts[2], WordPart::Literal { text, .. } if text == "-post"));
    }

    // ---- v27 here-string lexer tests -------------------------------------------

    #[test]
    fn tokenize_here_string_op_alone() {
        let tokens = tokenize("<<<").unwrap();
        assert_eq!(tokens, vec![Token::Op(Operator::HereString)]);
    }

    #[test]
    fn tokenize_here_string_with_unquoted_word() {
        let tokens = tokenize("cat <<< hello").unwrap();
        assert_eq!(tokens.len(), 3);
        assert!(matches!(tokens[0], Token::Word(_)));
        assert!(matches!(tokens[1], Token::Op(Operator::HereString)));
        assert!(matches!(tokens[2], Token::Word(_)));
    }

    #[test]
    fn tokenize_here_string_with_quoted_word() {
        let tokens = tokenize("cat <<< \"hi there\"").unwrap();
        let Token::Word(Word(parts)) = &tokens[2] else { panic!("got {:?}", tokens[2]) };
        assert!(matches!(&parts[0], WordPart::Literal { text, quoted: true } if text == "hi there"));
    }

    #[test]
    fn tokenize_here_string_with_var_in_body() {
        let tokens = tokenize("cat <<< $FOO").unwrap();
        let Token::Word(Word(parts)) = &tokens[2] else { panic!() };
        assert!(matches!(&parts[0], WordPart::Var { name, .. } if name == "FOO"));
    }

    #[test]
    fn tokenize_here_string_with_command_sub_in_body() {
        let tokens = tokenize("cat <<< $(echo hi)").unwrap();
        let Token::Word(Word(parts)) = &tokens[2] else { panic!() };
        assert!(matches!(&parts[0], WordPart::CommandSub { .. }));
    }

    #[test]
    fn tokenize_double_less_still_heredoc() {
        // Regression: `<<EOF` must still lex as Heredoc, not split into `<<` + `<EOF`.
        let tokens = tokenize("cat <<EOF\nbody\nEOF\n").unwrap();
        assert!(tokens.iter().any(|t| matches!(t, Token::Heredoc { .. })),
            "expected Heredoc token, got {:?}", tokens);
    }

    #[test]
    fn tokenize_dup_out_basic() {
        let tokens = tokenize(">&").unwrap();
        assert_eq!(tokens, vec![Token::Op(Operator::DupOut)]);
    }

    #[test]
    fn tokenize_dup_err_basic() {
        let tokens = tokenize("2>&").unwrap();
        assert_eq!(tokens, vec![Token::Op(Operator::DupErr)]);
    }

    #[test]
    fn tokenize_and_redir_out() {
        let tokens = tokenize("&>").unwrap();
        assert_eq!(tokens, vec![Token::Op(Operator::AndRedirOut)]);
    }

    #[test]
    fn tokenize_and_redir_append() {
        let tokens = tokenize("&>>").unwrap();
        assert_eq!(tokens, vec![Token::Op(Operator::AndRedirAppend)]);
    }

    #[test]
    fn tokenize_dup_in_context() {
        let tokens = tokenize("cmd 2>&1").unwrap();
        assert_eq!(tokens.len(), 3);
        assert!(matches!(tokens[0], Token::Word(_)));
        assert!(matches!(tokens[1], Token::Op(Operator::DupErr)));
        assert!(matches!(tokens[2], Token::Word(_)));
    }

    #[test]
    fn tokenize_redir_out_regression() {
        assert_eq!(tokenize(">").unwrap(), vec![Token::Op(Operator::RedirOut)]);
        assert_eq!(tokenize(">>").unwrap(), vec![Token::Op(Operator::RedirAppend)]);
    }

    #[test]
    fn tokenize_redir_err_regression() {
        assert_eq!(tokenize("2>").unwrap(), vec![Token::Op(Operator::RedirErr)]);
        assert_eq!(tokenize("2>>").unwrap(), vec![Token::Op(Operator::RedirErrAppend)]);
    }

    #[test]
    fn tokenize_explicit_fd1_redir_out() {
        // `1>` lexes as RedirOut (same as `>`).
        let tokens = tokenize("1>").unwrap();
        assert_eq!(tokens, vec![Token::Op(Operator::RedirOut)]);
    }

    #[test]
    fn tokenize_explicit_fd1_redir_append() {
        let tokens = tokenize("1>>").unwrap();
        assert_eq!(tokens, vec![Token::Op(Operator::RedirAppend)]);
    }

    #[test]
    fn tokenize_explicit_fd1_dup() {
        let tokens = tokenize("1>&").unwrap();
        assert_eq!(tokens, vec![Token::Op(Operator::DupOut)]);
    }

    #[test]
    fn tokenize_one_as_arg_when_has_token() {
        // `cmd 1` where 1 is an argument — should NOT trigger the new arm.
        let tokens = tokenize("cmd 1").unwrap();
        assert_eq!(tokens.len(), 2);
        assert!(matches!(tokens[0], Token::Word(_)));
        assert!(matches!(tokens[1], Token::Word(_)));
    }

    #[test]
    fn tokenize_background_regression() {
        assert_eq!(tokenize("&").unwrap(), vec![Token::Op(Operator::Background)]);
        assert_eq!(tokenize("&&").unwrap(), vec![Token::Op(Operator::And)]);
    }

    // ──────────────────────────────────────────────────────────────
    // [[ ]] keyword recognition tests (v30)
    // ──────────────────────────────────────────────────────────────

    #[test]
    fn tokenize_double_bracket_open_at_word_start() {
        // `[[` at command-start → single Word token containing the literal `[[`.
        // The keyword is recognised by the *parser* (command.rs `keyword_of`),
        // not the lexer, so the lexer emits an ordinary Word.
        let tokens = tokenize("[[").unwrap();
        assert_eq!(tokens.len(), 1, "expected 1 token, got {:?}", tokens);
        assert!(
            matches!(&tokens[0], Token::Word(Word(parts))
                if parts.len() == 1
                && matches!(&parts[0], WordPart::Literal { text, quoted: false } if text == "[[")
            ),
            "expected Word([[), got {:?}", tokens[0]
        );
    }

    #[test]
    fn tokenize_double_bracket_close() {
        // `]]` → Word token with literal `]]`.
        let tokens = tokenize("]]").unwrap();
        assert_eq!(tokens.len(), 1, "expected 1 token, got {:?}", tokens);
        assert!(
            matches!(&tokens[0], Token::Word(Word(parts))
                if parts.len() == 1
                && matches!(&parts[0], WordPart::Literal { text, quoted: false } if text == "]]")
            ),
            "expected Word(]]), got {:?}", tokens[0]
        );
    }

    #[test]
    fn tokenize_double_bracket_not_at_word_start_is_literal() {
        // `cmd[[foo]]` — `[[` appears mid-word-sequence; because there is no
        // space before it the lexer folds everything into a single Word.
        // The important thing is that no separate keyword token is emitted.
        let tokens = tokenize("cmd[[foo]]").unwrap();
        // The whole thing is one word token (the lexer has no special-casing for [[ )].
        assert_eq!(tokens.len(), 1, "expected 1 word token, got {:?}", tokens);
        assert!(matches!(&tokens[0], Token::Word(_)), "expected Word, got {:?}", tokens[0]);
    }

    #[test]
    fn brace_substring_simple() {
        let mut t = tokenize_words("${name:1}").unwrap();
        let part = single_param_expansion(&mut t);
        if let WordPart::ParamExpansion { name, modifier: ParamModifier::Substring { offset, length }, quoted } = part {
            assert_eq!(name, "name");
            assert!(!quoted);
            assert_eq!(word_to_literal(&offset), "1");
            assert!(length.is_none());
        } else { panic!("expected Substring") }
    }

    #[test]
    fn brace_substring_with_length() {
        let mut t = tokenize_words("${name:1:3}").unwrap();
        let part = single_param_expansion(&mut t);
        if let WordPart::ParamExpansion { modifier: ParamModifier::Substring { offset, length }, .. } = part {
            assert_eq!(word_to_literal(&offset), "1");
            assert_eq!(word_to_literal(&length.expect("length")), "3");
        } else { panic!("expected Substring") }
    }

    #[test]
    fn brace_substring_negative_offset_with_space() {
        // `${name: -3}` — the space disambiguates from `:-` (UseDefault).
        let mut t = tokenize_words("${name: -3}").unwrap();
        let part = single_param_expansion(&mut t);
        if let WordPart::ParamExpansion { modifier: ParamModifier::Substring { offset, .. }, .. } = part {
            assert_eq!(word_to_literal(&offset), "-3");
        } else { panic!("expected Substring, got {part:?}") }
    }

    #[test]
    fn brace_substring_no_space_is_use_default_regression() {
        // `${name:-3}` — no space, so this MUST remain UseDefault with default "3".
        let mut t = tokenize_words("${name:-3}").unwrap();
        let part = single_param_expansion(&mut t);
        assert!(
            matches!(part, WordPart::ParamExpansion { modifier: ParamModifier::UseDefault { colon: true, .. }, .. }),
            "expected UseDefault, got {part:?}",
        );
    }

    #[test]
    fn brace_substring_positional() {
        // `${1:0:3}` — must emit ParamExpansion (not Var) so the modifier runs.
        let mut t = tokenize_words("${1:0:3}").unwrap();
        let part = single_param_expansion(&mut t);
        if let WordPart::ParamExpansion { name, modifier: ParamModifier::Substring { offset, length }, .. } = part {
            assert_eq!(name, "1");
            assert_eq!(word_to_literal(&offset), "0");
            assert_eq!(word_to_literal(&length.expect("length")), "3");
        } else { panic!("expected Substring on positional, got {part:?}") }
    }

    #[test]
    fn brace_substring_nested_braced_var_in_operand() {
        // The depth-aware split must not break on the inner `${start}`'s `}`.
        let mut t = tokenize_words("${name:${start}:${len}}").unwrap();
        let part = single_param_expansion(&mut t);
        if let WordPart::ParamExpansion { modifier: ParamModifier::Substring { offset, length }, .. } = part {
            // Offset word should contain a Var part for `start`.
            let Word(off_parts) = &offset;
            assert!(
                off_parts.iter().any(|p| matches!(p, WordPart::Var { name, .. } if name == "start")),
                "expected Var(start) in offset, got {off_parts:?}",
            );
            // Length word should contain a Var part for `len`.
            let Word(len_parts) = length.as_ref().expect("length");
            assert!(
                len_parts.iter().any(|p| matches!(p, WordPart::Var { name, .. } if name == "len")),
                "expected Var(len) in length, got {len_parts:?}",
            );
        } else { panic!("expected Substring") }
    }

    #[test]
    fn brace_substring_unterminated_is_error() {
        assert!(matches!(
            tokenize_words("${name:1:3"),
            Err(LexError::UnterminatedBrace)
        ));
    }

    #[test]
    fn brace_substring_empty_operand_is_lex_error() {
        // `${var:}` — colon followed immediately by close brace has no
        // semantic meaning; reject at lex time rather than letting a
        // confusing arithmetic error fire later.
        assert!(matches!(
            tokenize_words("${name:}"),
            Err(LexError::InvalidBraceModifier(s)) if s == ":"
        ));
    }

    #[test]
    fn brace_length_positional() {
        let mut t = tokenize_words("${#1}").unwrap();
        let part = single_param_expansion(&mut t);
        if let WordPart::ParamExpansion { name, modifier, quoted } = part {
            assert_eq!(name, "1");
            assert!(!quoted);
            assert!(matches!(modifier, ParamModifier::Length));
        } else { panic!("expected ParamExpansion, got {part:?}") }
    }

    #[test]
    fn brace_length_multi_digit_positional() {
        let mut t = tokenize_words("${#10}").unwrap();
        let part = single_param_expansion(&mut t);
        if let WordPart::ParamExpansion { name, modifier, .. } = part {
            assert_eq!(name, "10");
            assert!(matches!(modifier, ParamModifier::Length));
        } else { panic!("expected ParamExpansion") }
    }

    #[test]
    fn brace_length_at() {
        let mut t = tokenize_words("${#@}").unwrap();
        let part = single_param_expansion(&mut t);
        if let WordPart::ParamExpansion { name, modifier, .. } = part {
            assert_eq!(name, "@");
            assert!(matches!(modifier, ParamModifier::Length));
        } else { panic!("expected ParamExpansion") }
    }

    #[test]
    fn brace_length_star() {
        let mut t = tokenize_words("${#*}").unwrap();
        let part = single_param_expansion(&mut t);
        if let WordPart::ParamExpansion { name, modifier, .. } = part {
            assert_eq!(name, "*");
            assert!(matches!(modifier, ParamModifier::Length));
        } else { panic!("expected ParamExpansion") }
    }

    #[test]
    fn brace_length_unchanged_for_named() {
        // Regression: `${#foo}` still parses as Length on a named var.
        let mut t = tokenize_words("${#foo}").unwrap();
        let part = single_param_expansion(&mut t);
        if let WordPart::ParamExpansion { name, modifier, .. } = part {
            assert_eq!(name, "foo");
            assert!(matches!(modifier, ParamModifier::Length));
        } else { panic!("expected ParamExpansion") }
    }

    #[test]
    fn brace_length_bare_hash_unchanged() {
        // Regression: `${#}` still parses as Var { name: "#" }.
        let mut t = tokenize_words("${#}").unwrap();
        let part = single_param_expansion(&mut t);
        if let WordPart::Var { name, .. } = part {
            assert_eq!(name, "#");
        } else { panic!("expected Var(#), got {part:?}") }
    }
}
