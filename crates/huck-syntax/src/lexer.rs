#[derive(Debug, PartialEq, Eq, Clone)]
#[non_exhaustive]
pub enum LexError {
    UnterminatedQuote,
    InvalidVarName,
    UnterminatedBrace,
    UnterminatedSubstitution,
    UnterminatedArith,
    InvalidBraceModifier(String),
    EmptyParamName,
    Substitution(Box<LexError>),
    SubstitutionParseError(crate::command::ParseError),
    UnterminatedHeredoc,
    AnsiCInvalidCodepoint(u32),
    BraceExpansionLimit,
    /// `${a[3` or `a[3=v` — missing `]` closing a subscript.
    UnterminatedSubscript,
    /// `a=(x y` — missing `)` closing a compound array RHS.
    UnterminatedArrayLiteral,
    /// `a=([3] x)` — `[3]` not followed by `=`.
    ArrayLiteralMissingEquals,
    /// `$[ 1+2` — EOF before the `]` closing a legacy `$[ … ]` arithmetic
    /// expansion (bash's deprecated synonym for `$(( … ))`).
    UnterminatedLegacyArith,
    /// `((1+2` — EOF before matching `))`.
    UnterminatedArithBlock,
    /// `+(a|b` — EOF before the closing `)` of an extglob group (only
    /// reachable when `LexerOptions::extglob` is set).
    UnterminatedExtglob,
}

impl std::fmt::Display for LexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&crate::errors::lex_error_message_impl(self))
    }
}

impl std::error::Error for LexError {}

/// A char cursor over a `&str` that also tracks the byte offset and 1-based
/// line number of the next char to be produced. Drop-in for the
/// `Peekable<Chars>` the lexer used: implements `Iterator<Item = char>`, a
/// `peek()` returning `Option<&char>`, `Clone`, and (via `Iterator`) `by_ref()`.
/// `offset()` is the byte position of the char that the next `next()`/`peek()`
/// will yield (or `s.len()` at end). `line()` is the 1-based line of that same
/// char — it advances to the next line immediately after a `'\n'` is consumed,
/// exactly mirroring how `offset()` advances after each byte.
#[derive(Clone)]
pub struct CharCursor<'a> {
    s: &'a str,
    pos: usize,
    line: u32,
    peeked: Option<char>,
    peeked_len: usize,
}

impl<'a> CharCursor<'a> {
    pub fn new(s: &'a str) -> Self {
        CharCursor { s, pos: 0, line: 1, peeked: None, peeked_len: 0 }
    }

    /// Peek the next char without consuming it.
    pub fn peek(&mut self) -> Option<&char> {
        if self.peeked.is_none()
            && let Some(c) = self.s[self.pos..].chars().next()
        {
            self.peeked = Some(c);
            self.peeked_len = c.len_utf8();
        }
        self.peeked.as_ref()
    }

    /// Byte offset of the next char to be produced (start of the next token
    /// when the cursor sits on a token boundary). Equals `s.len()` at EOF.
    pub fn offset(&self) -> usize {
        self.pos
    }

    /// 1-based line of the next char to be produced (mirrors `offset()`).
    /// After consuming a `'\n'`, this reflects the NEXT line.
    pub fn line(&self) -> u32 {
        self.line
    }

    /// Byte slice of the source from `start` to the current offset. Used to
    /// reconstruct the raw `${…}` text for a deferred bad-substitution.
    pub fn slice_from(&self, start: usize) -> &str {
        &self.s[start..self.pos]
    }
}

impl Iterator for CharCursor<'_> {
    type Item = char;
    fn next(&mut self) -> Option<char> {
        if let Some(c) = self.peeked.take() {
            self.pos += self.peeked_len;
            self.peeked_len = 0;
            if c == '\n' { self.line += 1; }
            Some(c)
        } else if let Some(c) = self.s[self.pos..].chars().next() {
            self.pos += c.len_utf8();
            if c == '\n' { self.line += 1; }
            Some(c)
        } else {
            None
        }
    }
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
    RedirClobber,   // >|
    RedirErrClobber, // 2>|
    DupIn,          // <&
    RedirReadWrite, // <>
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseDirection {
    Upper,  // ^ / ^^
    Lower,  // , / ,,
}

/// Scalar and whole-array `${var@OP}` transform operators (bash 5.x).
/// Per-element across arrays via the per-element arm in expand.rs;
/// whole-array via the sibling whole-array arm; scalar via the
/// param_expansion path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TransformOp {
    PromptExpand, // @P — prompt-string expansion of the value
    Quote,        // @Q — shell-quote so the result re-reads as the same value
    Upper,        // @U — uppercase all
    Lower,        // @L — lowercase all
    UpperFirst,   // @u — uppercase first char
    EscapeExpand, // @E — expand backslash escapes ($'...' style)
    AssignDecl,   // @A — declare-style assignment string
    KvString,     // @K — k/v pairs as one quoted-internally string
    KvWords,      // @k — k/v pairs as word list
    AttrFlags,    // @a — attribute flag letters
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ParamModifier {
    /// No scalar-style modifier — bare `${a[i]}`, `${a[@]}`, `${a[*]}`.
    /// Expansion path treats this as a pure lookup (and, for subscripted
    /// forms, defers to the `subscript` field on `ParamExpansion`).
    /// Distinct from `UseDefault { word: empty }`, which would silently
    /// substitute "" on unset and trigger unwanted modifier semantics.
    None,
    Length,
    /// `${!name[@]}` / `${!name[*]}` — list of subscripts present in
    /// the named indexed array (bash's "array keys" form). For v71
    /// the bare indirect form `${!name}` is not yet supported; the
    /// lexer only emits this on a subscripted reference.
    IndirectKeys,
    /// `${!prefix*}` / `${!prefix@}` — expand to the sorted NAMES of set
    /// shell variables whose name starts with `prefix`. `at=false` (`*`)
    /// joins like `$*`; `at=true` (`@`) yields separate words like `$@`.
    /// The `name` field holds the prefix.
    PrefixNames { at: bool },
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
    Case {
        direction: CaseDirection,
        all: bool,
        pattern: Option<Word>,
    },
    /// `${var@OP}` scalar transform (`@P`/`@Q`/`@U`/`@L`/`@u`/`@E`).
    Transform { op: TransformOp },
    /// A `${…}` whose content is lexable (matching `}` found) but
    /// semantically invalid (bad modifier, special char as name, etc.).
    /// Parses successfully and defers to a RUNTIME "bad substitution"
    /// error, matching bash. `raw` is the literal `${…}` source for the
    /// message. Evaluated lazily — only errors when actually expanded.
    BadSubst { raw: String },
}

/// Subscript form attached to `${a[…]}` / `${a[@]}` / `${a[*]}`.
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum SubscriptKind {
    /// `${a[@]}` — produces a word list, one element per array entry,
    /// no IFS splitting when quoted.
    All,
    /// `${a[*]}` — joined-by-IFS scalar when quoted; word-split when not.
    Star,
    /// `${a[expr]}` — `expr` arith-evaluates to a usize subscript.
    Index(Word),
}

/// One element inside a compound array RHS `name=(elem [idx]=elem …)`.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ArrayLiteralElement {
    /// `Some(word)` for explicit `[expr]=value`; `None` for positional.
    pub subscript: Option<Word>,
    pub value: Word,
}

/// Direction of a process substitution: `<(cmd)` reads from the process,
/// `>(cmd)` writes to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcDir {
    /// `<(cmd)` — the command's stdout is available as a `/dev/fd/N` path
    /// that the surrounding command can open for reading.
    In,
    /// `>(cmd)` — the command's stdin is available as a `/dev/fd/N` path
    /// that the surrounding command can open for writing.
    Out,
}

/// The original source quoting style of a `WordPart::Quoted` run, preserved
/// so `declare -f` / `type` reconstruction reproduces bash's exact bytes.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum QuoteStyle {
    Single,    // '…'
    Double,    // "…"
    AnsiC,     // $'…'
    Backslash, // \c
}

#[derive(Debug, PartialEq, Eq, Clone)]
#[non_exhaustive]
pub enum WordPart {
    Literal { text: String, quoted: bool },
    Tilde(TildeSpec),
    Var { name: String, quoted: bool },
    LastStatus { quoted: bool },
    CommandSub { sequence: crate::command::Sequence, quoted: bool },
    /// Process substitution `<(cmd)` / `>(cmd)`. Only produced when UNQUOTED
    /// (inside double/single quotes `<(`/`>(` are literal). Expands to a
    /// `/dev/fd/N` (or FIFO) path at runtime.
    ProcessSub { sequence: crate::command::Sequence, dir: ProcDir },
    Arith { body: Word, quoted: bool },
    ParamExpansion {
        name: String,
        modifier: ParamModifier,
        quoted: bool,
        /// `Some(...)` for `${a[i]}` / `${a[@]}` / `${a[*]}`;
        /// `None` for `${a}`.
        subscript: Option<SubscriptKind>,
        /// `${!name}` indirect expansion: resolve `name`'s value to an
        /// effective name, then expand THAT (with `modifier`). v95.
        /// Distinct from `ParamModifier::IndirectKeys` (`${!a[@]}` array
        /// keys), which keeps `indirect: false`.
        indirect: bool,
    },
    /// `$@` (joined=false) or `$*` (joined=true). `quoted` reflects whether
    /// this was inside double quotes.
    AllArgs { quoted: bool, joined: bool },
    /// Synthetic prefix marker emitted by the lexer at the head of an
    /// assignment Word whose LHS isn't expressible as a leading
    /// `Literal { text: "name=" }`. Specifically: `name+=…`,
    /// `name[expr]=…`, and `name[expr]+=…`. The parser
    /// (`try_split_assignment`) consumes this prefix to produce an
    /// `Assignment` with the parsed target + append flag; the remaining
    /// parts form the value.
    AssignPrefix {
        target: crate::command::AssignTarget,
        append: bool,
    },
    /// Compound array RHS `(elem elem [idx]=elem …)`. Only appears
    /// as the sole trailing `WordPart` in a `Word` used as the RHS of
    /// an array-assignment in `SimpleCommand::Assign` / inline prefix.
    ArrayLiteral(Vec<ArrayLiteralElement>),
    /// One contiguous quoted run, preserving source `style` and span. Inner
    /// `parts` keep their own `quoted: true` flag so the expansion path is
    /// unchanged; the wrapper exists for reconstruction in `declare -f` /
    /// `type`.
    Quoted { style: QuoteStyle, parts: Vec<WordPart> },
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Word(pub Vec<WordPart>);

#[derive(Debug, PartialEq, Eq, Clone)]
#[non_exhaustive]
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
    /// Raw text inside a `(( ... ))` block (the outer `((` and `))`
    /// already consumed). Parsed by `crate::arith::parse` downstream.
    /// Captured verbatim including embedded `;` separators.
    ArithBlock(String, LexerOptions),
    /// An explicit fd-prefix glued to a following redirect operator:
    /// `3>` → `RedirFd(Number(3))` then `RedirOut`; `{fd}>` →
    /// `RedirFd(Var("fd"))` then `RedirOut`. Emitted by the lexer only
    /// when a digit-run or `{ident}` Word immediately precedes (no
    /// whitespace) a redirect operator (or heredoc).
    RedirFd(crate::command::RedirFd),
}

/// State for a heredoc whose body hasn't been collected yet.
struct PendingHeredoc {
    delim: String,
    expand: bool,
    strip_tabs: bool,
    /// Index into `tokens` of the `Token::Heredoc` placeholder to patch.
    token_idx: usize,
}

/// Lexer feature toggles resolved from shell state at tokenize time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LexerOptions {
    pub extglob: bool,
}

/// Raw output of the partial tokenizer: `tokens`, parallel byte `offsets` and
/// 1-based `lines` (with a trailing sentinel), and an optional trailing lex
/// error + the byte offset where lexing failed.
type PartialTokens = (Vec<Token>, Vec<usize>, Vec<u32>, Option<(LexError, usize)>);
/// Successful tokenization with positions: `tokens` plus parallel `offsets` and
/// `lines`. See [`tokenize_with_offsets`].
type TokensWithPos = (Vec<Token>, Vec<usize>, Vec<u32>);

/// Back-compat entry: lexes with all options off (current behavior).
pub fn tokenize(input: &str) -> Result<Vec<Token>, LexError> {
    tokenize_with_opts(input, LexerOptions::default())
}

/// Tokenize, returning each token's start byte offset (and a trailing sentinel
/// `offsets[tokens.len()] == input.len()`), and the per-token 1-based source
/// line numbers (same length as `offsets`). On error, returns the LexError and
/// the byte offset where lexing failed.
// Retained as a thin `Result`-returning wrapper over `tokenize_partial` and
// exercised by the offset unit tests; the source reader now calls
// `tokenize_partial` directly, so it is otherwise unused in non-test builds.
#[cfg_attr(not(test), allow(dead_code))]
pub fn tokenize_with_offsets(
    input: &str,
    opts: LexerOptions,
) -> Result<TokensWithPos, (LexError, usize)> {
    match tokenize_partial(input, opts) {
        (tokens, offsets, lines, None) => Ok((tokens, offsets, lines)),
        (_, _, _, Some((e, off))) => Err((e, off)),
    }
}

pub fn tokenize_with_opts(input: &str, opts: LexerOptions) -> Result<Vec<Token>, LexError> {
    match tokenize_partial(input, opts) {
        (tokens, _offsets, _lines, None) => Ok(tokens),
        (_, _, _, Some((e, _off))) => Err(e),
    }
}

/// Like `tokenize_with_offsets`, but on a lex error returns the tokens produced
/// BEFORE the error plus `Some((error, byte_offset))`. On success the third
/// element is `None`. In BOTH cases `offsets.len() == tokens.len() + 1`: the
/// trailing offset is `input.len()` on success, or the error byte offset on
/// failure. This lets the source reader execute the complete units that lexed
/// before the failure and re-lex the truncated trailing unit.
/// Core tokenizer body. `brace_expand` controls whether unquoted braces are
/// expanded into multiple Words (`true` for normal command words) or left as
/// literal text in a single Word (`false` for array-literal elements, which are
/// brace-expanded later). See the `tokenize_partial` / `tokenize_no_brace`
/// wrappers.
/// If `token` is a single plain-literal Word holding a pure digit-run or a
/// `{ident}` form, return the corresponding `RedirFd`. Used to detect an
/// fd-prefix glued to a following redirect operator. Returns `None` for any
/// other shape (e.g. `file2`, `{}`, `{1bad}`), leaving the Word intact as a
/// normal argument.
fn fd_prefix_of(token: Option<&Token>) -> Option<crate::command::RedirFd> {
    let Some(Token::Word(w)) = token else { return None };
    let text = crate::command::word_literal_text(w)?;
    if !text.is_empty() && text.chars().all(|c| c.is_ascii_digit()) {
        text.parse::<u16>().ok().map(crate::command::RedirFd::Number)
    } else if let Some(inner) = text.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
        if !inner.is_empty()
            && inner.chars().next().map(|c| c.is_alphabetic() || c == '_').unwrap_or(false)
            && inner.chars().all(|c| c.is_alphanumeric() || c == '_')
        {
            Some(crate::command::RedirFd::Var(inner.to_string()))
        } else {
            None
        }
    } else {
        None
    }
}

fn tokenize_partial_inner(
    input: &str,
    opts: LexerOptions,
    brace_expand: bool,
) -> PartialTokens {
    let mut tokens: Vec<Token> = Vec::new();
    let mut offsets: Vec<usize> = Vec::new();
    let mut lines: Vec<u32> = Vec::new();
    // Lockstep push: always push offset and line together so they can never diverge.
    macro_rules! push_pos {
        ($off:expr, $ln:expr) => {{ offsets.push($off); lines.push($ln); }};
    }
    // When `$glued` (no whitespace between the just-flushed Word and the
    // redirect operator about to be pushed), and that trailing Word is a pure
    // digit-run or `{ident}`, replace it with a `Token::RedirFd` occupying the
    // same offset/line. Keeps `offsets`/`lines` parallel to `tokens`. Must be
    // invoked AFTER the preceding word has been flushed and BEFORE the operator
    // token is pushed.
    macro_rules! take_fd_prefix {
        ($glued:expr) => {{
            if $glued {
                if let Some(fd) = fd_prefix_of(tokens.last()) {
                    tokens.pop();
                    let off = offsets.pop().expect("offset parallel to token");
                    let ln = lines.pop().expect("line parallel to token");
                    tokens.push(Token::RedirFd(fd));
                    push_pos!(off, ln);
                }
            }
        }};
    }
    let mut parts: Vec<WordPart> = Vec::new();
    let mut current = String::new();
    let mut has_token = false;
    let mut token_start: usize = 0;
    let mut token_start_line: u32 = 1;
    let mut in_assignment_value = false;
    // `[[ … ]]` context tracking for the `=~` regex operand. `dbracket_depth`
    // counts open `[[`; `expect_regex` is armed right after an `=~` keyword
    // inside `[[ ]]` so the NEXT word is scanned as one literal regex Word.
    let mut dbracket_depth: u32 = 0;
    let mut expect_regex = false;
    let mut chars = CharCursor::new(input);
    let mut pending_heredocs: std::collections::VecDeque<PendingHeredoc> = std::collections::VecDeque::new();

    let result: Result<(), LexError> = (|| {
    loop {
        // `=~` regex operand inside `[[ … ]]`: once `expect_regex` is armed and
        // the next char is the operand's first (non-whitespace) char, scan the
        // whole operand as one literal regex Word. Whitespace between `=~` and
        // the operand falls through to the normal loop (which skips it and keeps
        // `expect_regex` set). Branching before `chars.next()` keeps the emitted
        // offset exactly at the operand's first byte.
        if expect_regex {
            if let Some(&ch) = chars.peek() {
                if ch.is_whitespace() {
                    // skip leading whitespace via the normal path below
                } else {
                    expect_regex = false;
                    // The operand's first byte. Push the Word directly (NOT via
                    // emit_word_with_braces) so no brace expansion applies.
                    let operand_start = chars.offset();
                    let operand_line = chars.line();
                    let parts = scan_regex_operand(&mut chars, opts)?;
                    tokens.push(Token::Word(Word(parts)));
                    push_pos!(operand_start, operand_line);
                    has_token = false;
                    continue;
                }
            } else {
                break;
            }
        }
        let c_off = chars.offset();
        let c_line = chars.line();
        let c = match chars.next() {
            Some(c) => c,
            None => break,
        };
        if c.is_whitespace() {
            if has_token {
                flush_literal(&mut parts, &mut current, false);
                debug_assert!(
                    !parts.is_empty(),
                    "lexer invariant: has_token was true but no parts were emitted"
                );
                let kw = single_unquoted_literal(&parts).map(str::to_owned);
                let n = emit_word_with_braces(&mut tokens, std::mem::take(&mut parts), brace_expand)?;
                for _ in 0..n { push_pos!(token_start, token_start_line); }
                match kw.as_deref() {
                    Some("[[") => dbracket_depth += 1,
                    Some("]]") => dbracket_depth = dbracket_depth.saturating_sub(1),
                    Some("=~") if dbracket_depth > 0 => expect_regex = true,
                    _ => {}
                }
                has_token = false;
                in_assignment_value = false;
            }
            if c == '\n' {
                // If there are pending heredocs, collect their bodies now
                // before emitting the Newline token.
                if !pending_heredocs.is_empty() {
                    collect_heredoc_bodies(&mut chars, &mut pending_heredocs, &mut tokens, opts)?;
                }
                tokens.push(Token::Newline);
                push_pos!(c_off, c_line);
            }
            continue;
        }

        // Record the start byte offset of a word as soon as its first char is
        // seen. When `has_token` is false at the top of an iteration, this char
        // is a candidate first char; operator arms (which leave `has_token`
        // false) simply overwrite `token_start` on the next iteration, while
        // word arms read the value captured at the word's true first char.
        if !has_token {
            token_start = c_off;
            token_start_line = c_line;
        }

        // extglob (`shopt -s extglob`): one of `? * + @ !` directly followed
        // by `(` introduces a balanced parenthesised group (`+(a|b)`), lexed
        // as a single literal word part. Checked before the normal
        // `?`/`*`/`(` handling so the group is recognized first. With extglob
        // off, this branch never fires and lexing is byte-identical.
        if opts.extglob && matches!(c, '?' | '*' | '+' | '@' | '!') && chars.peek() == Some(&'(') {
            has_token = true;
            flush_literal(&mut parts, &mut current, false);
            let group_parts = scan_extglob_group(c, &mut chars, opts)?;
            parts.extend(group_parts);
            continue;
        }

        match c {
            '\'' => {
                has_token = true;
                flush_literal(&mut parts, &mut current, false);
                let mut run: Vec<WordPart> = Vec::new();
                let mut buf = String::new();
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(ch) => buf.push(ch),
                        None => return Err(LexError::UnterminatedQuote),
                    }
                }
                // empty '' still yields one empty quoted Literal (empty-token contract)
                run.push(WordPart::Literal { text: buf, quoted: true });
                parts.push(WordPart::Quoted { style: QuoteStyle::Single, parts: run });
            }
            '"' => {
                has_token = true;
                flush_literal(&mut parts, &mut current, false);
                let mut run: Vec<WordPart> = Vec::new();
                let mut qbuf = String::new();
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            // POSIX: inside `"..."`, backslash is special only
                            // before `$`, `, `"`, `\`, and newline. For other
                            // characters, the backslash is retained literally.
                            Some(esc @ ('"' | '\\' | '$' | '`')) => qbuf.push(esc),
                            // POSIX 2.2.3: `\<NL>` inside double quotes is also
                            // line continuation — both characters deleted.
                            Some('\n') => {}
                            Some(other) => {
                                qbuf.push('\\');
                                qbuf.push(other);
                            }
                            None => return Err(LexError::UnterminatedQuote),
                        },
                        Some('$') => {
                            // Expansion inside double quotes (quoted: true).
                            flush_literal(&mut run, &mut qbuf, true);
                            scan_dollar_expansion(&mut chars, &mut run, true, opts)?;
                        }
                        Some('`') => {
                            // Backtick substitution inside double quotes (quoted: true).
                            flush_literal(&mut run, &mut qbuf, true);
                            let sequence = scan_backtick_substitution(&mut chars, opts)?;
                            run.push(WordPart::CommandSub { sequence, quoted: true });
                        }
                        Some(ch) => qbuf.push(ch),
                        None => return Err(LexError::UnterminatedQuote),
                    }
                }
                flush_literal(&mut run, &mut qbuf, true);
                if run.is_empty() {
                    // Empty `""` — preserve the empty-token contract by
                    // emitting an empty quoted Literal.
                    run.push(WordPart::Literal { text: String::new(), quoted: true });
                }
                parts.push(WordPart::Quoted { style: QuoteStyle::Double, parts: run });
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
                    // escaped char as a one-char quoted Literal wrapped in a
                    // Backslash run. This is what makes `\*` survive pathname
                    // expansion as a literal `*` (the `quoted` flag inhibits
                    // globbing) while recording the backslash quote style for
                    // byte-exact reconstruction.
                    has_token = true;
                    flush_literal(&mut parts, &mut current, false);
                    parts.push(WordPart::Quoted {
                        style: QuoteStyle::Backslash,
                        parts: vec![WordPart::Literal { text: ch.to_string(), quoted: true }],
                    });
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
                scan_dollar_expansion(&mut chars, &mut parts, false, opts)?;
            }
            '#' if !has_token => {
                // POSIX: an unquoted `#` that begins a word starts a comment to
                // end-of-line. `#` mid-word (has_token) falls through as literal.
                skip_line_comment(&mut chars);
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
                let sequence = scan_backtick_substitution(&mut chars, opts)?;
                parts.push(WordPart::CommandSub { sequence, quoted: false });
            }
            '|' => {
                if has_token {
                    flush_literal(&mut parts, &mut current, false);
                    let n = emit_word_with_braces(&mut tokens, std::mem::take(&mut parts), brace_expand)?;
                    for _ in 0..n { push_pos!(token_start, token_start_line); }
                    has_token = false;
                }
                if chars.peek() == Some(&'|') {
                    chars.next();
                    tokens.push(Token::Op(Operator::Or));
                    push_pos!(c_off, c_line);
                } else if chars.peek() == Some(&'&') {
                    // `|&` is bash shorthand for `2>&1 |`: merge the left command's
                    // stderr into the pipe, then pipe. Desugar at the token level so
                    // the existing pipeline/redirect machinery (incl. v176
                    // compound-stage redirects) handles it unchanged.
                    chars.next(); // consume the '&' of `|&`
                    tokens.push(Token::RedirFd(crate::command::RedirFd::Number(2)));
                    push_pos!(c_off, c_line);
                    tokens.push(Token::Op(Operator::DupOut));
                    push_pos!(c_off, c_line);
                    tokens.push(Token::Word(Word(vec![WordPart::Literal {
                        text: "1".to_string(),
                        quoted: false,
                    }])));
                    push_pos!(c_off, c_line);
                    tokens.push(Token::Op(Operator::Pipe));
                    push_pos!(c_off, c_line);
                } else {
                    tokens.push(Token::Op(Operator::Pipe));
                    push_pos!(c_off, c_line);
                }
                in_assignment_value = false;
            }
            '&' => {
                if has_token {
                    flush_literal(&mut parts, &mut current, false);
                    let n = emit_word_with_braces(&mut tokens, std::mem::take(&mut parts), brace_expand)?;
                    for _ in 0..n { push_pos!(token_start, token_start_line); }
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
                push_pos!(c_off, c_line);
                in_assignment_value = false;
            }
            ';' => {
                if has_token {
                    flush_literal(&mut parts, &mut current, false);
                    let n = emit_word_with_braces(&mut tokens, std::mem::take(&mut parts), brace_expand)?;
                    for _ in 0..n { push_pos!(token_start, token_start_line); }
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
                push_pos!(c_off, c_line);
                in_assignment_value = false;
            }
            '(' => {
                if has_token {
                    flush_literal(&mut parts, &mut current, false);
                    let n = emit_word_with_braces(&mut tokens, std::mem::take(&mut parts), brace_expand)?;
                    for _ in 0..n { push_pos!(token_start, token_start_line); }
                    has_token = false;
                }
                // Detect `((` (contiguous, no whitespace). The peek/next
                // sequence below consumes the second `(` only when present.
                // Whitespace between the two `(` is already consumed by the
                // outer loop's whitespace handling — so by the time we get
                // here, a second `(` means they were truly adjacent.
                if chars.peek() == Some(&'(') {
                    // `((` is an arithmetic command ONLY if a matching `))` is
                    // found; otherwise bash treats it as nested subshells `( (`.
                    // Save the cursor at the second `(`, try the arith block, and
                    // on failure rewind + emit a single LParen (the first `(`); the
                    // second `(` then re-lexes as another LParen. A `((` that DOES
                    // close as `))` but isn't valid arithmetic stays an ArithBlock
                    // → arith error at parse/eval, matching bash. Mirrors the v177
                    // `$((` disambiguation.
                    let saved = chars.clone();
                    chars.next(); // consume the second `(`
                    match scan_arith_block(&mut chars) {
                        Ok(body) => tokens.push(Token::ArithBlock(body, opts)),
                        Err(_) => {
                            chars = saved;
                            tokens.push(Token::Op(Operator::LParen));
                        }
                    }
                } else {
                    tokens.push(Token::Op(Operator::LParen));
                }
                push_pos!(c_off, c_line);
                in_assignment_value = false;
            }
            ')' => {
                if has_token {
                    flush_literal(&mut parts, &mut current, false);
                    let n = emit_word_with_braces(&mut tokens, std::mem::take(&mut parts), brace_expand)?;
                    for _ in 0..n { push_pos!(token_start, token_start_line); }
                    has_token = false;
                }
                tokens.push(Token::Op(Operator::RParen));
                push_pos!(c_off, c_line);
                in_assignment_value = false;
            }
            '<' => {
                // `glued` = a Word was being accumulated with no intervening
                // whitespace before this operator. Captured before the flush.
                let glued = has_token;
                if has_token {
                    flush_literal(&mut parts, &mut current, false);
                    let n = emit_word_with_braces(&mut tokens, std::mem::take(&mut parts), brace_expand)?;
                    for _ in 0..n { push_pos!(token_start, token_start_line); }
                    has_token = false;
                }
                if chars.peek() == Some(&'<') {
                    chars.next(); // consume second '<'
                    if chars.peek() == Some(&'<') {
                        chars.next(); // consume third '<' — here-string
                        take_fd_prefix!(glued);
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
                        // A glued fd-prefix (`3<<EOF`) becomes a RedirFd token
                        // before the heredoc placeholder.
                        take_fd_prefix!(glued);
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
                    push_pos!(c_off, c_line);
                    in_assignment_value = false;
                } else if chars.peek() == Some(&'(') {
                    // `<(cmd)` — process substitution. Consume the `(` and scan the
                    // inner command body exactly like `$(…)`. The result is a word
                    // part on the CURRENT word (not a standalone redirect operator).
                    chars.next(); // consume '('
                    let sequence = scan_paren_substitution(&mut chars, opts)?;
                    if !has_token {
                        token_start = c_off;
                        token_start_line = c_line;
                    }
                    has_token = true;
                    parts.push(WordPart::ProcessSub { sequence, dir: ProcDir::In });
                    in_assignment_value = false;
                } else if chars.peek() == Some(&'&') {
                    chars.next();
                    take_fd_prefix!(glued);
                    tokens.push(Token::Op(Operator::DupIn));
                    push_pos!(c_off, c_line);
                    in_assignment_value = false;
                } else if chars.peek() == Some(&'>') {
                    chars.next();
                    take_fd_prefix!(glued);
                    tokens.push(Token::Op(Operator::RedirReadWrite));
                    push_pos!(c_off, c_line);
                    in_assignment_value = false;
                } else {
                    take_fd_prefix!(glued);
                    tokens.push(Token::Op(Operator::RedirIn));
                    push_pos!(c_off, c_line);
                    in_assignment_value = false;
                }
            }
            '>' => {
                let glued = has_token;
                if has_token {
                    flush_literal(&mut parts, &mut current, false);
                    let n = emit_word_with_braces(&mut tokens, std::mem::take(&mut parts), brace_expand)?;
                    for _ in 0..n { push_pos!(token_start, token_start_line); }
                    has_token = false;
                }
                if chars.peek() == Some(&'>') {
                    chars.next();
                    take_fd_prefix!(glued);
                    tokens.push(Token::Op(Operator::RedirAppend));
                    push_pos!(c_off, c_line);
                    in_assignment_value = false;
                } else if chars.peek() == Some(&'&') {
                    chars.next();
                    take_fd_prefix!(glued);
                    tokens.push(Token::Op(Operator::DupOut));
                    push_pos!(c_off, c_line);
                    in_assignment_value = false;
                } else if chars.peek() == Some(&'|') {
                    chars.next();
                    take_fd_prefix!(glued);
                    tokens.push(Token::Op(Operator::RedirClobber));
                    push_pos!(c_off, c_line);
                    in_assignment_value = false;
                } else if chars.peek() == Some(&'(') {
                    // `>(cmd)` — process substitution. Consume the `(` and scan the
                    // inner command body exactly like `$(…)`. The result is a word
                    // part on the CURRENT word (not a standalone redirect operator).
                    chars.next(); // consume '('
                    let sequence = scan_paren_substitution(&mut chars, opts)?;
                    if !has_token {
                        token_start = c_off;
                        token_start_line = c_line;
                    }
                    has_token = true;
                    parts.push(WordPart::ProcessSub { sequence, dir: ProcDir::Out });
                    in_assignment_value = false;
                } else {
                    take_fd_prefix!(glued);
                    tokens.push(Token::Op(Operator::RedirOut));
                    push_pos!(c_off, c_line);
                    in_assignment_value = false;
                }
            }
            '1' if !has_token && chars.peek() == Some(&'>') => {
                chars.next();
                if chars.peek() == Some(&'>') {
                    chars.next();
                    tokens.push(Token::Op(Operator::RedirAppend));
                } else if chars.peek() == Some(&'&') {
                    chars.next();
                    tokens.push(Token::Op(Operator::DupOut));
                } else if chars.peek() == Some(&'|') {
                    chars.next();
                    tokens.push(Token::Op(Operator::RedirClobber));
                } else {
                    tokens.push(Token::Op(Operator::RedirOut));
                }
                push_pos!(c_off, c_line);
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
                } else if chars.peek() == Some(&'|') {
                    chars.next();
                    tokens.push(Token::Op(Operator::RedirErrClobber));
                } else {
                    tokens.push(Token::Op(Operator::RedirErr));
                }
                push_pos!(c_off, c_line);
                in_assignment_value = false;
            }
            '=' if !in_assignment_value && word_is_identifier_so_far(&current, &parts) => {
                in_assignment_value = true;
                has_token = true;
                current.push('=');
                // Compound RHS: `name=(...)`. Scan the array literal as
                // a single WordPart that becomes the value.
                // A `\<NL>` line continuation may sit between `=` and the array
                // `(` (`arr=\<NL>(…)`); bash deletes it pre-tokenization.
                skip_line_continuations(&mut chars);
                if chars.peek() == Some(&'(') {
                    chars.next(); // consume '('
                    flush_literal(&mut parts, &mut current, false);
                    let elements = scan_array_literal(&mut chars, opts)?;
                    parts.push(WordPart::ArrayLiteral(elements));
                }
            }
            // `+=`: scalar-or-array append assignment when the prefix is
            // identifier-shaped. Emits an AssignPrefix(Bare, append=true)
            // prefix Word.
            '+' if !in_assignment_value
                && chars.peek() == Some(&'=')
                && word_is_identifier_so_far(&current, &parts) =>
            {
                chars.next(); // consume '='
                in_assignment_value = true;
                has_token = true;
                // Bake the accumulated identifier text into the target.
                let name = std::mem::take(&mut current);
                debug_assert!(
                    parts.is_empty(),
                    "word_is_identifier_so_far guarantees no prior parts"
                );
                parts.push(WordPart::AssignPrefix {
                    target: crate::command::AssignTarget::Bare(name),
                    append: true,
                });
                // Compound RHS: `name+=(...)`.
                skip_line_continuations(&mut chars);
                if chars.peek() == Some(&'(') {
                    chars.next();
                    let elements = scan_array_literal(&mut chars, opts)?;
                    parts.push(WordPart::ArrayLiteral(elements));
                }
            }
            // Subscripted lvalue: `name[expr]=…` or `name[expr]+=…`.
            // Only fires before the assignment value has started AND
            // when the accumulated text is identifier-shaped. We
            // speculatively scan the `[…]` and the optional `+`; if
            // an `=` follows, this is an indexed assignment. Otherwise
            // (e.g. `cmd[[foo]]`, glob-style `[abc]*`), we fall back
            // to treating the `[` and everything we scanned as literal
            // text so existing word semantics are preserved.
            '[' if !in_assignment_value && word_is_identifier_so_far(&current, &parts) => {
                let mut raw_subscript = String::new();
                let mut depth: usize = 1;
                let mut closed_subscript = false;
                while let Some(&c) = chars.peek() {
                    if c == '[' {
                        depth += 1;
                        raw_subscript.push(c);
                        chars.next();
                    } else if c == ']' {
                        chars.next();
                        depth -= 1;
                        if depth == 0 {
                            closed_subscript = true;
                            break;
                        }
                        raw_subscript.push(c);
                    } else {
                        raw_subscript.push(c);
                        chars.next();
                    }
                }
                // Decide: is this an assignment? Peek for `=` or `+=`.
                let assign_op: Option<bool> = if closed_subscript {
                    match chars.peek().copied() {
                        Some('=') => {
                            chars.next();
                            Some(false)
                        }
                        Some('+') => {
                            // Need to peek two chars; clone iter for lookahead.
                            let mut peeker = chars.clone();
                            peeker.next();
                            if peeker.peek() == Some(&'=') {
                                chars.next(); // consume '+'
                                chars.next(); // consume '='
                                Some(true)
                            } else {
                                None
                            }
                        }
                        _ => None,
                    }
                } else {
                    None
                };
                match assign_op {
                    Some(append) => {
                        let name = std::mem::take(&mut current);
                        debug_assert!(
                            parts.is_empty(),
                            "word_is_identifier_so_far guarantees no prior parts"
                        );
                        let subscript = parse_subscript_body(&raw_subscript, opts)?;
                        in_assignment_value = true;
                        has_token = true;
                        parts.push(WordPart::AssignPrefix {
                            target: crate::command::AssignTarget::Indexed { name, subscript },
                            append,
                        });
                        // Compound RHS: `name[i]=(...)`.
                        if chars.peek() == Some(&'(') {
                            chars.next();
                            let elements = scan_array_literal(&mut chars, opts)?;
                            parts.push(WordPart::ArrayLiteral(elements));
                        }
                    }
                    None => {
                        // Not an indexed assignment. Fall back: append
                        // the `[`, the scanned subscript text, and the
                        // closing `]` (if any) back into the current
                        // literal so the word behaves the same as
                        // before this arm existed.
                        has_token = true;
                        current.push('[');
                        current.push_str(&raw_subscript);
                        if closed_subscript {
                            current.push(']');
                        }
                    }
                }
            }
            other => {
                has_token = true;
                current.push(other);
            }
        }
    }

    if has_token {
        flush_literal(&mut parts, &mut current, false);
        let n = emit_word_with_braces(&mut tokens, parts, brace_expand)?;
        for _ in 0..n { push_pos!(token_start, token_start_line); }
    }
    // If there are unresolved pending heredocs after end-of-input, it's an error.
    if !pending_heredocs.is_empty() {
        return Err(LexError::UnterminatedHeredoc);
    }
    Ok(())
    })();

    match result {
        Ok(()) => {
            push_pos!(input.len(), chars.line()); // sentinel, ONLY on success
            debug_assert_eq!(
                offsets.len(),
                tokens.len() + 1,
                "offset sidecar must have one entry per token plus a sentinel"
            );
            debug_assert_eq!(offsets.len(), lines.len(), "offsets/lines out of lockstep");
            (tokens, offsets, lines, None)
        }
        Err(e) => {
            // Partial: keep the tokens produced before the error and push the
            // error byte offset as the trailing sentinel, preserving the
            // `offsets.len() == tokens.len() + 1` invariant.
            let off = chars.offset();
            push_pos!(off, chars.line());
            debug_assert_eq!(
                offsets.len(),
                tokens.len() + 1,
                "offset sidecar must have one entry per token plus a sentinel"
            );
            debug_assert_eq!(offsets.len(), lines.len(), "offsets/lines out of lockstep");
            (tokens, offsets, lines, Some((e, off)))
        }
    }
}

/// Like `tokenize_with_offsets`, but on a lex error returns the tokens produced
/// BEFORE the error plus `Some((error, byte_offset))`. On success the third
/// element is `None`. In BOTH cases `offsets.len() == tokens.len() + 1`: the
/// trailing offset is `input.len()` on success, or the error byte offset on
/// failure. This lets the source reader execute the complete units that lexed
/// before the failure and re-lex the truncated trailing unit.
pub fn tokenize_partial(
    input: &str,
    opts: LexerOptions,
) -> PartialTokens {
    tokenize_partial_inner(input, opts, true)
}

/// Tokenizes WITHOUT brace expansion (used for array-literal elements, whose
/// braces are expanded later by `brace_expand_parts` so adjacent expansions
/// are preserved as real WordParts).
fn tokenize_no_brace(input: &str, opts: LexerOptions) -> Result<Vec<Token>, LexError> {
    match tokenize_partial_inner(input, opts, false) {
        (tokens, _, _, None) => Ok(tokens),
        (_, _, _, Some((e, _))) => Err(e),
    }
}

/// Reads `X(...)` where `prefix` is the just-seen extglob prefix char (one of
/// `? * + @ !`), consuming a balanced-paren group (nested parens; inner
/// `|`/metachars literal). `chars` is positioned just before the `(`; returns
/// the group's word PARTS INCLUDING the prefix char, e.g. `+($x)` yields a
/// Literal `"+("`, a Var for `$x`, and a Literal `")"`. Inner `$…`/`` `…` ``/
/// quotes are preserved as their own parts so they expand at runtime; the
/// structural `(`/`|`/`)`/prefix stay literal. EOF before the closing `)` is
/// `LexError::UnterminatedExtglob`.
/// `Some(text)` when `parts` is exactly one unquoted `Literal` (the keyword form,
/// like `[[` / `]]` / `=~`); `None` otherwise.
fn single_unquoted_literal(parts: &[WordPart]) -> Option<&str> {
    match parts {
        [WordPart::Literal { text, quoted: false }] => Some(text.as_str()),
        _ => None,
    }
}

/// Scan the RHS operand of `=~` inside `[[ … ]]` as one regex word. `(`/`)`/`|`/`((`
/// are literal; paren depth keeps unquoted whitespace part of the operand while >0.
/// `$…`/`` `…` ``/quotes/`\` behave as in a normal word. No brace expansion, no
/// extglob. The cursor starts at the operand's first char; returns sitting just
/// before the terminating depth-0 whitespace (or at EOF).
fn scan_regex_operand(chars: &mut CharCursor<'_>, opts: LexerOptions) -> Result<Vec<WordPart>, LexError> {
    let mut parts: Vec<WordPart> = Vec::new();
    let mut lit = String::new();
    let mut depth: u32 = 0;
    fn flush(lit: &mut String, parts: &mut Vec<WordPart>) {
        if !lit.is_empty() {
            parts.push(WordPart::Literal { text: std::mem::take(lit), quoted: false });
        }
    }
    loop {
        let c = match chars.peek() {
            None => break,
            Some(&c) => c,
        };
        if depth == 0 && c.is_whitespace() {
            // Leading whitespace only reaches here after a `\`-newline line
            // continuation was consumed just before the operand began (e.g.
            // bash_completion's `[[ $line =~ \<newline>   regex ]]`); the
            // continuation line's indentation must be skipped, not treated as
            // the (still-empty) operand's terminator. Once the operand has
            // content, depth-0 whitespace ends it as before.
            if lit.is_empty() && parts.is_empty() {
                chars.next();
                continue;
            }
            break; // terminate, leave whitespace for the main loop
        }
        chars.next();
        match c {
            '$' => {
                flush(&mut lit, &mut parts);
                scan_dollar_expansion(chars, &mut parts, false, opts)?;
            }
            '`' => {
                flush(&mut lit, &mut parts);
                let sequence = scan_backtick_substitution(chars, opts)?;
                parts.push(WordPart::CommandSub { sequence, quoted: false });
            }
            '\'' => {
                flush(&mut lit, &mut parts);
                let mut inner = String::new();
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(ch) => inner.push(ch),
                        None => return Err(LexError::UnterminatedQuote),
                    }
                }
                parts.push(WordPart::Literal { text: inner, quoted: true });
            }
            '"' => {
                flush(&mut lit, &mut parts);
                let mut q = String::new();
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(esc @ ('"' | '\\' | '$' | '`')) => q.push(esc),
                            Some('\n') => {}
                            Some(other) => {
                                q.push('\\');
                                q.push(other);
                            }
                            None => return Err(LexError::UnterminatedQuote),
                        },
                        Some('$') => {
                            flush_literal(&mut parts, &mut q, true);
                            scan_dollar_expansion(chars, &mut parts, true, opts)?;
                        }
                        Some('`') => {
                            flush_literal(&mut parts, &mut q, true);
                            let sequence = scan_backtick_substitution(chars, opts)?;
                            parts.push(WordPart::CommandSub { sequence, quoted: true });
                        }
                        Some(ch) => q.push(ch),
                        None => return Err(LexError::UnterminatedQuote),
                    }
                }
                flush_literal(&mut parts, &mut q, true);
            }
            '\\' => match chars.next() {
                Some('\n') => {} // line continuation
                Some(next) => {
                    lit.push('\\');
                    lit.push(next);
                }
                None => lit.push('\\'),
            },
            '(' => {
                lit.push('(');
                depth += 1;
            }
            ')' => {
                lit.push(')');
                depth = depth.saturating_sub(1);
            }
            other => lit.push(other), // includes | < > ; & and depth>0 whitespace
        }
    }
    flush(&mut lit, &mut parts);
    Ok(parts)
}

fn scan_extglob_group(
    prefix: char,
    chars: &mut CharCursor<'_>,
    opts: LexerOptions,
) -> Result<Vec<WordPart>, LexError> {
    let mut group_parts: Vec<WordPart> = Vec::new();
    let mut lit = format!("{prefix}(");
    chars.next(); // consume '('
    let mut depth = 1usize;

    // Flush the accumulated structural/literal text as one unquoted Literal.
    fn flush(lit: &mut String, parts: &mut Vec<WordPart>) {
        if !lit.is_empty() {
            parts.push(WordPart::Literal { text: std::mem::take(lit), quoted: false });
        }
    }

    while let Some(c) = chars.next() {
        match c {
            '$' => {
                flush(&mut lit, &mut group_parts);
                scan_dollar_expansion(chars, &mut group_parts, false, opts)?;
            }
            '`' => {
                flush(&mut lit, &mut group_parts);
                let sequence = scan_backtick_substitution(chars, opts)?;
                group_parts.push(WordPart::CommandSub { sequence, quoted: false });
            }
            '\'' => {
                // Single quote: literal, no expansion.
                flush(&mut lit, &mut group_parts);
                let mut inner = String::new();
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(ch) => inner.push(ch),
                        None => return Err(LexError::UnterminatedQuote),
                    }
                }
                group_parts.push(WordPart::Literal { text: inner, quoted: true });
            }
            '"' => {
                // Double quote: mirror the main loop's `"` arm.
                flush(&mut lit, &mut group_parts);
                let mut quoted_current = String::new();
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(esc @ ('"' | '\\' | '$' | '`')) => quoted_current.push(esc),
                            Some('\n') => {}
                            Some(other) => {
                                quoted_current.push('\\');
                                quoted_current.push(other);
                            }
                            None => return Err(LexError::UnterminatedQuote),
                        },
                        Some('$') => {
                            flush_literal(&mut group_parts, &mut quoted_current, true);
                            scan_dollar_expansion(chars, &mut group_parts, true, opts)?;
                        }
                        Some('`') => {
                            flush_literal(&mut group_parts, &mut quoted_current, true);
                            let sequence = scan_backtick_substitution(chars, opts)?;
                            group_parts.push(WordPart::CommandSub { sequence, quoted: true });
                        }
                        Some(ch) => quoted_current.push(ch),
                        None => return Err(LexError::UnterminatedQuote),
                    }
                }
                flush_literal(&mut group_parts, &mut quoted_current, true);
            }
            '\\' => {
                // Literal escape: keep both chars.
                lit.push('\\');
                if let Some(next) = chars.next() {
                    lit.push(next);
                }
            }
            '(' => {
                lit.push('(');
                depth += 1;
            }
            ')' => {
                lit.push(')');
                depth -= 1;
                if depth == 0 {
                    flush(&mut lit, &mut group_parts);
                    return Ok(group_parts);
                }
            }
            other => lit.push(other),
        }
    }
    Err(LexError::UnterminatedExtglob) // EOF before closing ')'
}

/// Returns true if any unquoted Literal part in `parts` contains
/// an unquoted `{`. The fast-path check for brace expansion.
fn word_contains_unquoted_brace(parts: &[WordPart]) -> bool {
    parts.iter().any(|p| {
        matches!(p, WordPart::Literal { text, quoted: false } if text.contains('{'))
    })
}

/// Builds a concat string for brace expansion. Unquoted Literal
/// text is appended verbatim. Other parts (quoted Literals, Var,
/// Arith, CommandSub, Tilde, etc.) get a sentinel block
/// `\u{E000}<idx>\u{E001}` and are recorded in `placeholders`.
fn build_concat_with_sentinels(parts: &[WordPart]) -> (String, Vec<WordPart>) {
    let mut concat = String::new();
    let mut placeholders: Vec<WordPart> = Vec::new();
    for p in parts {
        match p {
            WordPart::Literal { text, quoted: false } => {
                concat.push_str(text);
            }
            other => {
                let idx = placeholders.len();
                placeholders.push(other.clone());
                concat.push('\u{E000}');
                concat.push_str(&idx.to_string());
                concat.push('\u{E001}');
            }
        }
    }
    (concat, placeholders)
}

/// Walks an expanded brace-expansion string and reconstructs a
/// `Vec<WordPart>`. Literal runs (no sentinels) become Literals
/// with `quoted: false`. Each sentinel block `\u{E000}<idx>\u{E001}`
/// is replaced by `placeholders[idx].clone()`.
fn split_on_sentinels(s: &str, placeholders: &[WordPart]) -> Vec<WordPart> {
    let mut out: Vec<WordPart> = Vec::new();
    let mut buf = String::new();
    let mut chars = CharCursor::new(s);
    while let Some(c) = chars.next() {
        if c == '\u{E000}' {
            if !buf.is_empty() {
                out.push(WordPart::Literal { text: std::mem::take(&mut buf), quoted: false });
            }
            let mut idx_str = String::new();
            while let Some(&nc) = chars.peek() {
                if nc == '\u{E001}' {
                    chars.next();
                    break;
                }
                idx_str.push(nc);
                chars.next();
            }
            if let Ok(idx) = idx_str.parse::<usize>()
                && let Some(p) = placeholders.get(idx)
            {
                out.push(p.clone());
            }
        } else {
            buf.push(c);
        }
    }
    if !buf.is_empty() {
        out.push(WordPart::Literal { text: buf, quoted: false });
    }
    out
}

/// Emits a Word into `tokens`. If the parts contain an unquoted
/// `{`, runs brace expansion and emits one Word per expansion.
/// Emits the word for `parts` into `tokens`, expanding any unquoted braces.
/// Returns the number of tokens pushed (1 normally, or one per brace-expansion
/// product). Callers that track byte offsets push the word's start offset this
/// many times to keep the offset sidecar in lockstep with the token stream.
fn emit_word_with_braces(
    tokens: &mut Vec<Token>,
    parts: Vec<WordPart>,
    brace_expand: bool,
) -> Result<usize, LexError> {
    if !brace_expand {
        tokens.push(Token::Word(Word(parts)));
        return Ok(1);
    }
    let products = brace_expand_parts(parts)?;
    let count = products.len();
    for p in products {
        tokens.push(Token::Word(Word(p)));
    }
    Ok(count)
}

/// Brace-expands a word's `parts` into one-or-more parts-lists. With no
/// unquoted brace, returns the single input list unchanged. Non-literal
/// parts (expansions, quoted runs) are sentinel-protected so only literal
/// source braces expand. Shared by `emit_word_with_braces` (command words)
/// and `scan_array_literal` (bare array elements).
fn brace_expand_parts(parts: Vec<WordPart>) -> Result<Vec<Vec<WordPart>>, LexError> {
    if !word_contains_unquoted_brace(&parts) {
        return Ok(vec![parts]);
    }
    let (concat, placeholders) = build_concat_with_sentinels(&parts);
    let expansions = crate::brace_expand::expand(&concat)
        .map_err(|_| LexError::BraceExpansionLimit)?;
    Ok(expansions
        .into_iter()
        .map(|s| split_on_sentinels(&s, &placeholders))
        .collect())
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
    chars: &mut CharCursor<'_>,
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
    chars: &mut CharCursor<'_>,
    pending: &mut std::collections::VecDeque<PendingHeredoc>,
    tokens: &mut [Token],
    opts: LexerOptions,
) -> Result<(), LexError> {
    while let Some(ph) = pending.pop_front() {
        let body = collect_one_heredoc_body(chars, &ph, opts)?;
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
pub fn ends_with_continuation_backslash(s: &str) -> bool {
    s.chars().rev().take_while(|&c| c == '\\').count() % 2 == 1
}

/// Collects the body of one heredoc, reading lines until the close-delimiter
/// is matched (or end-of-input, which is an error).
fn collect_one_heredoc_body(
    chars: &mut CharCursor<'_>,
    ph: &PendingHeredoc,
    opts: LexerOptions,
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
            scan_expanding_body_line(&body_line, &mut body_parts, opts)?;
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
    opts: LexerOptions,
) -> Result<(), LexError> {
    let mut chars = CharCursor::new(line);
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
                scan_dollar_expansion(&mut chars, parts, true, opts)?;
            }
            '`' => {
                flush_body_literal(parts, &mut current, false);
                let sequence = scan_backtick_substitution(&mut chars, opts)?;
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
/// Converts a raw arithmetic body string into an expandable `Word`, treating
/// it as if within double quotes (bash's rule for arithmetic expressions).
/// `$`-forms become ParamExpansion/Var/CommandSub/Arith parts; backticks
/// become CommandSub; everything else is literal text. Used by `$(( ))`
/// (lexer) and, via `command.rs`, by `(( ))` and arith-`for` headers.
pub(crate) fn arith_string_to_word(s: &str, opts: LexerOptions) -> Result<Word, LexError> {
    let mut chars = CharCursor::new(s);
    let mut parts: Vec<WordPart> = Vec::new();
    let mut lit = String::new();
    macro_rules! flush_lit {
        () => {
            if !lit.is_empty() {
                parts.push(WordPart::Literal { text: std::mem::take(&mut lit), quoted: true });
            }
        };
    }
    while let Some(&c) = chars.peek() {
        match c {
            '$' => {
                flush_lit!();
                chars.next();
                scan_dollar_expansion(&mut chars, &mut parts, true, opts)?;
            }
            '`' => {
                flush_lit!();
                chars.next();
                let sequence = scan_backtick_substitution(&mut chars, opts)?;
                parts.push(WordPart::CommandSub { sequence, quoted: true });
            }
            // bash performs quote removal within arithmetic: the quote
            // characters disappear and adjacent text concatenates
            // (`1"2"3` == 123, `x == "5"` == `x == 5`). Single quotes are
            // literal; double quotes still expand `$`-forms inside.
            '\'' => {
                chars.next();
                for ch in chars.by_ref() {
                    if ch == '\'' { break; }
                    lit.push(ch);
                }
            }
            '"' => {
                chars.next();
                while let Some(&ch) = chars.peek() {
                    match ch {
                        '"' => { chars.next(); break; }
                        '\\' => {
                            chars.next();
                            if let Some(&n) = chars.peek() {
                                // Inside double quotes, `\` only escapes a few
                                // metacharacters; otherwise it stays literal.
                                if matches!(n, '"' | '\\' | '$' | '`') {
                                    chars.next();
                                    lit.push(n);
                                } else {
                                    lit.push('\\');
                                }
                            } else {
                                lit.push('\\');
                            }
                        }
                        '$' => {
                            flush_lit!();
                            chars.next();
                            scan_dollar_expansion(&mut chars, &mut parts, true, opts)?;
                        }
                        '`' => {
                            flush_lit!();
                            chars.next();
                            let sequence = scan_backtick_substitution(&mut chars, opts)?;
                            parts.push(WordPart::CommandSub { sequence, quoted: true });
                        }
                        _ => { lit.push(ch); chars.next(); }
                    }
                }
            }
            _ => { lit.push(c); chars.next(); }
        }
    }
    flush_lit!();
    Ok(Word(parts))
}

fn scan_dollar_expansion(
    chars: &mut CharCursor<'_>,
    parts: &mut Vec<WordPart>,
    quoted: bool,
    opts: LexerOptions,
) -> Result<(), LexError> {
    match chars.peek().copied() {
        Some('(') => {
            chars.next(); // consume first '('
            if chars.peek() == Some(&'(') {
                // `$((` is EITHER an arithmetic expansion `$(( … ))` OR a command
                // substitution whose body starts with a subshell written glued:
                // `$( (subshell) … )`. Try arithmetic; if the body does not close
                // as `))` (scan_arith_body Err — bash's "not arithmetic" signal),
                // rewind to just after the first `(` and reparse as a command
                // substitution so the inner `(` parses as a subshell. Mirrors bash.
                let saved = chars.clone();
                chars.next(); // consume the second '('
                match scan_arith_body(chars) {
                    Ok(inner) => {
                        let body = arith_string_to_word(&inner, opts)?;
                        parts.push(WordPart::Arith { body, quoted });
                    }
                    Err(_) => {
                        *chars = saved; // rewind to just after the first '('
                        let sequence = scan_paren_substitution(chars, opts)?;
                        parts.push(WordPart::CommandSub { sequence, quoted });
                    }
                }
            } else {
                let sequence = scan_paren_substitution(chars, opts)?;
                parts.push(WordPart::CommandSub { sequence, quoted });
            }
        }
        Some('{') => {
            // Capture the `$` offset before consuming `{`. The `$` was already
            // consumed by the outer loop; chars.offset() is the position of `{`,
            // so `$` is exactly 1 byte before it.
            let dollar_start = chars.offset() - 1;
            chars.next(); // consume `{`
            scan_braced_param_expansion(chars, parts, quoted, opts, dollar_start)?;
        }
        Some('[') => {
            chars.next(); // consume '['
            let inner = scan_legacy_arith_body(chars)?;
            let body = arith_string_to_word(&inner, opts)?;
            parts.push(WordPart::Arith { body, quoted });
        }
        // `$'…'` is ANSI-C quoting ONLY outside double quotes. Inside `"…"`
        // (`quoted`) bash treats the `$` as a literal char, so skip this arm and
        // fall through to the `_` arm (literal `$`, the `'` left for the caller's
        // double-quote loop to take as a literal) — matching bash `echo "$'"` → `$'`.
        Some('\'') if !quoted => {
            chars.next();
            let text = scan_ansi_c_quoted(chars)?;
            parts.push(WordPart::Quoted {
                style: QuoteStyle::AnsiC,
                parts: vec![WordPart::Literal { text, quoted: true }],
            });
        }
        // `$"…"` is bash's locale-translation quoting, special only outside double
        // quotes. huck has no message catalog, so the translation is the identity:
        // `$"…"` ≡ `"…"`. Drop the `$` and leave the `"` unconsumed so the caller's
        // existing double-quote handler scans the body (with its normal
        // expansions/escapes). Inside double quotes (`quoted`) `$"` is a literal `$`
        // via the `_` arm, after which the `"` closes the surrounding string.
        Some('"') if !quoted => {}
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
        Some('-') => {
            chars.next();
            parts.push(WordPart::Var { name: "-".to_string(), quoted });
        }
        Some(c) if c.is_ascii_digit() => {
            let d = chars.next().unwrap();
            parts.push(WordPart::Var { name: d.to_string(), quoted });
        }
        Some(c) if is_name_start(c) => {
            let name = scan_var_name(chars);
            parts.push(WordPart::Var { name, quoted });
        }
        _ => {
            parts.push(WordPart::Literal { text: "$".to_string(), quoted });
        }
    }
    Ok(())
}

/// Reads the body of a `$'...'` ANSI-C quoted string. The opening `$'` has
/// already been consumed; this scans forward, processing C-style backslash
/// escapes, until the matching unescaped `'` is consumed. Returns the
/// decoded string.
fn scan_ansi_c_quoted(
    chars: &mut CharCursor<'_>,
) -> Result<String, LexError> {
    let mut out = String::new();
    loop {
        match chars.next() {
            None => return Err(LexError::UnterminatedQuote),
            Some('\'') => return Ok(out),
            Some('\\') => decode_ansi_c_escape(chars, &mut out)?,
            Some(c) => out.push(c),
        }
    }
}

/// Expands backslash escapes in `v` exactly as `$'...'` (ANSI-C quoting)
/// does, returning the decoded string. Used by `${var@E}`. Unknown
/// escapes (`\q`) and trailing `\` are preserved verbatim, matching bash.
pub fn decode_ansi_c_escapes(v: &str) -> String {
    let mut out = String::new();
    let mut chars = CharCursor::new(v);
    while let Some(c) = chars.next() {
        if c == '\\' {
            // `decode_ansi_c_escape` only errors on `\` at EOF (no escape
            // char). bash's `@E` leaves a trailing backslash literal.
            if decode_ansi_c_escape(&mut chars, &mut out).is_err() {
                out.push('\\');
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Decodes a single backslash escape inside `$'...'` and appends the
/// result to `out`. The leading `\` has already been consumed.
fn decode_ansi_c_escape(
    chars: &mut CharCursor<'_>,
    out: &mut String,
) -> Result<(), LexError> {
    match chars.next() {
        None => return Err(LexError::UnterminatedQuote),
        Some('a') => out.push('\x07'),
        Some('b') => out.push('\x08'),
        Some('e') | Some('E') => out.push('\x1B'),
        Some('f') => out.push('\x0C'),
        Some('n') => out.push('\n'),
        Some('r') => out.push('\r'),
        Some('t') => out.push('\t'),
        Some('v') => out.push('\x0B'),
        Some('\\') => out.push('\\'),
        Some('\'') => out.push('\''),
        Some('"') => out.push('"'),
        Some('?') => out.push('?'),
        Some(c @ '0'..='7') => {
            let mut v: u32 = c.to_digit(8).unwrap();
            for _ in 0..2 {
                match chars.peek().copied() {
                    Some(d @ '0'..='7') => {
                        chars.next();
                        v = v * 8 + d.to_digit(8).unwrap();
                    }
                    _ => break,
                }
            }
            push_codepoint(out, v)?;
        }
        Some('x') => {
            if chars.peek().copied().is_some_and(|c| c.is_ascii_hexdigit()) {
                let v = scan_hex_digits(chars, 2);
                push_codepoint(out, v)?;
            } else {
                out.push('\\');
                out.push('x');
            }
        }
        Some('u') => {
            if chars.peek().copied().is_some_and(|c| c.is_ascii_hexdigit()) {
                let v = scan_hex_digits(chars, 4);
                push_codepoint(out, v)?;
            } else {
                out.push('\\');
                out.push('u');
            }
        }
        Some('U') => {
            if chars.peek().copied().is_some_and(|c| c.is_ascii_hexdigit()) {
                let v = scan_hex_digits(chars, 8);
                push_codepoint(out, v)?;
            } else {
                out.push('\\');
                out.push('U');
            }
        }
        Some('c') => match chars.next() {
            None => {
                out.push('\\');
                out.push('c');
            }
            Some('?') => out.push('\x7F'),
            Some('@') => out.push('\0'),
            Some(c) => {
                let v = (c.to_ascii_uppercase() as u32) & 0x1F;
                push_codepoint(out, v)?;
            }
        },
        Some(other) => {
            out.push('\\');
            out.push(other);
        }
    }
    Ok(())
}

/// Reads up to `max` hex digits (greedy, stops at first non-hex char) and
/// returns their value. Caller has already confirmed at least one hex
/// digit is available.
fn scan_hex_digits(
    chars: &mut CharCursor<'_>,
    max: u32,
) -> u32 {
    let mut v: u32 = 0;
    for _ in 0..max {
        match chars.peek().copied() {
            Some(d) if d.is_ascii_hexdigit() => {
                chars.next();
                v = v.wrapping_mul(16) + d.to_digit(16).unwrap();
            }
            _ => break,
        }
    }
    v
}

/// Appends a codepoint to `out`, or errors if the value is not a valid
/// Unicode scalar (surrogate range or > U+10FFFF).
fn push_codepoint(out: &mut String, v: u32) -> Result<(), LexError> {
    match char::from_u32(v) {
        Some(c) => {
            out.push(c);
            Ok(())
        }
        None => Err(LexError::AnsiCInvalidCodepoint(v)),
    }
}

/// Scans the body of a `(( ... ))` block. The caller has already
/// consumed both opening `(` characters; this function consumes the
/// body and the matching `))`. Returns the raw body text. Tracks
/// nested paren depth so `(((a+b)*c))` correctly captures `((a+b)*c)`
/// as the body.
fn scan_arith_block(
    chars: &mut CharCursor<'_>,
) -> Result<String, LexError> {
    let mut collected = String::new();
    let mut depth: i32 = 0;
    while let Some(c) = chars.next() {
        match c {
            '(' => {
                depth += 1;
                collected.push('(');
            }
            ')' => {
                if depth == 0 {
                    if chars.peek() == Some(&')') {
                        chars.next(); // consume the second `)`
                        return Ok(collected);
                    }
                    // A `)` at depth 0 not forming `))` means the two opening
                    // `(` of the `((` cannot close as an adjacent `))` — this is
                    // not a balanced arithmetic block. Fail fast so the caller
                    // (the `((` lexer site) rewinds and re-lexes as nested
                    // subshells `( (`, instead of scanning on to an unrelated
                    // distant `))` elsewhere in the input (L-51).
                    return Err(LexError::UnterminatedArithBlock);
                }
                depth -= 1;
                collected.push(')');
            }
            _ => collected.push(c),
        }
    }
    Err(LexError::UnterminatedArithBlock)
}

/// Reads the inner text of a `$((...))` arithmetic expansion. The opening
/// `$((` has already been consumed; this function scans forward until the
/// matching `))` at depth 0. Returns the inner text (without the closing
/// `))`). Tracks paren depth so that nested `(` / `)` inside the
/// expression do not prematurely close the expansion.
fn scan_arith_body(
    chars: &mut CharCursor<'_>,
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

/// Appends a quoted span — the opening quote already pushed by the caller —
/// through its matching closing `quote`, verbatim. Single quotes take every
/// char literally; double quotes honor `\` so `\"` does not close the span.
/// Running out of input returns `Err(err)`.
fn push_quoted_span(
    chars: &mut CharCursor<'_>,
    quote: char,
    out: &mut String,
    err: LexError,
) -> Result<(), LexError> {
    loop {
        match chars.next() {
            None => return Err(err),
            Some(c) if c == quote => {
                out.push(c);
                return Ok(());
            }
            Some('\\') if quote == '"' => {
                out.push('\\');
                if let Some(c) = chars.next() {
                    out.push(c);
                }
            }
            Some(c) => out.push(c),
        }
    }
}

/// Skips a `${…}` parameter expansion VERBATIM — the opening `${` already
/// consumed and pushed by the caller — appending through the matching `}` at
/// brace-depth 0 (inclusive). Tracks `{`/`}` depth and `'…'`/`"…"` spans so a
/// `}` inside a nested expansion or quote does not close early. Used by
/// `scan_legacy_arith_body` so a `]` inside `${…}` cannot close the `$[…]`.
fn scan_braced_skip(
    chars: &mut CharCursor<'_>,
    out: &mut String,
) -> Result<(), LexError> {
    let mut depth: usize = 1; // inside the outer `${`
    loop {
        match chars.next() {
            None => return Err(LexError::UnterminatedLegacyArith),
            Some('{') => {
                depth += 1;
                out.push('{');
            }
            Some('}') => {
                depth -= 1;
                out.push('}');
                if depth == 0 {
                    return Ok(());
                }
            }
            Some(q @ ('\'' | '"')) => {
                out.push(q);
                push_quoted_span(chars, q, out, LexError::UnterminatedLegacyArith)?;
            }
            Some(c) => out.push(c),
        }
    }
}

/// Reads the inner text of a `$[ … ]` legacy arithmetic expansion. The opening
/// `$[` has already been consumed; this scans forward to the matching `]` and
/// returns the inner text (without the closing `]`). bash treats `$[ expr ]` as
/// exactly `$(( expr ))`, so the caller feeds the result to
/// `arith_string_to_word`. "Fully aware": tracks raw `[`/`]` nesting (so array
/// subscripts `a[1]`, `${a[i]}`, and nested `$[…]` balance as raw brackets) and
/// consumes `'…'`/`"…"` quoted spans and nested `$(…)`/`${…}` verbatim, so a `]`
/// inside any of them does not close the expansion. EOF before the close yields
/// `UnterminatedLegacyArith`.
fn scan_legacy_arith_body(
    chars: &mut CharCursor<'_>,
) -> Result<String, LexError> {
    let mut body = String::new();
    let mut depth: usize = 0; // raw `[` nesting
    loop {
        match chars.next() {
            None => return Err(LexError::UnterminatedLegacyArith),
            Some('[') => {
                depth += 1;
                body.push('[');
            }
            Some(']') => {
                if depth == 0 {
                    return Ok(body);
                }
                depth -= 1;
                body.push(']');
            }
            Some(q @ ('\'' | '"')) => {
                body.push(q);
                push_quoted_span(chars, q, &mut body, LexError::UnterminatedLegacyArith)?;
            }
            Some('\\') => {
                body.push('\\');
                if let Some(c) = chars.next() {
                    body.push(c);
                }
            }
            Some('$') => {
                body.push('$');
                match chars.peek().copied() {
                    Some('(') => {
                        chars.next(); // consume '('
                        body.push('(');
                        scan_cmdsub_body(chars, &mut body, LexError::UnterminatedLegacyArith)?;
                        body.push(')');
                    }
                    Some('{') => {
                        chars.next(); // consume '{'
                        body.push('{');
                        scan_braced_skip(chars, &mut body)?;
                    }
                    _ => {}
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
/// Consumes a `$(…)` command substitution body VERBATIM from `chars`, starting
/// just after the opening `(` (which the caller has already appended to `out`),
/// through the matching `)` (also appended). Any unquoted `(` raises the paren
/// depth and any unquoted `)` lowers it, so nested `$(…)`, `$((…))`, and
/// `$( (…) )` all balance; `'…'`/`"…"` spans are skipped (double-quote honors
/// `\`) so a `)` or `}` inside them does not affect depth. Running out of input
/// yields `Err(LexError::UnterminatedBrace)` (the same error `scan_braced_operand`
/// raises for an unterminated operand). Mirrors `scan_paren_substitution`'s loop
/// but appends text instead of parsing it.
/// Scans a `$(…)` command-substitution body, the opening `$(` having already
/// been consumed by the caller. Consumes through the matching `)` (which is
/// consumed but NOT appended); any unquoted `(` raises the paren depth and any
/// unquoted `)` lowers it, so nested `$(…)`, `$((…))`, and `$( (…) )` balance;
/// `'…'`/`"…"` spans are skipped (double-quote honors `\`) and `\` escapes the
/// next char — none affect depth. The body (excluding the closing `)`) is
/// appended to `out`. Running out of input unterminated returns `Err(unterminated)`.
/// The single source of truth for `$()` scanning (see `scan_paren_substitution`,
/// `consume_paren_cmdsub_verbatim`, `split_modifier_operand`).
fn scan_cmdsub_body(
    chars: &mut CharCursor<'_>,
    out: &mut String,
    unterminated: LexError,
) -> Result<(), LexError> {
    let mut depth: usize = 0;
    // `#` comment recognition (v183): a `#` at a word boundary starts a comment.
    let mut at_boundary = true;
    // v186: `case … esac` state so a BARE case-pattern `)` at paren-depth 0 is a
    // pattern terminator, not the cmdsub close. `cmd_pos` = the next word begins
    // at a COMMAND position (so a bare `case`/`esac` there is a keyword, but
    // `echo case` / `grep case` are not). `word` accumulates the current BARE
    // word (identifier chars); `word_bare` goes false once a quote/`$`/other char
    // makes the word not a bare keyword. KNOWN LIMITATION (pathological, absent
    // from real code): a `case`/`esac` literal in PATTERN position is mishandled —
    // a pattern named `case`/`esac` (after `in` or `;;`) is mis-counted, and the
    // empty case `$(case x in esac)` errors (huck doesn't track `in`, so the first
    // pattern position isn't a command position). Also a `VAR=val case` prefix-
    // assignment case. These match bash's own LEX_INCASE edges' rarity.
    let mut case_depth: usize = 0;
    let mut cmd_pos = true;
    let mut word = String::new();
    let mut word_bare = true;

    // End the current word: recognise a bare `case`/`esac` keyword at command
    // position; return whether it was a command-introducer keyword (for the
    // space transition). Resets `word`/`word_bare`.
    macro_rules! end_word {
        () => {{
            let introducer = word_bare
                && matches!(
                    word.as_str(),
                    "if" | "then" | "elif" | "else" | "while" | "until" | "do"
                );
            if word_bare && cmd_pos {
                if word == "case" {
                    case_depth += 1;
                } else if word == "esac" {
                    case_depth = case_depth.saturating_sub(1);
                }
            }
            word.clear();
            word_bare = true;
            introducer
        }};
    }

    loop {
        match chars.next() {
            None => return Err(unterminated),
            Some('#') if at_boundary => {
                end_word!();
                // Word-start comment to end-of-line: keep it VERBATIM in `out`
                // (re-tokenized + stripped later) so its `)` is not counted.
                out.push('#');
                while let Some(&c) = chars.peek() {
                    if c == '\n' {
                        break;
                    }
                    out.push(c);
                    chars.next();
                }
                // the trailing newline (next char) restores at_boundary + cmd_pos
            }
            Some(')') => {
                // Finalize the pending word FIRST so e.g. `esac)` updates
                // case_depth before we decide whether this `)` is the close.
                end_word!();
                if depth == 0 {
                    if case_depth == 0 {
                        return Ok(()); // the cmdsub close
                    }
                    // depth-0 `)` inside a `case` is a pattern terminator — keep
                    // scanning; a clause body (commands) follows.
                    out.push(')');
                } else {
                    depth -= 1;
                    out.push(')');
                }
                at_boundary = true;
                cmd_pos = true;
            }
            Some('(') => {
                end_word!();
                depth += 1;
                out.push('(');
                at_boundary = true;
                cmd_pos = true;
            }
            Some('\\') => {
                word_bare = false;
                out.push('\\');
                match chars.next() {
                    Some(c) => out.push(c),
                    None => return Err(unterminated),
                }
                at_boundary = false;
            }
            Some('\'') => {
                word_bare = false;
                out.push('\'');
                loop {
                    match chars.next() {
                        Some('\'') => {
                            out.push('\'');
                            break;
                        }
                        Some(c) => out.push(c),
                        None => return Err(unterminated),
                    }
                }
                at_boundary = false;
            }
            Some('"') => {
                word_bare = false;
                out.push('"');
                loop {
                    match chars.next() {
                        Some('"') => {
                            out.push('"');
                            break;
                        }
                        Some('\\') => {
                            out.push('\\');
                            match chars.next() {
                                Some(c) => out.push(c),
                                None => return Err(unterminated),
                            }
                        }
                        Some(c) => out.push(c),
                        None => return Err(unterminated),
                    }
                }
                at_boundary = false;
            }
            Some(c) => {
                out.push(c);
                if c.is_ascii_alphanumeric() || c == '_' {
                    // identifier char: extend the current bare word.
                    if word_bare {
                        word.push(c);
                    }
                    at_boundary = false;
                } else if c.is_whitespace() {
                    // A word was being built iff `word` is non-empty (bare) or
                    // `word_bare` is false (a non-bare word). Whitespace after a
                    // separator (no word) must PRESERVE cmd_pos (e.g. after `;;`).
                    let had_word = !word.is_empty() || !word_bare;
                    let introducer = end_word!();
                    if had_word {
                        // command position survives a space only after an
                        // introducer keyword (`then case` → keyword; `echo case` → arg).
                        cmd_pos = introducer;
                    }
                    at_boundary = true;
                } else if matches!(c, ';' | '&' | '|') {
                    end_word!();
                    cmd_pos = true;
                    at_boundary = true;
                } else if matches!(c, '{' | '}') {
                    end_word!();
                    cmd_pos = c == '{';
                    at_boundary = false;
                } else if matches!(c, '<' | '>') {
                    end_word!();
                    cmd_pos = false; // redirect — same command
                    at_boundary = true;
                } else {
                    // `$`, `-`, `.`, `*`, `?`, `=`, `~`, backtick, etc.: continues
                    // / starts a word that is not a bare keyword.
                    word_bare = false;
                    at_boundary = false;
                }
            }
        }
    }
}

fn consume_paren_cmdsub_verbatim(
    chars: &mut CharCursor<'_>,
    out: &mut String,
) -> Result<(), LexError> {
    // The kernel consumes (but does not append) the closing `)`; re-add it so
    // the command substitution is reconstructed verbatim in `out`.
    scan_cmdsub_body(chars, out, LexError::UnterminatedBrace)?;
    out.push(')');
    Ok(())
}

/// Scans a backtick (`` `…` ``) command-substitution body, the opening backtick
/// having already been consumed by the caller. Consumes through the matching
/// un-escaped backtick (consumed but NOT appended); a `\` escapes the next char
/// (so `` \` `` does not close — the `\` and next char are appended raw). The
/// raw body (escapes preserved, excluding the closing backtick) is appended to
/// `out`. Backticks are quote-naive and do not nest. EOF → `Err(unterminated)`.
/// The single source of truth for backtick boundary scanning (see
/// `scan_backtick_substitution`, `consume_backtick_verbatim`).
fn scan_backtick_body(
    chars: &mut CharCursor<'_>,
    out: &mut String,
    unterminated: LexError,
) -> Result<(), LexError> {
    loop {
        match chars.next() {
            None => return Err(unterminated),
            Some('`') => return Ok(()),
            Some('\\') => {
                out.push('\\');
                match chars.next() {
                    Some(c) => out.push(c),
                    None => return Err(unterminated),
                }
            }
            Some(c) => out.push(c),
        }
    }
}

/// Appends a backtick command substitution to `out` verbatim, the opening
/// backtick having already been pushed by the caller: the kernel collects the
/// raw body (excluding the closing backtick); this re-adds the closing backtick.
fn consume_backtick_verbatim(
    chars: &mut CharCursor<'_>,
    out: &mut String,
) -> Result<(), LexError> {
    scan_backtick_body(chars, out, LexError::UnterminatedBrace)?;
    out.push('`');
    Ok(())
}

/// Applies bash's backtick un-escaping to a raw backtick body: `` \` `` → `` ` ``,
/// `\\` → `\`, `\$` → `$`, and `\x` (any other char) → `\x` verbatim. A trailing
/// lone `\` is kept. Only the parse path un-escapes, so it lives in one function.
fn unescape_backtick(raw: &str) -> String {
    let mut out = String::new();
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('`') => out.push('`'),
                Some('\\') => out.push('\\'),
                Some('$') => out.push('$'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Recovery for a lexable-but-invalid `${…}`: consume the rest of the brace
/// body through the matching `}`, then build a `BadSubst` ParamExpansion whose
/// `raw` is the literal `${…}` source (for the runtime message). `dollar_start`
/// is the offset of the leading `$`. Used so bad substitutions defer to runtime
/// instead of aborting the parse (matching bash).
fn recover_bad_subst(
    chars: &mut CharCursor<'_>,
    parts: &mut Vec<WordPart>,
    quoted: bool,
    dollar_start: usize,
) -> Result<(), LexError> {
    // `scan_braced_operand` consumes through the matching `}` (depth + quote +
    // $'…' aware after Task 1). It returns the inner body; we don't need it —
    // we slice the raw source instead, which includes `${` … `}`.
    let _ = scan_braced_operand(chars)?; // may still error on genuinely unterminated
    let raw = chars.slice_from(dollar_start).to_string();
    parts.push(WordPart::ParamExpansion {
        name: String::new(),
        modifier: ParamModifier::BadSubst { raw },
        quoted,
        subscript: None,
        indirect: false,
    });
    Ok(())
}

fn scan_braced_operand(
    chars: &mut CharCursor<'_>,
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
            Some('`') => {
                // Backtick command substitution: consume verbatim through the
                // matching unescaped backtick so a `}` inside it does not close
                // the operand (L-52). `\` escapes the next char inside.
                body.push('`');
                consume_backtick_verbatim(chars, &mut body)?;
            }
            Some('$') => {
                // `${` nests; `$(` is a cmdsub consumed verbatim; `$'…'` /
                // `$"…"` are ANSI-C / locale quoted spans whose internal `'`/`"`
                // (and `\'` escapes) must not be mistaken for plain quoting.
                body.push('$');
                match chars.peek() {
                    Some(&'{') => {
                        chars.next();
                        body.push('{');
                        depth += 1;
                    }
                    Some(&'(') => {
                        chars.next();
                        body.push('(');
                        consume_paren_cmdsub_verbatim(chars, &mut body)?;
                    }
                    Some(&'\'') => {
                        chars.next();
                        body.push('\'');
                        // ANSI-C span: `\` escapes the next char (incl. `\'`),
                        // closing on the first UNescaped `'`.
                        loop {
                            match chars.next() {
                                None => return Err(LexError::UnterminatedBrace),
                                Some('\\') => {
                                    body.push('\\');
                                    if let Some(c) = chars.next() { body.push(c); }
                                }
                                Some('\'') => { body.push('\''); break; }
                                Some(c) => body.push(c),
                            }
                        }
                    }
                    Some(&'"') => {
                        chars.next();
                        body.push('"');
                        // Locale `$"…"`: same scan as a normal double-quote span
                        // (handled by the outer `Some('"')` loop shape).
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
                    _ => {}
                }
            }
            Some('{') => { body.push('{'); } // bare brace: literal, does not nest
            Some('}') => {
                if depth == 1 { return Ok(body); }
                depth -= 1;
                body.push('}');
            }
            Some(c) => body.push(c),
        }
    }
}

/// Parses a brace-modifier operand BODY (already extracted up to the matching
/// `}` by `scan_braced_operand`) as a single WORD: `$…` / `` `…` `` / quotes are
/// expansions/quoting; ALL other characters — including shell metacharacters
/// `(` `)` `|` `;` `&` `<` `>` and whitespace — are LITERAL. Field splitting is
/// NOT driven by the `quoted` flag inside the modifier word: at expansion time
/// the modifier word goes through `expand_assignment` (which returns the string
/// verbatim, no splitting), and the OUTER `ParamExpansion`'s own `quoted` flag
/// in `expand()` then drives any IFS splitting of the result. So unquoted
/// `${x:-a b}` splits to `a` `b` and `"${x:-a b}"` stays one — driven by the
/// outer context, not these parts. The inner `quoted` flags are set correctly
/// (unquoted literals, quoted spans/escapes) for future-compatibility and
/// glob-safety. Matches bash: the operand of `:-`/`:=`/`:?`/`:+` (and
/// substitution/substring operands) is a word, not a command.
fn parse_braced_operand_opts(
    body: &str,
    enclosing_dquote: bool,
    opts: LexerOptions,
) -> Result<Word, LexError> {
    let mut chars = CharCursor::new(body);
    let mut parts: Vec<WordPart> = Vec::new();
    let mut cur = String::new();
    // When the `${...}` is itself inside double quotes, a VALUE-substitution
    // operand (`:-`/`:=`/`:+`) is in double-quote context: single quotes are
    // LITERAL and backslash is special only before `$ ` " \`. `q` is the
    // quoted-ness of the bare literal text / expansions.
    let q = enclosing_dquote;
    while let Some(c) = chars.next() {
        match c {
            // Double-quote context: backslash is special only before `$ ` " \`;
            // any other `\x` keeps the backslash literal (so `\*`/`\n` survive).
            '\\' if enclosing_dquote => match chars.peek().copied() {
                Some(e @ ('$' | '`' | '"' | '\\')) => {
                    chars.next();
                    flush_body_literal(&mut parts, &mut cur, true);
                    parts.push(WordPart::Literal { text: e.to_string(), quoted: true });
                }
                _ => cur.push('\\'),
            },
            '\\' => {
                // Backslash escapes the next char: emit it as a quoted literal
                // (glob-safe, consistent with the main tokenizer). `\` at end of
                // body silently vanishes.
                if let Some(n) = chars.next() {
                    flush_body_literal(&mut parts, &mut cur, false);
                    parts.push(WordPart::Literal { text: n.to_string(), quoted: true });
                }
            }
            '$' => {
                flush_body_literal(&mut parts, &mut cur, q);
                scan_dollar_expansion(&mut chars, &mut parts, q, opts)?;
            }
            '`' => {
                flush_body_literal(&mut parts, &mut cur, q);
                let sequence = scan_backtick_substitution(&mut chars, opts)?;
                parts.push(WordPart::CommandSub { sequence, quoted: q });
            }
            // In double-quote context a single quote is a LITERAL character.
            '\'' if enclosing_dquote => cur.push('\''),
            '\'' => {
                // Single-quoted span: everything literal until the next `'`.
                flush_body_literal(&mut parts, &mut cur, false);
                let mut s = String::new();
                loop {
                    match chars.next() {
                        None => return Err(LexError::UnterminatedQuote),
                        Some('\'') => break,
                        Some(ch) => s.push(ch),
                    }
                }
                parts.push(WordPart::Literal { text: s, quoted: true });
            }
            '"' => {
                // Double-quoted span: $/`/\ active; everything else literal (quoted).
                flush_body_literal(&mut parts, &mut cur, q);
                loop {
                    match chars.next() {
                        None => return Err(LexError::UnterminatedQuote),
                        Some('"') => break,
                        Some('\\') => match chars.peek().copied() {
                            Some(e @ ('$' | '`' | '"' | '\\')) => {
                                chars.next();
                                flush_body_literal(&mut parts, &mut cur, true);
                                parts.push(WordPart::Literal { text: e.to_string(), quoted: true });
                            }
                            _ => cur.push('\\'),
                        },
                        Some('$') => {
                            flush_body_literal(&mut parts, &mut cur, true);
                            scan_dollar_expansion(&mut chars, &mut parts, true, opts)?;
                        }
                        Some('`') => {
                            flush_body_literal(&mut parts, &mut cur, true);
                            let sequence = scan_backtick_substitution(&mut chars, opts)?;
                            parts.push(WordPart::CommandSub { sequence, quoted: true });
                        }
                        Some(ch) => cur.push(ch),
                    }
                }
                flush_body_literal(&mut parts, &mut cur, true);
            }
            other => cur.push(other),
        }
    }
    flush_body_literal(&mut parts, &mut cur, q);
    Ok(Word(parts))
}

/// Back-compat shim for unit tests that parse a braced operand with extglob
/// off (the historical behavior). Production callers thread `LexerOptions`
/// via `parse_braced_operand_opts`.
#[cfg(test)]
fn parse_braced_operand(body: &str) -> Result<Word, LexError> {
    parse_braced_operand_opts(body, false, LexerOptions::default())
}

/// Reads the body of a `$(...)` substitution. The opening `$(` is already
/// consumed; this function consumes through the matching `)` at depth 0.
/// Tracks quote and escape state so that `)` inside `'...'`, `"..."`, or
/// after `\` does not close the substitution, and nested `$(...)` increments
/// the depth.
fn scan_paren_substitution(
    chars: &mut CharCursor<'_>,
    opts: LexerOptions,
) -> Result<crate::command::Sequence, LexError> {
    let mut body = String::new();
    scan_cmdsub_body(chars, &mut body, LexError::UnterminatedSubstitution)?;
    parse_substitution_body(&body, opts)
}

/// Tokenizes and parses a substitution body, wrapping any errors with the
/// substitution-context `LexError` variants. Empty bodies (whitespace only)
/// produce an empty `Sequence`.
fn parse_substitution_body(body: &str, opts: LexerOptions) -> Result<crate::command::Sequence, LexError> {
    let tokens = tokenize_with_opts(body, opts).map_err(|e| LexError::Substitution(Box::new(e)))?;
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
    chars: &mut CharCursor<'_>,
    opts: LexerOptions,
) -> Result<crate::command::Sequence, LexError> {
    let mut raw = String::new();
    scan_backtick_body(chars, &mut raw, LexError::UnterminatedSubstitution)?;
    parse_substitution_body(&unescape_backtick(&raw), opts)
}

fn empty_sequence() -> crate::command::Sequence {
    crate::command::Sequence {
        first: crate::command::Command::Pipeline(crate::command::Pipeline {
            negate: false,
            commands: Vec::new(),
        }),
        rest: Vec::new(),
        background: false,
    }
}

fn scan_var_name(chars: &mut CharCursor<'_>) -> String {
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
/// or a `WordPart::ParamExpansion` (any modifier). `dollar_start` is the byte
/// offset of the leading `$` in the source (for `recover_bad_subst`).
fn scan_braced_param_expansion(
    chars: &mut CharCursor<'_>,
    parts: &mut Vec<WordPart>,
    quoted: bool,
    opts: LexerOptions,
    dollar_start: usize,
) -> Result<(), LexError> {
    // Special single-char forms: ${@}, ${*}, ${#} (arg count).
    // These must be checked before the Length form (${#name}) disambiguation.
    // `${@}` and `${*}` produce `AllArgs`; `${@:...}` / `${*:...}` route
    // through `dispatch_braced_modifier` so the substring modifier
    // (v71 task 3: closes v33's slicing deferral) is parseable.
    match chars.peek().copied() {
        Some('@') => {
            chars.next();
            if chars.peek() == Some(&'}') {
                chars.next();
                parts.push(WordPart::AllArgs { joined: false, quoted });
                return Ok(());
            }
            // `${@<mod>...}` — fall through to the modifier dispatcher
            // with name="@" and no subscript.
            return dispatch_braced_modifier("@".to_string(), quoted, None, chars, parts, false, opts, dollar_start);
        }
        Some('*') => {
            chars.next();
            if chars.peek() == Some(&'}') {
                chars.next();
                parts.push(WordPart::AllArgs { joined: true, quoted });
                return Ok(());
            }
            return dispatch_braced_modifier("*".to_string(), quoted, None, chars, parts, false, opts, dollar_start);
        }
        // Scalar special params: ${-} (option flags), ${?} (exit status),
        // ${$} (shell pid). Route bare `}` and modifiers through the
        // dispatcher (e.g. `${-#*e}` from nvm). Resolved by `lookup_var`.
        Some('-') => {
            chars.next();
            return dispatch_braced_modifier("-".to_string(), quoted, None, chars, parts, false, opts, dollar_start);
        }
        Some('?') => {
            chars.next();
            return dispatch_braced_modifier("?".to_string(), quoted, None, chars, parts, false, opts, dollar_start);
        }
        Some('$') => {
            chars.next();
            return dispatch_braced_modifier("$".to_string(), quoted, None, chars, parts, false, opts, dollar_start);
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
            // Length of a special parameter's value: `${##}` = len of `$#`,
            // `${#?}` = len of `$?`, `${#-}` = len of `$-`, `${#$}` = len of
            // `$$`, `${#!}` = len of `$!` (bash semantics). `@`/`*` are caught
            // above (arg count), so this only matches `# $ ! ? -`.
            Some(c) if special_param_char(c) => { chars.next(); c.to_string() }
            _ => scan_braced_name(chars)?,
        };
        if name.is_empty() {
            // e.g. `${#+}` — bad substitution at runtime.
            return recover_bad_subst(chars, parts, quoted, dollar_start);
        }
        // Optional subscript for the Length form: `${#a[i]}`, `${#a[@]}`.
        // The named regular-identifier path is the only one that takes
        // a subscript — positional names (`${#1}`) and the `@`/`*`
        // forms (which already are pseudo-subscripts) do not.
        let subscript = if name.chars().all(|c| c == '_' || c.is_ascii_alphanumeric())
            && !name.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(true)
        {
            scan_param_subscript(chars, opts)?
        } else {
            None
        };
        if chars.next() != Some('}') {
            return Err(LexError::UnterminatedBrace);
        }
        parts.push(WordPart::ParamExpansion {
            name,
            modifier: ParamModifier::Length,
            quoted,
            subscript,
            indirect: false,
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
        // Positional parameters cannot be subscripted.
        return dispatch_braced_modifier(name, quoted, None, chars, parts, false, opts, dollar_start);
    }

    // `${!NAME[@]}` / `${!NAME[*]}` — array-keys form (v71). The bare
    // `${!NAME}` indirect form is NOT yet supported and is rejected
    // here by requiring the `[@]` / `[*]` subscript immediately after
    // the name.
    if chars.peek() == Some(&'!') {
        chars.next(); // consume '!'
        // Bare `${!}` is the `$!` special param (last bg pid), NOT indirect.
        if chars.peek() == Some(&'}') {
            chars.next(); // consume `}`
            parts.push(WordPart::Var { name: "!".to_string(), quoted });
            return Ok(());
        }
        // `${!N}` — indirect through a numeric positional source (e.g.
        // `${!2}`, `${!1-default}`). The source name is a positional
        // parameter reference; `scan_braced_name` rejects digit-leading
        // names, so read the run of digits directly here. Positionals
        // cannot be subscripted, so the subscript is `None`.
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
            return dispatch_braced_modifier(name, quoted, None, chars, parts, /* indirect */ true, opts, dollar_start);
        }
        // `${!X}` where X is a special parameter immediately followed by `}`:
        // special-parameter indirect. `${!#}` = indirect of `$#`, `${!*}` /
        // `${!@}` = indirect of `$*` / `$@`, `${!?}` = indirect of `$?`. This
        // MUST run before the empty-name guard and the M1 prefix lookahead
        // below, so `${!*}` / `${!@}` route to indirect (bash: empty when
        // unset, "invalid variable name" when set) rather than a bad-subst or
        // a prefix-name expansion.
        //
        // The valid set is exactly `# @ * ?` (verified against bash 5.x).
        // `$` and `!` are bad-substs (`${!$}` / `${!!}`); `-` and `+` are
        // operator introducers (`${!-}` / `${!-x}` / `${!+x}` parse `-`/`+`
        // as use-default/use-alternate on an empty indirect ref, NOT as the
        // special param `$-`), so all four fall through to the empty-name
        // guard / operator paths below.
        if matches!(chars.peek().copied(), Some('#' | '@' | '*' | '?')) {
            let mut look = chars.clone();
            let c = look.next().unwrap();
            if look.peek() == Some(&'}') {
                chars.next(); // consume the special-parameter char
                return dispatch_braced_modifier(c.to_string(), quoted, None, chars, parts, /* indirect */ true, opts, dollar_start);
            }
        }
        let name = scan_braced_name(chars)?;
        if name.is_empty() {
            // e.g. `${!+foo}` or `${!-default}` — bad substitution at runtime.
            return recover_bad_subst(chars, parts, quoted, dollar_start);
        }
        // `${!prefix*}` / `${!prefix@}` — prefix-name expansion. Distinguish
        // `*}`/`@}` (prefix form) from `@OP}` (a transform on an indirect
        // ref): only a `*`/`@` IMMEDIATELY followed by `}` is the prefix form.
        // Clone the cursor for a one-char lookahead past the `*`/`@`.
        match chars.peek().copied() {
            Some(c @ ('*' | '@')) => {
                let mut look = chars.clone();
                look.next(); // skip the `*`/`@`
                if look.peek() == Some(&'}') {
                    chars.next(); // consume `*`/`@`
                    chars.next(); // consume `}`
                    parts.push(WordPart::ParamExpansion {
                        name,
                        modifier: ParamModifier::PrefixNames { at: c == '@' },
                        quoted,
                        subscript: None,
                        indirect: false,
                    });
                    return Ok(());
                }
            }
            _ => {}
        }
        let subscript = scan_param_subscript(chars, opts)?;
        match subscript {
            Some(SubscriptKind::All) | Some(SubscriptKind::Star) => {
                // `${!arr[@]}` / `${!arr[*]}` with NOTHING after `]` is the
                // array-KEYS operator. With a trailing operator it is instead
                // INDIRECT expansion through `${arr[@]}`'s value, then the
                // operator (bash) — route that through dispatch_braced_modifier
                // exactly like the scalar-subscript `_` arm below.
                if chars.peek() == Some(&'}') {
                    chars.next(); // consume `}`
                    parts.push(WordPart::ParamExpansion {
                        name,
                        modifier: ParamModifier::IndirectKeys,
                        quoted,
                        subscript,
                        indirect: false,
                    });
                    return Ok(());
                }
                return dispatch_braced_modifier(name, quoted, subscript, chars, parts, /* indirect */ true, opts, dollar_start);
            }
            _ => {
                // `${!NAME}` / `${!NAME-word}` / `${!NAME[i]}` — indirect
                // scalar expansion (v95): resolve NAME's value to a name,
                // then expand that (with any trailing modifier). The name +
                // (non-`[@]`/`[*]`) subscript are already read/scanned here.
                return dispatch_braced_modifier(name, quoted, subscript, chars, parts, /* indirect */ true, opts, dollar_start);
            }
        }
    }

    let name = scan_braced_name(chars)?;
    if name.is_empty() {
        // `${}` (truly empty) or `${+foo}` etc. — bad substitution at runtime.
        return recover_bad_subst(chars, parts, quoted, dollar_start);
    }
    // Optional subscript: `${a[…]}`, `${a[@]}`, `${a[*]}`.
    let subscript = scan_param_subscript(chars, opts)?;
    dispatch_braced_modifier(name, quoted, subscript, chars, parts, false, opts, dollar_start)
}

/// Scans an optional `[…]` subscript immediately after the parameter name
/// inside a `${…}` form. Returns `None` if the next char isn't `[`.
/// Special sigils `@` and `*` produce `SubscriptKind::All` / `Star`;
/// any other expression is parsed via `scan_subscript` into
/// `SubscriptKind::Index`.
fn scan_param_subscript(
    chars: &mut CharCursor<'_>,
    opts: LexerOptions,
) -> Result<Option<SubscriptKind>, LexError> {
    if chars.peek() != Some(&'[') {
        return Ok(None);
    }
    chars.next(); // consume '['
    match chars.peek().copied() {
        Some('@') | Some('*') => {
            let sigil = chars.next().unwrap();
            if chars.next() != Some(']') {
                return Err(LexError::UnterminatedSubscript);
            }
            Ok(Some(if sigil == '@' {
                SubscriptKind::All
            } else {
                SubscriptKind::Star
            }))
        }
        _ => {
            let inner = scan_subscript(chars, opts)?;
            Ok(Some(SubscriptKind::Index(inner)))
        }
    }
}

/// Scans `expr]` and returns the Word inside the brackets. Caller has
/// already consumed the leading `[`. Balanced over nested `[…]` (for
/// arith-style expressions like `a[$((i+1))]`).
fn scan_subscript(
    chars: &mut CharCursor<'_>,
    opts: LexerOptions,
) -> Result<Word, LexError> {
    let mut depth: usize = 1;
    let mut buf = String::new();
    while let Some(&c) = chars.peek() {
        match c {
            '[' => {
                depth += 1;
                buf.push(c);
                chars.next();
            }
            ']' => {
                chars.next();
                depth -= 1;
                if depth == 0 {
                    // Re-tokenize the subscript body so embedded
                    // expansions (`$i`, `${j}`, `$((n))`) are honoured.
                    return parse_subscript_body(&buf, opts);
                }
                buf.push(c);
            }
            _ => {
                buf.push(c);
                chars.next();
            }
        }
    }
    Err(LexError::UnterminatedSubscript)
}

/// Re-tokenizes the inside of a `[…]` subscript as a single Word. If
/// `tokenize` returns more or fewer than one Word token, falls back to
/// a single unquoted Literal containing the raw text (which is exactly
/// what arithmetic evaluation will see).
fn parse_subscript_body(src: &str, opts: LexerOptions) -> Result<Word, LexError> {
    let toks = tokenize_with_opts(src, opts)?;
    let mut words: Vec<Word> = Vec::new();
    for t in toks {
        if let Token::Word(w) = t {
            words.push(w);
        }
    }
    if words.len() == 1 {
        return Ok(words.pop().unwrap());
    }
    // Multi-word or empty: collapse into a single Literal containing
    // the raw text. Arithmetic evaluation tolerates spaces in numeric
    // expressions; literal-name lookups still see the joined text.
    Ok(Word(vec![WordPart::Literal {
        text: src.to_string(),
        quoted: false,
    }]))
}

/// Consumes any run of `\`-newline line continuations at the cursor (POSIX
/// 2.2.1: `\<NL>` is deleted before tokenizing). Uses a cloned-cursor 2-char
/// lookahead so a `\` NOT followed by a newline (a real escape like `\x`) is
/// left untouched. No-op when the cursor is not at a `\<NL>`.
fn skip_line_continuations(chars: &mut CharCursor<'_>) {
    loop {
        let mut probe = chars.clone();
        if probe.next() == Some('\\') && probe.next() == Some('\n') {
            *chars = probe;
        } else {
            return;
        }
    }
}

/// Scans a compound array RHS `elem elem [idx]=elem … )`. The caller has
/// already consumed the leading `(`. Whitespace and newlines separate
/// elements; quoting, command substitution `$(…)`, and `${…}` interiors
/// are all preserved verbatim and re-tokenized into a single Word per
/// element. Subscripted elements `[expr]=value` carry an explicit
/// `subscript` Word.
fn scan_array_literal(
    chars: &mut CharCursor<'_>,
    opts: LexerOptions,
) -> Result<Vec<ArrayLiteralElement>, LexError> {
    let mut elements: Vec<ArrayLiteralElement> = Vec::new();
    loop {
        // Skip inter-element separators: whitespace, newlines, and comments.
        skip_array_literal_separators(chars);
        match chars.peek() {
            Some(&')') => {
                chars.next();
                return Ok(elements);
            }
            None => return Err(LexError::UnterminatedArrayLiteral),
            _ => {}
        }
        // Optional explicit `[expr]=`.
        let subscript = if chars.peek() == Some(&'[') {
            chars.next(); // consume '['
            let sub = scan_subscript(chars, opts)?;
            if chars.next() != Some('=') {
                return Err(LexError::ArrayLiteralMissingEquals);
            }
            Some(sub)
        } else {
            None
        };
        let value = scan_array_element_word(chars, opts)?;
        match subscript {
            // Subscripted `[i]=value` keeps single-value semantics (brace stays
            // literal — matches bash for associative subscripts; the indexed
            // `[i]=val{brace}` edge is a documented low-impact divergence).
            Some(sub) => {
                elements.push(ArrayLiteralElement { subscript: Some(sub), value });
            }
            // Bare elements brace-expand (textual, first) into N elements; the
            // executor then word-splits/globs each. Reuses the command-word path.
            None => {
                for p in brace_expand_parts(value.0)? {
                    elements.push(ArrayLiteralElement { subscript: None, value: Word(p) });
                }
            }
        }
    }
}

/// Consumes a `#` line comment's body up to (but NOT including) the terminating
/// newline; the caller's loop handles the newline. The opening `#` must already
/// be confirmed as a comment-start (word boundary) by the caller.
fn skip_line_comment(chars: &mut CharCursor<'_>) {
    while let Some(&c) = chars.peek() {
        if c == '\n' {
            break;
        }
        chars.next();
    }
}

/// Skips inter-element separators inside an array literal: whitespace, newlines,
/// `\<NL>` line continuations, and `#` comments. The post-skip position is always
/// an element boundary (after `(` or inter-element whitespace), so a `#` here is
/// unambiguously a comment — its body (incl. any `)`) must NOT be read as
/// elements or close the literal. A `\<NL>` between elements (`[a]=1 \<NL>
/// [b]=2`) is a line continuation, not the start of an element value.
fn skip_array_literal_separators(
    chars: &mut CharCursor<'_>,
) {
    loop {
        while let Some(&c) = chars.peek() {
            if c.is_whitespace() {
                chars.next();
            } else {
                break;
            }
        }
        // `\<NL>` continuation (only consumes a backslash IMMEDIATELY followed by
        // a newline — a real escape like `\x` starting an element is left alone).
        let before = chars.offset();
        skip_line_continuations(chars);
        if chars.offset() != before {
            continue; // consumed a continuation — re-check for more separators
        }
        if chars.peek() == Some(&'#') {
            skip_line_comment(chars);
        } else {
            break;
        }
    }
}

/// Scans a single array-element word (terminated by unquoted whitespace
/// or unquoted `)`). Honours `"…"`, `'…'`, `$'…'`, `$(…)`, `\…`, and
/// nested `${…}` so closing `)` inside command substitutions doesn't
/// end the array literal prematurely. The collected raw text is then
/// re-tokenized via `tokenize` to produce a `Word`.
fn scan_array_element_word(
    chars: &mut CharCursor<'_>,
    opts: LexerOptions,
) -> Result<Word, LexError> {
    let mut buf = String::new();
    loop {
        let c = match chars.peek().copied() {
            Some(c) => c,
            None => return Err(LexError::UnterminatedArrayLiteral),
        };
        match c {
            ')' => break,
            c if c.is_whitespace() => break,
            '\'' => {
                buf.push(c);
                chars.next();
                loop {
                    match chars.next() {
                        Some('\'') => {
                            buf.push('\'');
                            break;
                        }
                        Some(ch) => buf.push(ch),
                        None => return Err(LexError::UnterminatedQuote),
                    }
                }
            }
            '"' => {
                buf.push(c);
                chars.next();
                loop {
                    match chars.next() {
                        Some('"') => {
                            buf.push('"');
                            break;
                        }
                        Some('\\') => {
                            buf.push('\\');
                            match chars.next() {
                                Some(esc) => buf.push(esc),
                                None => return Err(LexError::UnterminatedQuote),
                            }
                        }
                        Some(ch) => buf.push(ch),
                        None => return Err(LexError::UnterminatedQuote),
                    }
                }
            }
            '\\' => {
                buf.push(c);
                chars.next();
                if let Some(next) = chars.next() {
                    buf.push(next);
                }
            }
            '$' => {
                buf.push('$');
                chars.next();
                match chars.peek().copied() {
                    Some('(') => {
                        buf.push('(');
                        chars.next();
                        scan_cmdsub_body(chars, &mut buf, LexError::UnterminatedSubstitution)?;
                        buf.push(')');
                    }
                    Some('{') => {
                        buf.push('{');
                        chars.next();
                        let mut depth: usize = 1;
                        for ch in chars.by_ref() {
                            buf.push(ch);
                            match ch {
                                '{' => depth += 1,
                                '}' => {
                                    depth -= 1;
                                    if depth == 0 {
                                        break;
                                    }
                                }
                                _ => {}
                            }
                        }
                        if depth != 0 {
                            return Err(LexError::UnterminatedBrace);
                        }
                    }
                    _ => {}
                }
            }
            '`' => {
                buf.push('`');
                chars.next();
                consume_backtick_verbatim(chars, &mut buf)?;
            }
            _ => {
                buf.push(c);
                chars.next();
            }
        }
    }
    // Re-tokenize the collected text as a single Word. Brace expansion is
    // suppressed here so an element like `x{1,2}$v` stays ONE Word with the
    // brace as literal text and `$v` as a real expansion part; the brace is
    // expanded later by `brace_expand_parts` in `scan_array_literal`, which
    // sentinel-protects the expansion.
    let toks = tokenize_no_brace(&buf, opts)?;
    let mut words: Vec<Word> = Vec::new();
    for t in toks {
        if let Token::Word(w) = t {
            words.push(w);
        }
    }
    if words.len() == 1 {
        Ok(words.pop().unwrap())
    } else if words.is_empty() {
        Ok(Word(vec![WordPart::Literal {
            text: String::new(),
            quoted: false,
        }]))
    } else {
        // Multi-word: collapse into a single Literal (rare; would mean
        // unquoted brace expansion or similar inside the element).
        Ok(Word(vec![WordPart::Literal {
            text: buf,
            quoted: false,
        }]))
    }
}

/// Reads identifier chars (the parameter name) inside a `${...}` until it
/// hits a non-identifier char. Does NOT consume the terminator.
/// The special single-char parameter names that may appear as the operand
/// of the length (`${#X}`) form. (`@`/`*` are handled separately in the
/// Length path because they mean "arg count", not "length of the special
/// param's value".) For the indirect (`${!X}`) form a narrower set is used
/// inline — see `scan_braced_param_expansion` — because bash bad-substs
/// `${!$}` and `${!!}`.
fn special_param_char(c: char) -> bool {
    matches!(c, '#' | '@' | '*' | '$' | '!' | '?' | '-')
}

fn scan_braced_name(
    chars: &mut CharCursor<'_>,
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
/// `}`, `:`, `-`, `=`, `?`, `+`, `#`, `%`, or `/`). `subscript` is
/// `Some(...)` when the name was followed by `[...]` (already scanned).
/// `dollar_start` is the byte offset of the leading `$` (for bad-subst
/// recovery). Pushes a single `WordPart` (`Var` or `ParamExpansion`) onto `parts`.
fn dispatch_braced_modifier(
    name: String,
    quoted: bool,
    subscript: Option<SubscriptKind>,
    chars: &mut CharCursor<'_>,
    parts: &mut Vec<WordPart>,
    indirect: bool,
    opts: LexerOptions,
    dollar_start: usize,
) -> Result<(), LexError> {
    match chars.next() {
        Some('}') => {
            if subscript.is_some() {
                // `${a[i]}` / `${a[@]}` / `${a[*]}` — no scalar-style
                // modifier. Emit `ParamModifier::None`, a no-op marker.
                // (We can't reuse `UseDefault { word: empty }`: that
                // would be semantically `${a[i]-}` — silently substitute
                // "" on unset — which suppresses the array-expansion
                // path's ability to distinguish unset elements.) Task 3
                // dispatches the array lookup via the `subscript` field.
                parts.push(WordPart::ParamExpansion {
                    name,
                    modifier: ParamModifier::None,
                    quoted,
                    subscript,
                    indirect,
                });
                return Ok(());
            }
            if indirect {
                // Bare `${!ref}` — indirect scalar expansion with no
                // trailing modifier. Emit `ParamModifier::None` so the
                // eval `indirect` branch resolves the through-value.
                parts.push(WordPart::ParamExpansion {
                    name,
                    modifier: ParamModifier::None,
                    quoted,
                    subscript,
                    indirect: true,
                });
                return Ok(());
            }
            parts.push(WordPart::Var { name, quoted });
            Ok(())
        }
        Some(':') => {
            match chars.peek().copied() {
                Some('-') => {
                    chars.next();
                    let modifier = modifier_with_operand(chars, quoted, opts, |w| ParamModifier::UseDefault { word: w, colon: true })?;
                    parts.push(WordPart::ParamExpansion { name, modifier, quoted, subscript, indirect });
                    Ok(())
                }
                Some('=') => {
                    chars.next();
                    let modifier = modifier_with_operand(chars, quoted, opts, |w| ParamModifier::AssignDefault { word: w, colon: true })?;
                    parts.push(WordPart::ParamExpansion { name, modifier, quoted, subscript, indirect });
                    Ok(())
                }
                Some('?') => {
                    chars.next();
                    let modifier = modifier_with_operand(chars, false, opts, |w| ParamModifier::ErrorIfUnset { word: w, colon: true })?;
                    parts.push(WordPart::ParamExpansion { name, modifier, quoted, subscript, indirect });
                    Ok(())
                }
                Some('+') => {
                    chars.next();
                    let modifier = modifier_with_operand(chars, quoted, opts, |w| ParamModifier::UseAlternate { word: w, colon: true })?;
                    parts.push(WordPart::ParamExpansion { name, modifier, quoted, subscript, indirect });
                    Ok(())
                }
                Some('}') => recover_bad_subst(chars, parts, quoted, dollar_start),
                Some(_) => {
                    let (offset, length) = scan_substring_operands(chars, opts)?;
                    parts.push(WordPart::ParamExpansion {
                        name,
                        modifier: ParamModifier::Substring { offset, length },
                        quoted,
                        subscript,
                        indirect,
                    });
                    Ok(())
                }
                None => Err(LexError::UnterminatedBrace),
            }
        }
        Some('-') => {
            let modifier = modifier_with_operand(chars, quoted, opts, |w| ParamModifier::UseDefault { word: w, colon: false })?;
            parts.push(WordPart::ParamExpansion { name, modifier, quoted, subscript, indirect });
            Ok(())
        }
        Some('=') => {
            let modifier = modifier_with_operand(chars, quoted, opts, |w| ParamModifier::AssignDefault { word: w, colon: false })?;
            parts.push(WordPart::ParamExpansion { name, modifier, quoted, subscript, indirect });
            Ok(())
        }
        Some('?') => {
            let modifier = modifier_with_operand(chars, false, opts, |w| ParamModifier::ErrorIfUnset { word: w, colon: false })?;
            parts.push(WordPart::ParamExpansion { name, modifier, quoted, subscript, indirect });
            Ok(())
        }
        Some('+') => {
            let modifier = modifier_with_operand(chars, quoted, opts, |w| ParamModifier::UseAlternate { word: w, colon: false })?;
            parts.push(WordPart::ParamExpansion { name, modifier, quoted, subscript, indirect });
            Ok(())
        }
        Some('#') => {
            let longest = chars.peek() == Some(&'#');
            if longest { chars.next(); }
            let modifier = modifier_with_operand(chars, false, opts, |w| ParamModifier::RemovePrefix { pattern: w, longest })?;
            parts.push(WordPart::ParamExpansion { name, modifier, quoted, subscript, indirect });
            Ok(())
        }
        Some('%') => {
            let longest = chars.peek() == Some(&'%');
            if longest { chars.next(); }
            let modifier = modifier_with_operand(chars, false, opts, |w| ParamModifier::RemoveSuffix { pattern: w, longest })?;
            parts.push(WordPart::ParamExpansion { name, modifier, quoted, subscript, indirect });
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
            let (pattern, replacement) = scan_substitution_operand(chars, opts)?;
            parts.push(WordPart::ParamExpansion {
                name,
                modifier: ParamModifier::Substitute { pattern, replacement, anchor, all },
                quoted,
                subscript,
                indirect,
            });
            Ok(())
        }
        Some('^') => {
            let all = chars.peek() == Some(&'^');
            if all { chars.next(); }
            let pattern = scan_optional_braced_operand(chars, opts)?;
            parts.push(WordPart::ParamExpansion {
                name,
                modifier: ParamModifier::Case { direction: CaseDirection::Upper, all, pattern },
                quoted,
                subscript,
                indirect,
            });
            Ok(())
        }
        Some(',') => {
            let all = chars.peek() == Some(&',');
            if all { chars.next(); }
            let pattern = scan_optional_braced_operand(chars, opts)?;
            parts.push(WordPart::ParamExpansion {
                name,
                modifier: ParamModifier::Case { direction: CaseDirection::Lower, all, pattern },
                quoted,
                subscript,
                indirect,
            });
            Ok(())
        }
        Some('@') => {
            // `${V@}` with no op letter — bad substitution at runtime.
            if chars.peek() == Some(&'}') {
                return recover_bad_subst(chars, parts, quoted, dollar_start);
            }
            let op = match chars.next() {
                Some('P') => TransformOp::PromptExpand,
                Some('Q') => TransformOp::Quote,
                Some('U') => TransformOp::Upper,
                Some('L') => TransformOp::Lower,
                Some('u') => TransformOp::UpperFirst,
                Some('E') => TransformOp::EscapeExpand,
                Some('A') => TransformOp::AssignDecl,
                Some('K') => TransformOp::KvString,
                Some('k') => TransformOp::KvWords,
                Some('a') => TransformOp::AttrFlags,
                _other => {
                    // Unknown or missing op letter — bad substitution at runtime.
                    // One char has already been consumed; scan_braced_operand will
                    // continue from here to the matching `}`.
                    return recover_bad_subst(chars, parts, quoted, dollar_start);
                }
            };
            // After the operator letter, the next char must close the brace.
            match chars.next() {
                Some('}') => {
                    parts.push(WordPart::ParamExpansion {
                        name,
                        modifier: ParamModifier::Transform { op },
                        quoted,
                        subscript,
                        indirect,
                    });
                    Ok(())
                }
                _ => Err(LexError::UnterminatedBrace),
            }
        }
        Some(c) => {
            // Unknown modifier character — bad substitution at runtime.
            // `c` was already consumed by `chars.next()` at the top of this match;
            // `recover_bad_subst` will scan from here to the matching `}`.
            let _ = c;
            recover_bad_subst(chars, parts, quoted, dollar_start)
        }
        None => Err(LexError::UnterminatedBrace),
    }
}

/// Scans the operand text until the matching `}` and parses it as a single
/// `Word`. Builds the `ParamModifier` via the caller's closure.
fn modifier_with_operand<F>(
    chars: &mut CharCursor<'_>,
    enclosing_dquote: bool,
    opts: LexerOptions,
    build: F,
) -> Result<ParamModifier, LexError>
where
    F: FnOnce(Word) -> ParamModifier,
{
    let body = scan_braced_operand(chars)?;
    let word = parse_braced_operand_opts(&body, enclosing_dquote, opts)?;
    Ok(build(word))
}

/// Scans a single optional operand inside a `${name<mod>OPERAND}` form.
/// Returns `None` if the operand body is empty (i.e. the modifier is
/// immediately followed by `}`), or `Some(Word)` for a non-empty body.
/// Delegates to `scan_braced_operand` (depth + quote aware) so nested
/// `${...}` constructs in the operand are handled correctly.
fn scan_optional_braced_operand(
    chars: &mut CharCursor<'_>,
    opts: LexerOptions,
) -> Result<Option<Word>, LexError> {
    let body = scan_braced_operand(chars)?;
    if body.is_empty() {
        Ok(None)
    } else {
        Ok(Some(parse_braced_operand_opts(&body, false, opts)?))
    }
}

/// Walks the chars iterator from just after the leading `/` of a
/// substitution operand. Delegates to `scan_braced_operand` to collect the
/// raw body (which depth-tracks nested `${...}` and protects `}` inside
/// quoted spans), then splits pattern from replacement on the first
/// unescaped `/` at brace-depth zero outside any quoted span. `\/` becomes
/// a literal `/`; `\\` becomes a literal `\`; any other `\x` passes
/// through unchanged so the inner operand tokenizer sees it.
fn scan_substitution_operand(
    chars: &mut CharCursor<'_>,
    opts: LexerOptions,
) -> Result<(Word, Word), LexError> {
    let body = scan_braced_operand(chars)?;
    let (pattern_src, replacement_src) = split_substitution_body(&body);
    let pattern = parse_braced_operand_opts(&pattern_src, false, opts)?;
    let replacement = parse_braced_operand_opts(&replacement_src, false, opts)?;
    Ok((pattern, replacement))
}

/// Splits a `${…}` modifier operand body on the FIRST top-level `delim`,
/// returning `(before, Some(after))` if a top-level delimiter was found, or
/// `(before, None)` otherwise. "Top level" excludes single quotes, double
/// quotes, backticks, a `$(…)` command substitution (nested parens — also
/// covers `$((…))` and `$( (…) )`), and `{…}` braces. Skipped spans are
/// appended VERBATIM so the segments re-parse exactly as written. A backslash
/// escape `\x` is ALSO preserved verbatim (and the escaped char consumed, so an
/// escaped delimiter `\delim` does not split and an escaped quote `\"` does not
/// open a span); all un-escaping is done once, downstream, by
/// `parse_braced_operand_opts`. Inside a command substitution escapes are
/// verbatim too (they belong to the command), mirroring `scan_paren_substitution`.
fn split_modifier_operand(body: &str, delim: char) -> (String, Option<String>) {
    let mut first = String::new();
    let mut second = String::new();
    let mut delim_seen = false;
    let mut brace_depth: u32 = 0; // { } nesting
    let mut chars = CharCursor::new(body);
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                // Preserve an escaped char VERBATIM (backslash + the char) and
                // CONSUME the char so it cannot act as a delimiter or open a
                // quote/backtick span. The real un-escaping happens once,
                // downstream, in parse_braced_operand_opts; pre-un-escaping here
                // would double-process backslashes (corrupting runs like `\\\"`).
                // An escaped delimiter (`\/`) is thus preserved AND not seen as a
                // split point. A trailing `\` at end of body pushes just `\`.
                let dst = if delim_seen { &mut second } else { &mut first };
                dst.push('\\');
                if let Some(nc) = chars.next() {
                    dst.push(nc);
                }
            }
            '\'' => {
                let dst = if delim_seen { &mut second } else { &mut first };
                dst.push('\'');
                for qc in chars.by_ref() {
                    dst.push(qc);
                    if qc == '\'' {
                        break;
                    }
                }
            }
            '"' => {
                let dst = if delim_seen { &mut second } else { &mut first };
                dst.push('"');
                while let Some(qc) = chars.next() {
                    dst.push(qc);
                    if qc == '\\' {
                        if let Some(nc) = chars.next() {
                            dst.push(nc);
                        }
                    } else if qc == '"' {
                        break;
                    }
                }
            }
            '`' => {
                let dst = if delim_seen { &mut second } else { &mut first };
                dst.push('`');
                // Skip the backtick command substitution verbatim. The Result is
                // unreachable (operand body pre-balanced by scan_braced_operand);
                // on the impossible EOF the closing backtick is not re-added and
                // the cursor is exhausted, so the loop ends with identical segments.
                let _ = consume_backtick_verbatim(&mut chars, dst);
            }
            '$' => {
                let dst = if delim_seen { &mut second } else { &mut first };
                dst.push('$');
                if chars.peek() == Some(&'(') {
                    chars.next();
                    dst.push('(');
                    // Skip the whole command substitution verbatim so a delimiter
                    // inside it is ignored (L-10). The `Result` is unreachable: the
                    // operand body was already $()-balanced by scan_braced_operand;
                    // on the impossible error the partial is appended and the cursor
                    // is exhausted, so the outer loop ends with identical segments.
                    let _ = consume_paren_cmdsub_verbatim(&mut chars, dst);
                }
            }
            '{' => {
                brace_depth += 1;
                if delim_seen { second.push('{'); } else { first.push('{'); }
            }
            '}' => {
                brace_depth = brace_depth.saturating_sub(1);
                if delim_seen { second.push('}'); } else { first.push('}'); }
            }
            c if c == delim && brace_depth == 0 && !delim_seen => {
                delim_seen = true;
            }
            _ => {
                if delim_seen { second.push(c); } else { first.push(c); }
            }
        }
    }
    if delim_seen {
        (first, Some(second))
    } else {
        (first, None)
    }
}

/// Splits a `${var/pat/repl}` operand body into `(pattern, replacement)` on the
/// first top-level `/` (skipping command substitutions / quotes / braces — see
/// `split_modifier_operand`). A missing replacement (`${var/pat}`) yields `""`,
/// matching bash's treatment of `${var/pat}` as `${var/pat/}`.
fn split_substitution_body(body: &str) -> (String, String) {
    let (pattern, replacement) = split_modifier_operand(body, '/');
    (pattern, replacement.unwrap_or_default())
}

/// Scans a `${var:offset}` / `${var:offset:length}` operand pair. Delegates
/// to `scan_braced_operand` + `split_substring_body` + `parse_braced_operand`
/// to collect and parse the offset and optional length Words.
fn scan_substring_operands(
    chars: &mut CharCursor<'_>,
    opts: LexerOptions,
) -> Result<(Word, Option<Word>), LexError> {
    let body = scan_braced_operand(chars)?;
    let (offset_src, length_src) = split_substring_body(&body);
    let offset = parse_braced_operand_opts(&offset_src, false, opts)?;
    let length = match length_src {
        Some(s) => Some(parse_braced_operand_opts(&s, false, opts)?),
        None => None,
    };
    Ok((offset, length))
}

/// Splits a substring-operand body (as returned by `scan_braced_operand`)
/// on the first unescaped `:` that sits at brace-depth zero outside any
/// quoted span. Returns `(offset_src, Some(length_src))` if a delimiter
/// was found, or `(offset_src, None)` otherwise (the no-length form).
fn split_substring_body(body: &str) -> (String, Option<String>) {
    split_modifier_operand(body, ':')
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
    chars: &mut CharCursor<'_>,
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

/// 1-based line number of byte offset `off` within `src`
/// (1 + the count of '\n' bytes before `off`). Clamps `off` to `src.len()`.
/// Used in tests and for isolated single-offset lookups.
#[allow(dead_code)]
pub fn line_at_offset(src: &str, off: usize) -> u32 {
    1 + src.as_bytes()[..off.min(src.len())].iter().filter(|&&b| b == b'\n').count() as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::RedirFd;

    #[test]
    fn split_modifier_operand_basic_split() {
        assert_eq!(split_modifier_operand("a/b", '/'), ("a".into(), Some("b".into())));
        assert_eq!(split_modifier_operand("a", '/'), ("a".into(), None));
        assert_eq!(split_modifier_operand("2:3", ':'), ("2".into(), Some("3".into())));
        assert_eq!(split_modifier_operand("2", ':'), ("2".into(), None));
    }

    #[test]
    fn split_modifier_operand_skips_command_sub() {
        // A delimiter inside $(...) is NOT the split point (L-10).
        assert_eq!(
            split_modifier_operand("$(echo a/x)/Z", '/'),
            ("$(echo a/x)".into(), Some("Z".into()))
        );
        assert_eq!(
            split_modifier_operand("$(echo 1:2)", ':'),
            ("$(echo 1:2)".into(), None)
        );
        // Nested $( $() ).
        assert_eq!(
            split_modifier_operand("$(echo $(echo a/b))/Q", '/'),
            ("$(echo $(echo a/b))".into(), Some("Q".into()))
        );
        // $(( ... )) arithmetic with a ternary colon inside.
        assert_eq!(
            split_modifier_operand("$((1>0?2:3))", ':'),
            ("$((1>0?2:3))".into(), None)
        );
    }

    #[test]
    fn split_modifier_operand_skips_backtick() {
        assert_eq!(
            split_modifier_operand("`echo a/x`/Z", '/'),
            ("`echo a/x`".into(), Some("Z".into()))
        );
    }

    #[test]
    fn split_modifier_operand_quotes_and_escapes() {
        // A quoted delimiter is kept verbatim and does not split.
        assert_eq!(
            split_modifier_operand("\"a/b\"/x", '/'),
            ("\"a/b\"".into(), Some("x".into()))
        );
        // An escaped delimiter is preserved VERBATIM and does not split
        // (downstream parse_braced_operand_opts un-escapes `\/`→`/`).
        assert_eq!(split_modifier_operand("a\\/b/x", '/'), ("a\\/b".into(), Some("x".into())));
        // `\\` is preserved verbatim (un-escaped once, downstream).
        assert_eq!(split_modifier_operand("a\\\\b", '/'), ("a\\\\b".into(), None));
        // Regression: `\\\"` (escaped backslash + escaped quote) must not let the
        // `"` open a span that swallows the delimiter. Body `\\\"/Z` (Rust
        // literal `"\\\\\\\"/Z"`) splits to pattern `\\\"` and replacement `Z`.
        assert_eq!(
            split_modifier_operand("\\\\\\\"/Z", '/'),
            ("\\\\\\\"".into(), Some("Z".into()))
        );
    }

    #[test]
    fn split_modifier_operand_brace_nesting() {
        // A delimiter inside ${...} plain nesting is not the split point.
        assert_eq!(split_modifier_operand("${x:-y}", ':'), ("${x:-y}".into(), None));
    }

    /// True iff `tokens` contains at least one `Token::RedirFd`.
    fn has_redir_fd(tokens: &[Token]) -> bool {
        tokens.iter().any(|t| matches!(t, Token::RedirFd(_)))
    }

    #[test]
    fn lexer_fd_prefix_numeric() {
        // `echo 2>&1`: the `2` is whitespace-separated from `echo`, glued to the
        // operator — but the dedicated `2>` scanner fires (DupErr) and encodes
        // fd 2 in the operator itself, so NO RedirFd token is emitted here.
        // Use an fd with no dedicated scanner (`3>`) to exercise take_fd_prefix.
        let toks = tokenize("echo 3>file").unwrap();
        assert!(
            toks.iter().any(|t| matches!(t, Token::RedirFd(RedirFd::Number(3)))),
            "expected RedirFd(Number(3)) in {toks:?}"
        );
        // And `echo 12>file` → fd 12.
        let toks = tokenize("echo 12>file").unwrap();
        assert!(
            toks.iter().any(|t| matches!(t, Token::RedirFd(RedirFd::Number(12)))),
            "expected RedirFd(Number(12)) in {toks:?}"
        );
    }

    #[test]
    fn lexer_fd_prefix_space_is_not_prefix() {
        // `echo 3 >file`: a space separates `3` from `>` — the `3` stays a Word
        // argument and NO RedirFd is emitted.
        let toks = tokenize("echo 3 >file").unwrap();
        assert!(!has_redir_fd(&toks), "unexpected RedirFd in {toks:?}");
        // The `3` survives as a Word arg.
        assert!(
            toks.iter().any(|t| matches!(t, Token::Word(w) if crate::command::word_literal_text(w) == Some("3"))),
            "expected Word(\"3\") arg in {toks:?}"
        );
    }

    #[test]
    fn lexer_fd_prefix_glued_word_is_not_prefix() {
        // `file2>x`: `file2` is not all-digits, so no RedirFd; `file2` stays a Word.
        let toks = tokenize("file2>x").unwrap();
        assert!(!has_redir_fd(&toks), "unexpected RedirFd in {toks:?}");
        assert!(
            toks.iter().any(|t| matches!(t, Token::Word(w) if crate::command::word_literal_text(w) == Some("file2"))),
            "expected Word(\"file2\") in {toks:?}"
        );
    }

    #[test]
    fn lexer_named_fd_prefix() {
        // `exec {fd}>log`: `{fd}` glued to `>` → RedirFd::Var("fd").
        let toks = tokenize("exec {fd}>log").unwrap();
        assert!(
            toks.iter().any(|t| matches!(t, Token::RedirFd(RedirFd::Var(name)) if name == "fd")),
            "expected RedirFd(Var(\"fd\")) in {toks:?}"
        );
    }

    #[test]
    fn lexer_readwrite_and_dupin_operators() {
        let toks = tokenize("cmd <>f").unwrap();
        assert!(toks.iter().any(|t| matches!(t, Token::Op(Operator::RedirReadWrite))));
        let toks = tokenize("cmd 3<&0").unwrap();
        assert!(toks.iter().any(|t| matches!(t, Token::RedirFd(RedirFd::Number(3)))));
        assert!(toks.iter().any(|t| matches!(t, Token::Op(Operator::DupIn))));
    }

    #[test]
    fn line_at_offset_counts_newlines() {
        let s = "a\nbb\nccc";
        assert_eq!(line_at_offset(s, 0), 1);   // 'a'
        assert_eq!(line_at_offset(s, 2), 2);   // first 'b'
        assert_eq!(line_at_offset(s, 5), 3);   // first 'c'
        assert_eq!(line_at_offset(s, 999), 3); // clamped
    }

    #[test]
    fn char_cursor_tracks_line() {
        let mut c = CharCursor::new("a\nbb\nc");
        assert_eq!(c.line(), 1);
        assert_eq!(c.next(), Some('a')); assert_eq!(c.line(), 1);
        assert_eq!(c.next(), Some('\n')); assert_eq!(c.line(), 2); // after the newline
        assert_eq!(c.next(), Some('b')); assert_eq!(c.line(), 2);
        c.next(); c.next(); // 'b','\n'
        assert_eq!(c.line(), 3);
        assert_eq!(c.next(), Some('c')); assert_eq!(c.line(), 3);
    }

    /// Builds a Token that holds a single-Literal Word.
    fn w(s: &str) -> Token {
        Token::Word(Word(vec![WordPart::Literal { text: s.to_string(), quoted: false }]))
    }

    fn word_text(t: &Token) -> Option<String> {
        if let Token::Word(Word(parts)) = t
            && parts.len() == 1
            && let WordPart::Literal { text, quoted: false } = &parts[0]
        {
            return Some(text.clone());
        }
        None
    }

    #[test]
    fn extglob_inside_command_sub_lexes() {
        let opts = LexerOptions { extglob: true };
        let toks = tokenize_with_opts("echo $(echo !(x))", opts).unwrap();
        assert!(toks.iter().any(|t| matches!(
            t, Token::Word(Word(parts)) if parts.iter().any(|p| matches!(p, WordPart::CommandSub { .. }))
        )));
    }

    #[test]
    fn extglob_inside_backtick_sub_lexes() {
        let opts = LexerOptions { extglob: true };
        tokenize_with_opts("echo `echo !(x)`", opts).unwrap();
    }

    #[test]
    fn extglob_inside_array_literal_command_sub_lexes() {
        let opts = LexerOptions { extglob: true };
        tokenize_with_opts("a=($(printf '%s\\n' /tmp/!(x)))", opts).unwrap();
    }

    #[test]
    fn command_sub_without_extglob_still_errors_on_bare_extglob() {
        let opts = LexerOptions { extglob: false };
        assert!(tokenize_with_opts("echo $(echo !(x))", opts).is_err());
    }

    #[test]
    fn plain_command_sub_unchanged() {
        for eg in [false, true] {
            let opts = LexerOptions { extglob: eg };
            tokenize_with_opts("echo $(echo hi) $((1+1))", opts).unwrap();
        }
    }

    #[test]
    fn dbracket_regex_paren_operand_is_one_word() {
        let toks = tokenize("[[ x =~ (a) ]]").unwrap();
        let texts: Vec<_> = toks.iter().filter_map(word_text).collect();
        assert_eq!(texts, vec!["[[", "x", "=~", "(a)", "]]"]);
        assert!(!toks.iter().any(|t| matches!(t, Token::Op(Operator::LParen) | Token::ArithBlock(..))));
    }

    #[test]
    fn dbracket_regex_double_paren_not_arithblock() {
        let toks = tokenize("[[ ab =~ ((a)) ]]").unwrap();
        let texts: Vec<_> = toks.iter().filter_map(word_text).collect();
        assert_eq!(texts, vec!["[[", "ab", "=~", "((a))", "]]"]);
        assert!(!toks.iter().any(|t| matches!(t, Token::ArithBlock(..))));
    }

    #[test]
    fn dbracket_regex_line847_shape() {
        let toks = tokenize(r"[[ $option =~ (\[((no|dont)-?)\]). ]]").unwrap();
        let texts: Vec<_> = toks.iter().filter_map(word_text).collect();
        assert!(texts.iter().any(|t| t.starts_with("(\\[")));
        assert!(texts.contains(&"]]".to_string()));
        assert!(!toks.iter().any(|t| matches!(t, Token::ArithBlock(..))));
    }

    #[test]
    fn dbracket_regex_space_inside_parens_kept() {
        let toks = tokenize("[[ x =~ (a b) ]]").unwrap();
        let texts: Vec<_> = toks.iter().filter_map(word_text).collect();
        assert_eq!(texts, vec!["[[", "x", "=~", "(a b)", "]]"]);
    }

    #[test]
    fn dbracket_regex_operand_after_line_continuation() {
        // bash_completion line 876 shape: the `=~` operand is on a `\`-newline
        // continuation line whose indentation must NOT end the operand empty.
        let toks = tokenize("[[ $x =~ \\\n   (a|b)c ]]").unwrap();
        let texts: Vec<_> = toks.iter().filter_map(word_text).collect();
        // the regex operand is the single word `(a|b)c`, then `]]`.
        assert!(texts.contains(&"(a|b)c".to_string()), "texts: {texts:?}");
        assert!(texts.contains(&"]]".to_string()));
        assert!(!toks.iter().any(|t| matches!(t, Token::ArithBlock(..) | Token::Op(Operator::LParen))));
    }

    #[test]
    fn braced_operand_bare_brace_is_literal() {
        // bash_completion line 849/854: `${var%%[<{(]*}` — a bare `{` in the
        // pattern must not nest the `${...}` (only `${` nests). Previously this
        // raised UnterminatedBrace.
        assert!(tokenize("${x%%[<{(]*}").is_ok());
        assert!(tokenize("${x%%{*}").is_ok());
        // nested ${...} still depth-tracks:
        assert!(tokenize("${x:-${y}}").is_ok());
    }

    #[test]
    fn braced_operand_ansi_c_quote_with_escaped_quote() {
        // `$'a\t\'\tb'` inside the body: the escaped `\'` must NOT terminate
        // the scan, and the trailing `'` is the ANSI-C close, not a new span.
        let toks = tokenize(r#"${x#$'a\t\'\tb'}"#).unwrap();
        // It must tokenize (not error). Exactly one Word token with a single
        // ParamExpansion part (RemovePrefix).
        assert_eq!(toks.len(), 1);
    }

    #[test]
    fn braced_operand_ansi_c_quote_simple() {
        let toks = tokenize(r#"${x#$'f'}"#).unwrap();
        assert_eq!(toks.len(), 1);
    }

    #[test]
    fn grouping_paren_outside_regex_still_op() {
        let toks = tokenize("[[ ( -n a ) ]]").unwrap();
        assert!(toks.iter().any(|t| matches!(t, Token::Op(Operator::LParen))));
        assert!(toks.iter().any(|t| matches!(t, Token::Op(Operator::RParen))));
    }

    #[test]
    fn arith_block_outside_dbracket_unchanged() {
        let toks = tokenize("(( 1 + 1 ))").unwrap();
        assert!(toks.iter().any(|t| matches!(t, Token::ArithBlock(..))));
    }

    #[test]
    fn quoted_dbracket_word_does_not_change_depth() {
        let toks = tokenize("[[ '[[' = x ]]").unwrap();
        assert!(toks.iter().filter_map(word_text).any(|t| t == "]]"));
    }

    #[test]
    fn char_cursor_tracks_byte_offset() {
        let mut c = CharCursor::new("ab\nc");
        assert_eq!(c.offset(), 0);
        assert_eq!(c.peek(), Some(&'a'));
        assert_eq!(c.offset(), 0); // peek does not advance
        assert_eq!(c.next(), Some('a'));
        assert_eq!(c.offset(), 1);
        assert_eq!(c.next(), Some('b'));
        assert_eq!(c.next(), Some('\n'));
        assert_eq!(c.offset(), 3);
        assert_eq!(c.next(), Some('c'));
        assert_eq!(c.offset(), 4);
        assert_eq!(c.next(), None);
        assert_eq!(c.offset(), 4);
    }

    #[test]
    fn offsets_align_with_token_starts() {
        // "echo hi\nls" -> Word(echo)@0 Word(hi)@5 Newline@7 Word(ls)@8, sentinel@10
        let (toks, offs, _lns) = tokenize_with_offsets("echo hi\nls", LexerOptions::default()).unwrap();
        assert_eq!(offs.len(), toks.len() + 1);
        assert_eq!(offs[toks.len()], 10); // sentinel = input.len()
        assert_eq!(offs[0], 0);
        let nl = toks.iter().position(|t| matches!(t, Token::Newline)).unwrap();
        assert_eq!(offs[nl], 7);
        assert_eq!(offs[nl + 1], 8);
    }

    /// Each token's recorded offset must point at the byte where that token's
    /// source actually begins (verified by re-deriving the offset from a
    /// distinctive first character). Guards offset *correctness*, not just the
    /// lockstep length invariant.
    #[test]
    fn offsets_point_at_real_token_starts() {
        // Leading whitespace before the first word; operators; a quoted word.
        //          0123456789012345678901
        let src = "  echo a && ls 'x y'\n";
        let (toks, offs, _lns) = tokenize_with_offsets(src, LexerOptions::default()).unwrap();
        assert_eq!(offs.len(), toks.len() + 1);
        // First word "echo" starts at byte 2 (after two spaces).
        assert_eq!(offs[0], 2);
        // The `&&` operator starts at byte 9.
        let and = toks.iter().position(|t| matches!(t, Token::Op(Operator::And))).unwrap();
        assert_eq!(offs[and], 9);
        assert_eq!(&src[offs[and]..offs[and] + 2], "&&");
        // The quoted word 'x y' starts at byte 15 (the opening quote).
        assert_eq!(&src[offs[toks.len() - 2]..offs[toks.len() - 2] + 1], "'");
        // Trailing Newline at byte 20; sentinel at 21.
        assert_eq!(offs[toks.len() - 1], 20);
        assert_eq!(offs[toks.len()], 21);
        // Offsets are non-decreasing and in range.
        for w in offs.windows(2) {
            assert!(w[0] <= w[1]);
        }
        assert!(offs.iter().all(|&o| o <= src.len()));
    }

    /// Offsets across multiple physical lines map to the right line when the
    /// reader counts newlines up to an offset (the v94 line-number mechanism).
    #[test]
    fn offsets_support_line_lookup() {
        let src = "echo a\necho b\nbad)\n";
        let (toks, offs, _lns) = tokenize_with_offsets(src, LexerOptions::default()).unwrap();
        // The `)` operator is on line 3; line = 1 + newlines before its offset.
        let rp = toks.iter().position(|t| matches!(t, Token::Op(Operator::RParen))).unwrap();
        let line = 1 + src.as_bytes()[..offs[rp]].iter().filter(|&&b| b == b'\n').count();
        assert_eq!(line, 3);
    }

    #[test]
    fn offsets_error_returns_failure_position() {
        let err = tokenize_with_offsets("echo 'oops", LexerOptions::default());
        assert!(err.is_err());
        let (_e, off) = err.unwrap_err();
        assert!(off >= 5, "failure offset {off} should be at/after the open quote");
    }

    #[test]
    fn tokenize_with_opts_output_unchanged() {
        let a = tokenize_with_opts("if true; then echo hi; fi", LexerOptions::default()).unwrap();
        let (b, _o, _l) = tokenize_with_offsets("if true; then echo hi; fi", LexerOptions::default()).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn char_cursor_offset_with_multibyte() {
        // 'é' is 2 bytes in UTF-8.
        let mut c = CharCursor::new("é!");
        assert_eq!(c.offset(), 0);
        assert_eq!(c.next(), Some('é'));
        assert_eq!(c.offset(), 2);
        assert_eq!(c.next(), Some('!'));
        assert_eq!(c.offset(), 3);
    }

    #[test]
    fn extglob_word_recognized_when_enabled() {
        let toks = tokenize_with_opts("+(a|b)", LexerOptions { extglob: true }).unwrap();
        assert_eq!(toks.len(), 1, "expected one Word token, got {toks:?}");
        assert!(matches!(&toks[0], Token::Word(_)));
    }

    #[test]
    fn extglob_word_split_when_disabled() {
        // default tokenize: unchanged — `(` is an operator.
        let toks = tokenize("+(a|b)").unwrap();
        assert!(toks.len() > 1, "default lexing must be unchanged: {toks:?}");
    }

    #[test]
    fn extglob_all_prefixes_and_nesting() {
        for p in ["?(a)", "*(a)", "@(a|b)", "!(a)", "a+(b|c)d", "@(a*(b)c)"] {
            let toks = tokenize_with_opts(p, LexerOptions { extglob: true }).unwrap();
            assert_eq!(toks.len(), 1, "{p} should be one word, got {toks:?}");
        }
    }

    #[test]
    fn extglob_group_preserves_inner_expansion() {
        // `+($x)` must NOT collapse to a single flat literal — the `$x`
        // inside the group has to survive as a Param part so it expands.
        let toks = tokenize_with_opts("+($x)", LexerOptions { extglob: true }).unwrap();
        assert_eq!(toks.len(), 1, "expected one Word token, got {toks:?}");
        let Token::Word(Word(parts)) = &toks[0] else {
            panic!("expected a Word token, got {:?}", toks[0]);
        };
        assert!(parts.len() > 1, "group should not be one flat literal: {parts:?}");
        assert!(
            parts.iter().any(|p| matches!(p, WordPart::Var { .. })),
            "expected a Var part for $x, got {parts:?}"
        );
    }

    /// Builds a Token that holds a single quoted-Literal Word.
    /// A single-quoted word: `'s'` → `Quoted{Single, [Literal{s, true}]}`.
    fn wq(s: &str) -> Token {
        Token::Word(Word(vec![qsingle(s)]))
    }
    /// A double-quoted word with a single literal body: `"s"`.
    fn wqd(s: &str) -> Token {
        Token::Word(Word(vec![qdouble(vec![WordPart::Literal {
            text: s.to_string(),
            quoted: true,
        }])]))
    }
    /// A backslash-escaped single char as a word: `\s`.
    fn wqb(s: &str) -> Token {
        Token::Word(Word(vec![qbackslash(s)]))
    }
    /// An ANSI-C quoted word: `$'s'` (s already decoded).
    fn wqa(s: &str) -> Token {
        Token::Word(Word(vec![WordPart::Quoted {
            style: QuoteStyle::AnsiC,
            parts: vec![WordPart::Literal { text: s.to_string(), quoted: true }],
        }]))
    }
    /// A single-quote run as a `WordPart`.
    fn qsingle(s: &str) -> WordPart {
        WordPart::Quoted {
            style: QuoteStyle::Single,
            parts: vec![WordPart::Literal { text: s.to_string(), quoted: true }],
        }
    }
    /// A double-quote run as a `WordPart`, given its inner parts.
    fn qdouble(parts: Vec<WordPart>) -> WordPart {
        WordPart::Quoted { style: QuoteStyle::Double, parts }
    }
    /// A backslash-escaped single char as a `WordPart`.
    fn qbackslash(s: &str) -> WordPart {
        WordPart::Quoted {
            style: QuoteStyle::Backslash,
            parts: vec![WordPart::Literal { text: s.to_string(), quoted: true }],
        }
    }
    /// Unwrap a `$'…'` (AnsiC) run, returning its single inner part.
    fn ansi_c_inner(part: &WordPart) -> &WordPart {
        let WordPart::Quoted { style: QuoteStyle::AnsiC, parts } = part
        else { panic!("expected AnsiC run, got {part:?}") };
        &parts[0]
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
        let part = word.0.into_iter().next().expect("non-empty word");
        // A `"${…}"` word wraps the expansion in a double-quote run; unwrap it
        // so callers see the inner expansion part directly.
        match part {
            WordPart::Quoted { parts, .. } => {
                parts.into_iter().next().expect("non-empty quoted run")
            }
            other => other,
        }
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
            vec![w("echo"), wqd("# inside")]
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
            vec![wqd("foobar")]
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
            vec![wqd("`")]
        );
    }

    #[test]
    fn tokenize_escaped_hash_is_literal() {
        // `\#` at word start: backslash escape, # is literal
        assert_eq!(
            tokenize(r"echo \#hash").unwrap(),
            vec![w("echo"), Token::Word(Word(vec![
                qbackslash("#"),
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
            vec![w("echo"), wqd("hello world")]
        );
    }

    #[test]
    fn tokenize_double_quote_escape() {
        assert_eq!(tokenize(r#"echo "a\"b""#).unwrap(), vec![w("echo"), wqd("a\"b")]);
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
                    qbackslash(" "),
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
        assert_eq!(parts, &[qbackslash("*")]);
    }

    #[test]
    fn backslash_in_middle_of_word_flushes_and_quotes() {
        // `foo\*bar` → unquoted "foo", quoted "*", unquoted "bar"
        let tokens = tokenize("foo\\*bar").unwrap();
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        assert_eq!(parts, &[
            WordPart::Literal { text: "foo".to_string(), quoted: false },
            qbackslash("*"),
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
                qdouble(vec![WordPart::Literal { text: "bar baz".to_string(), quoted: true }]),
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
    fn pipe_both_desugars_to_2_redir_1_then_pipe() {
        // `a |& b` lexes as `a 2>&1 | b`.
        let toks = tokenize("a |& b").unwrap();
        assert_eq!(
            toks,
            vec![
                w("a"),
                Token::RedirFd(crate::command::RedirFd::Number(2)),
                Token::Op(Operator::DupOut),
                w("1"),
                Token::Op(Operator::Pipe),
                w("b"),
            ]
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
            vec![w("echo"), wqd("|"), wqd(">")]
        );
    }

    #[test]
    fn tokenize_escaped_operators_stay_words() {
        // Escaped operators become quoted single-char Literals (one Word each).
        assert_eq!(
            tokenize(r"echo \| \>").unwrap(),
            vec![w("echo"), wqb("|"), wqb(">")]
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
            vec![w("echo"), wqd("&&"), wqd("||"), wqd(";")]
        );
    }

    #[test]
    fn tokenize_escaped_sequencing_operators_stay_words() {
        // Each `\X` becomes its own quoted single-char Literal part. Adjacent
        // escapes within the same token concatenate into one Word with N parts.
        let two_quoted = |a: &str, b: &str| {
            Token::Word(Word(vec![qbackslash(a), qbackslash(b)]))
        };
        assert_eq!(
            tokenize(r"echo \&\& \|\| \;").unwrap(),
            vec![w("echo"), two_quoted("&", "&"), two_quoted("|", "|"), wqb(";")]
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
        assert_eq!(
            tokenize("\"$FOO\"").unwrap(),
            vec![Token::Word(Word(vec![qdouble(vec![WordPart::Var {
                name: "FOO".to_string(),
                quoted: true,
            }])]))]
        );
    }

    #[test]
    fn tokenize_dollar_squote_inside_double_quotes_is_literal() {
        // v181: `$'` inside double quotes is a literal `$` + `'`, NOT ANSI-C
        // quoting; it must tokenize (pre-fix this was Err(UnterminatedQuote)).
        let toks = tokenize("\"$'\"").unwrap();
        assert_eq!(toks.len(), 1);
        let Token::Word(Word(parts)) = &toks[0] else { panic!("not a word: {toks:?}") };
        // The body is a single double-quote run wrapping literal parts.
        let [WordPart::Quoted { style: QuoteStyle::Double, parts: inner }] = &parts[..]
        else { panic!("expected one double-quote run: {parts:?}") };
        let joined: String = inner.iter().map(|p| match p {
            WordPart::Literal { text, .. } => text.clone(),
            other => panic!("unexpected part {other:?}"),
        }).collect();
        assert_eq!(joined, "$'");
    }

    #[test]
    fn tokenize_dollar_dquote_locale_drops_dollar() {
        // v181: `$"x"` is locale-translation quoting = identity; the `$` is
        // dropped and the body is a plain double-quoted literal `x`.
        assert_eq!(tokenize("$\"x\"").unwrap(), vec![wqd("x")]);
    }

    #[test]
    fn tokenize_unquoted_ansi_c_still_decodes() {
        // v181 regression: unquoted `$'…'` ANSI-C escapes still decode (the
        // `!quoted` guard must not disturb the outside-double-quotes path).
        assert_eq!(tokenize("$'a\\tb'").unwrap(), vec![wqa("a\tb")]);
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
        assert_eq!(tokenize("\"~\"").unwrap(), vec![wqd("~")]);
    }

    #[test]
    fn tokenize_braced_var_invalid_name() {
        // ${1foo}: digits consumed as positional name "1", then `f` is not a
        // valid modifier. v233: deferred to runtime BadSubst (matching bash)
        // instead of a parse error.
        let toks = tokenize("${1foo}").unwrap();
        let Token::Word(Word(parts)) = &toks[0] else { panic!() };
        assert!(matches!(&parts[0],
            WordPart::ParamExpansion { modifier: ParamModifier::BadSubst { raw }, .. }
            if raw == "${1foo}"
        ), "expected BadSubst, got {:?}", parts[0]);
    }

    #[test]
    fn tokenize_braced_var_empty_name() {
        // v233: `${}` is lexable-but-invalid → BadSubst at runtime, not a parse error.
        let toks = tokenize("${}").unwrap();
        let Token::Word(Word(parts)) = &toks[0] else { panic!() };
        assert!(matches!(&parts[0],
            WordPart::ParamExpansion { modifier: ParamModifier::BadSubst { raw }, .. }
            if raw == "${}"
        ), "expected BadSubst, got {:?}", parts[0]);
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
        assert_eq!(tokenize(r#""\$FOO""#).unwrap(), vec![wqd("$FOO")]);
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
                negate: false,
                commands: vec![Command::Simple(SimpleCommand::Exec(ExecCommand {
                    inline_assignments: Vec::new(),
                    program: Word(vec![WordPart::Literal { text: "echo".to_string(), quoted: false }]),
                    args: args
                        .iter()
                        .map(|a| Word(vec![WordPart::Literal { text: a.to_string(), quoted: false }]))
                        .collect(),
                    redirects: Vec::new(),
                    line: 0,
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
            vec![sub_word(vec![qdouble(vec![WordPart::CommandSub {
                sequence: echo_seq(&["hi"]),
                quoted: true,
            }])])]
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
                        crate::command::Pipeline { negate: false, commands: vec![] },
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
                negate: false,
                commands: vec![Command::Simple(SimpleCommand::Exec(ExecCommand {
                    inline_assignments: Vec::new(),
                    program: Word(vec![WordPart::Literal { text: "echo".to_string(), quoted: false }]),
                    args: vec![Word(vec![qdouble(vec![WordPart::Literal { text: ")".to_string(), quoted: true }])])],
                    redirects: Vec::new(),
                    line: 0,
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
                    negate: false,
                    commands: vec![Command::Simple(SimpleCommand::Exec(ExecCommand {
                        inline_assignments: Vec::new(),
                        program: Word(vec![WordPart::Literal { text: "echo".to_string(), quoted: false }]),
                        args: vec![inner_word],
                        redirects: Vec::new(),
                        line: 0,
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
    fn tokenize_command_sub_with_subshell_body() {
        // v101: `$( (echo a) )` — the inner `(` raises paren depth so the
        // subshell's `)` doesn't close the command substitution early. Used to
        // error with UnterminatedSubstitution (the bare-`(` arm didn't count).
        let tokens = tokenize("$( (echo a) )").unwrap();
        assert_eq!(tokens.len(), 1);
        match &tokens[0] {
            Token::Word(Word(parts)) => {
                assert_eq!(parts.len(), 1);
                match &parts[0] {
                    WordPart::CommandSub { sequence, .. } => {
                        // The command sub's body is a single subshell `(echo a)`.
                        assert!(
                            matches!(&sequence.first, crate::command::Command::Subshell { .. }),
                            "expected first command to be a Subshell, got {:?}",
                            sequence.first
                        );
                    }
                    other => panic!("expected CommandSub, got {other:?}"),
                }
            }
            other => panic!("expected Word, got {other:?}"),
        }
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
        // v233: `${1foo}` inside a substitution is now a runtime BadSubst,
        // not a parse error. The command sub parses successfully.
        let toks = tokenize("$(echo ${1foo})").unwrap();
        // The outer token is a Word containing a CommandSub.
        assert!(matches!(&toks[0], Token::Word(Word(p)) if matches!(&p[0], WordPart::CommandSub { .. })));
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
            vec![sub_word(vec![qdouble(vec![WordPart::CommandSub {
                sequence: echo_seq(&["hi"]),
                quoted: true,
            }])])]
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
                            // `\n` → a backslash run wrapping literal `n`.
                            match &e.args[0].0[0] {
                                WordPart::Quoted { style: QuoteStyle::Backslash, parts } => {
                                    match &parts[0] {
                                        WordPart::Literal { text, .. } => assert_eq!(text, "n"),
                                        other => panic!("expected Literal(n), got {other:?}"),
                                    }
                                }
                                other => panic!("expected backslash run, got {other:?}"),
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
        assert_eq!(parts[0], qdouble(vec![WordPart::Literal { text: "F".to_string(), quoted: true }]));
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
        assert_eq!(parts[1], qdouble(vec![WordPart::Literal { text: "bar".to_string(), quoted: true }]));
        assert_eq!(parts[2], WordPart::Literal { text: "baz".to_string(), quoted: false });
    }

    /// Helper: the literal text of a single-literal arith body `Word`.
    fn arith_body_lit(part: &WordPart) -> &str {
        let WordPart::Arith { body: Word(bparts), .. } = part else {
            panic!("expected Arith part, got {part:?}")
        };
        assert_eq!(bparts.len(), 1, "expected single-literal body, got {bparts:?}");
        let WordPart::Literal { text, .. } = &bparts[0] else {
            panic!("expected Literal body part, got {:?}", bparts[0])
        };
        text
    }

    #[test]
    fn tokenize_arith_simple() {
        // Post-v93: the arith body is deferred as an expandable Word; here it is
        // a single literal `1+2` (parsed at eval time, not lex time).
        let tokens = tokenize("$((1+2))").unwrap();
        assert_eq!(tokens.len(), 1);
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        assert_eq!(parts.len(), 1);
        let WordPart::Arith { quoted, .. } = &parts[0] else {
            panic!("expected Arith part, got {:?}", parts[0])
        };
        assert!(!(*quoted));
        assert_eq!(arith_body_lit(&parts[0]), "1+2");
    }

    #[test]
    fn tokenize_legacy_arith_basic() {
        let tokens = tokenize("$[2**(3*2)]").unwrap();
        assert_eq!(tokens.len(), 1);
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        assert_eq!(parts.len(), 1);
        let WordPart::Arith { quoted, .. } = &parts[0] else {
            panic!("expected Arith part, got {:?}", parts[0])
        };
        assert!(!(*quoted));
        assert_eq!(arith_body_lit(&parts[0]), "2**(3*2)");
    }

    #[test]
    fn tokenize_legacy_arith_array_subscript() {
        let tokens = tokenize("$[a[1]+1]").unwrap();
        assert_eq!(tokens.len(), 1);
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        assert_eq!(parts.len(), 1);
        assert_eq!(arith_body_lit(&parts[0]), "a[1]+1");
    }

    #[test]
    fn tokenize_legacy_arith_aware_close() {
        for src in ["$[ $(echo ']')+1 ]", "$[ \"x]\" + 1 ]"] {
            let tokens = tokenize(src).unwrap_or_else(|e| panic!("{src}: {e:?}"));
            assert_eq!(tokens.len(), 1, "{src} closed early: {tokens:?}");
            let Token::Word(Word(parts)) = &tokens[0] else { panic!("{src}") };
            assert_eq!(parts.len(), 1, "{src}: {parts:?}");
            assert!(matches!(parts[0], WordPart::Arith { .. }), "{src}: {:?}", parts[0]);
        }
    }

    #[test]
    fn tokenize_legacy_arith_unterminated() {
        assert!(matches!(tokenize("$[ 1+2"), Err(LexError::UnterminatedLegacyArith)));
    }

    #[test]
    fn tokenize_legacy_arith_braced_param() {
        // A `}` inside `${…}` inside `$[…]` must not close early (exercises
        // scan_braced_skip, which the other tests don't reach).
        let tokens = tokenize("$[${x}+1]").unwrap();
        assert_eq!(tokens.len(), 1);
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        assert_eq!(parts.len(), 1);
        assert!(matches!(parts[0], WordPart::Arith { .. }), "got {:?}", parts[0]);
    }

    #[test]
    fn tokenize_legacy_arith_inside_dquote() {
        // `"$[1+2]"` — the $[ arm must carry quoted: true through to WordPart::Arith.
        let tokens = tokenize("\"$[1+2]\"").unwrap();
        assert_eq!(tokens.len(), 1);
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        assert_eq!(parts.len(), 1);
        let [WordPart::Quoted { style: QuoteStyle::Double, parts: inner }] = &parts[..]
        else { panic!("expected one double-quote run: {parts:?}") };
        let WordPart::Arith { quoted, .. } = &inner[0] else {
            panic!("expected Arith part, got {:?}", inner[0])
        };
        assert!(*quoted);
        assert_eq!(arith_body_lit(&inner[0]), "1+2");
    }

    #[test]
    fn arith_string_to_word_inherits_extglob() {
        // A command substitution inside arithmetic whose body uses an extglob
        // pattern lexes only when extglob is enabled (L-24).
        let body = "$( [[ foo == @(foo|bar) ]] && echo 1 )";
        assert!(arith_string_to_word(body, LexerOptions { extglob: true }).is_ok());
        assert!(arith_string_to_word(body, LexerOptions { extglob: false }).is_err());
    }

    #[test]
    fn tokenize_arith_with_nested_parens() {
        let tokens = tokenize("$(( (1+2) * 3 ))").unwrap();
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        assert_eq!(arith_body_lit(&parts[0]), " (1+2) * 3 ");
    }

    #[test]
    fn tokenize_arith_inside_double_quotes_is_quoted() {
        let tokens = tokenize("\"$((1+2))\"").unwrap();
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        let [WordPart::Quoted { style: QuoteStyle::Double, parts: inner }] = &parts[..]
        else { panic!("expected one double-quote run: {parts:?}") };
        let WordPart::Arith { quoted, .. } = &inner[0] else { panic!() };
        assert!(*quoted);
    }

    #[test]
    fn tokenize_arith_unterminated_returns_error() {
        // `$((1+2` doesn't close as `))`, so since v177 it falls back to a
        // command substitution (`$( (1+2 … )`) — which is itself unterminated at
        // EOF. Still an error, now reported as an unterminated substitution.
        let err = tokenize("$((1+2").unwrap_err();
        assert_eq!(err, LexError::UnterminatedSubstitution);
    }

    #[test]
    fn tokenize_arith_parse_error_is_deferred_to_eval() {
        // Post-v93: arithmetic is parsed at eval time, so a body that would
        // fail to parse (`1+`) still lexes successfully into an Arith part.
        let tokens = tokenize("$((1+))").unwrap();
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        assert!(matches!(parts[0], WordPart::Arith { .. }));
        assert_eq!(arith_body_lit(&parts[0]), "1+");
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
        // Post-v93: `$x` inside `$(())` is now an expandable Var body part
        // (expanded before arith parse), not a pre-parsed ArithExpr::Var.
        let tokens = tokenize("$(($x))").unwrap();
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        let WordPart::Arith { body: Word(bparts), .. } = &parts[0] else { panic!() };
        assert_eq!(bparts.len(), 1);
        assert_eq!(bparts[0], WordPart::Var { name: "x".to_string(), quoted: true });
    }

    #[test]
    fn tokenize_arith_back_to_back_in_same_word() {
        let tokens = tokenize("$((1+2))$((3+4))").unwrap();
        assert_eq!(tokens.len(), 1);
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        assert_eq!(parts.len(), 2);
        assert_eq!(arith_body_lit(&parts[0]), "1+2");
        assert_eq!(arith_body_lit(&parts[1]), "3+4");
    }

    #[test]
    fn tokenize_dollar_paren_subshell_falls_back_to_command_sub() {
        // v177: when the body after `$((` does not close as `))` (the inner `)`
        // is not immediately followed by another `)`), it is a command
        // substitution whose body starts with a subshell — `$( (echo hi) 2>&1 )`
        // written glued — NOT arithmetic. (Pre-v177 this returned UnterminatedArith.)
        let tokens = tokenize("$((echo hi) 2>&1)").unwrap();
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        assert!(matches!(parts[0], WordPart::CommandSub { .. }));
    }

    #[test]
    fn skip_line_comment_stops_before_newline() {
        // The opening `#` is the caller's; this runs the body to (not incl.) \n.
        let mut chars = CharCursor::new("a comment ) here\nNEXT");
        skip_line_comment(&mut chars);
        assert_eq!(chars.next(), Some('\n'));
        assert_eq!(chars.next(), Some('N'));
    }

    #[test]
    fn scan_braced_operand_simple() {
        let mut chars = CharCursor::new("foo}");
        assert_eq!(scan_braced_operand(&mut chars).unwrap(), "foo");
    }

    #[test]
    fn scan_braced_operand_nested_braces() {
        let mut chars = CharCursor::new("${Y}}");
        assert_eq!(scan_braced_operand(&mut chars).unwrap(), "${Y}");
    }

    #[test]
    fn scan_braced_operand_double_quote_protects_brace() {
        let mut chars = CharCursor::new("\"a}b\"c}");
        assert_eq!(scan_braced_operand(&mut chars).unwrap(), "\"a}b\"c");
    }

    #[test]
    fn scan_braced_operand_single_quote_protects_brace() {
        let mut chars = CharCursor::new("'a}b'c}");
        assert_eq!(scan_braced_operand(&mut chars).unwrap(), "'a}b'c");
    }

    #[test]
    fn scan_braced_operand_unterminated_is_error() {
        let mut chars = CharCursor::new("foo");
        assert_eq!(scan_braced_operand(&mut chars).unwrap_err(), LexError::UnterminatedBrace);
    }

    #[test]
    fn scan_braced_operand_skips_paren_cmdsub_with_brace() {
        let mut chars = CharCursor::new("$(echo a}b)/Z}");
        assert_eq!(scan_braced_operand(&mut chars).unwrap(), "$(echo a}b)/Z");
    }

    #[test]
    fn scan_braced_operand_skips_backtick_cmdsub_with_brace() {
        let mut chars = CharCursor::new("`echo a}b`/Z}");
        assert_eq!(scan_braced_operand(&mut chars).unwrap(), "`echo a}b`/Z");
    }

    #[test]
    fn scan_braced_operand_skips_nested_cmdsub() {
        let mut chars = CharCursor::new("$(echo $(echo a}b))/Q}");
        assert_eq!(scan_braced_operand(&mut chars).unwrap(), "$(echo $(echo a}b))/Q");
    }

    #[test]
    fn scan_braced_operand_skips_arith_cmdsub() {
        let mut chars = CharCursor::new("$((1+2))}");
        assert_eq!(scan_braced_operand(&mut chars).unwrap(), "$((1+2))");
    }

    #[test]
    fn scan_braced_operand_unterminated_paren_cmdsub_errors() {
        let mut chars = CharCursor::new("$(echo a");
        assert_eq!(
            scan_braced_operand(&mut chars).unwrap_err(),
            LexError::UnterminatedBrace
        );
    }

    #[test]
    fn scan_braced_operand_paren_cmdsub_skips_quoted_paren() {
        // A `)` inside a quoted span within $(…) must not end the substitution.
        let mut chars = CharCursor::new("$(echo \")\")}");
        assert_eq!(scan_braced_operand(&mut chars).unwrap(), "$(echo \")\")");
    }

    #[test]
    fn scan_cmdsub_body_basic_consumes_through_close_paren() {
        let mut chars = CharCursor::new("echo hi)rest");
        let mut out = String::new();
        scan_cmdsub_body(&mut chars, &mut out, LexError::UnterminatedSubstitution).unwrap();
        assert_eq!(out, "echo hi"); // closing ) consumed, not appended
        assert_eq!(chars.next(), Some('r')); // cursor left just past the )
    }

    #[test]
    fn scan_cmdsub_body_balances_nested_and_arith() {
        let mut chars = CharCursor::new("echo $(echo x))");
        let mut out = String::new();
        scan_cmdsub_body(&mut chars, &mut out, LexError::UnterminatedBrace).unwrap();
        assert_eq!(out, "echo $(echo x)");

        // $((1+2)) — caller consumed the outer `$(`, body starts at the inner `(`
        let mut chars = CharCursor::new("(1+2))");
        let mut out = String::new();
        scan_cmdsub_body(&mut chars, &mut out, LexError::UnterminatedBrace).unwrap();
        assert_eq!(out, "(1+2)");
    }

    #[test]
    fn scan_cmdsub_body_skips_quoted_paren() {
        let mut chars = CharCursor::new("echo \")\")");
        let mut out = String::new();
        scan_cmdsub_body(&mut chars, &mut out, LexError::UnterminatedBrace).unwrap();
        assert_eq!(out, "echo \")\"");
    }

    #[test]
    fn scan_cmdsub_body_unterminated_uses_passed_error() {
        let mut chars = CharCursor::new("echo hi");
        let mut out = String::new();
        assert_eq!(
            scan_cmdsub_body(&mut chars, &mut out, LexError::UnterminatedSubstitution).unwrap_err(),
            LexError::UnterminatedSubstitution
        );
        let mut chars = CharCursor::new("echo hi");
        let mut out = String::new();
        assert_eq!(
            scan_cmdsub_body(&mut chars, &mut out, LexError::UnterminatedBrace).unwrap_err(),
            LexError::UnterminatedBrace
        );
    }

    #[test]
    fn scan_cmdsub_body_case_pattern_paren_not_close() {
        // v186: a bare case-pattern `)` (depth 0) is a pattern terminator, not the
        // cmdsub close. Stops at the FINAL `)` after `esac`.
        let mut chars = CharCursor::new("case $y in a) echo hit;; esac)rest");
        let mut out = String::new();
        scan_cmdsub_body(&mut chars, &mut out, LexError::UnterminatedSubstitution).unwrap();
        assert_eq!(out, "case $y in a) echo hit;; esac");
        assert_eq!(chars.next(), Some('r'));
    }

    #[test]
    fn scan_cmdsub_body_case_as_arg_is_not_keyword() {
        // v186: `case` NOT in command position (an argument) is a plain word — the
        // first `)` closes.
        let mut chars = CharCursor::new("echo case)rest");
        let mut out = String::new();
        scan_cmdsub_body(&mut chars, &mut out, LexError::UnterminatedBrace).unwrap();
        assert_eq!(out, "echo case");
        assert_eq!(chars.next(), Some('r'));
    }

    #[test]
    fn scan_cmdsub_body_nested_case() {
        // v186: nested `case … esac` — only the FINAL `)` closes.
        let mut chars = CharCursor::new("case $y in a) case $y in a) :;; esac;; esac)X");
        let mut out = String::new();
        scan_cmdsub_body(&mut chars, &mut out, LexError::UnterminatedBrace).unwrap();
        assert_eq!(out, "case $y in a) case $y in a) :;; esac;; esac");
        assert_eq!(chars.next(), Some('X'));
    }

    #[test]
    fn scan_cmdsub_body_skips_word_start_comment() {
        // v183: a word-start `#` comment is kept verbatim in the body; a `)`
        // inside it does NOT close the substitution. Stops at the FINAL `)`.
        let mut chars = CharCursor::new("echo hi # c with ) paren\n)rest");
        let mut out = String::new();
        scan_cmdsub_body(&mut chars, &mut out, LexError::UnterminatedSubstitution).unwrap();
        assert_eq!(out, "echo hi # c with ) paren\n");
        assert_eq!(chars.next(), Some('r'));
    }

    #[test]
    fn scan_cmdsub_body_midword_hash_not_comment() {
        // v183 regression: `#` mid-word (`a#b`) is literal, not a comment.
        let mut chars = CharCursor::new("echo a#b)");
        let mut out = String::new();
        scan_cmdsub_body(&mut chars, &mut out, LexError::UnterminatedBrace).unwrap();
        assert_eq!(out, "echo a#b");
    }

    #[test]
    fn scan_backtick_body_basic_consumes_through_close() {
        let mut chars = CharCursor::new("echo hi`rest");
        let mut out = String::new();
        scan_backtick_body(&mut chars, &mut out, LexError::UnterminatedSubstitution).unwrap();
        assert_eq!(out, "echo hi"); // closing backtick consumed, not appended
        assert_eq!(chars.next(), Some('r'));
    }

    #[test]
    fn scan_backtick_body_escaped_backtick_does_not_close() {
        // Input: a \ ` b `  — the escaped backtick is raw-preserved and does not close.
        let mut chars = CharCursor::new("a\\`b`");
        let mut out = String::new();
        scan_backtick_body(&mut chars, &mut out, LexError::UnterminatedBrace).unwrap();
        assert_eq!(out, "a\\`b"); // raw, escape preserved
    }

    #[test]
    fn scan_backtick_body_unterminated_uses_passed_error() {
        let mut chars = CharCursor::new("echo hi");
        let mut out = String::new();
        assert_eq!(
            scan_backtick_body(&mut chars, &mut out, LexError::UnterminatedSubstitution).unwrap_err(),
            LexError::UnterminatedSubstitution
        );
        let mut chars = CharCursor::new("echo hi");
        let mut out = String::new();
        assert_eq!(
            scan_backtick_body(&mut chars, &mut out, LexError::UnterminatedBrace).unwrap_err(),
            LexError::UnterminatedBrace
        );
    }

    #[test]
    fn unescape_backtick_applies_bash_rules() {
        assert_eq!(unescape_backtick("a\\`b"), "a`b"); // \` -> `
        assert_eq!(unescape_backtick("a\\\\b"), "a\\b"); // \\ -> \
        assert_eq!(unescape_backtick("a\\$b"), "a$b"); // \$ -> $
        assert_eq!(unescape_backtick("a\\xb"), "a\\xb"); // \x -> \x (verbatim)
        assert_eq!(unescape_backtick("plain"), "plain");
    }

    #[test]
    fn parse_braced_operand_single_word() {
        let w = parse_braced_operand("foo").unwrap();
        assert_eq!(w.0.len(), 1);
        assert_eq!(w.0[0], WordPart::Literal { text: "foo".to_string(), quoted: false });
    }

    // Local test helper: concatenate the literal text of a Word's parts
    // (expansions render as a placeholder so structure tests stay simple).
    fn operand_lits(w: &Word) -> String {
        let mut s = String::new();
        for p in &w.0 {
            match p {
                WordPart::Literal { text, .. } => s.push_str(text),
                WordPart::Var { name, .. } => { s.push('$'); s.push_str(name); }
                _ => s.push('§'), // any other expansion part
            }
        }
        s
    }

    #[test]
    fn parse_braced_operand_two_words_join_with_space() {
        // After the fix: "foo bar" is a single unquoted literal run.
        let w = parse_braced_operand("foo bar").unwrap();
        assert_eq!(operand_lits(&w), "foo bar");
        assert!(w.0.iter().all(|p| matches!(p, WordPart::Literal { quoted: false, .. })));
    }

    #[test]
    fn parse_braced_operand_top_level_pipe_is_ok() {
        // After the fix: pipe is literal, not an error.
        assert_eq!(operand_lits(&parse_braced_operand("foo | bar").unwrap()), "foo | bar");
    }

    #[test]
    fn parse_braced_operand_empty_returns_empty_word() {
        let w = parse_braced_operand("").unwrap();
        assert_eq!(w.0.len(), 0);
    }

    #[test]
    fn operand_parens_are_literal() {
        assert_eq!(operand_lits(&parse_braced_operand("(a)").unwrap()), "(a)");
    }

    #[test]
    fn operand_pipe_semicolon_amp_are_literal() {
        assert_eq!(operand_lits(&parse_braced_operand("a|b;c&d").unwrap()), "a|b;c&d");
        assert_eq!(operand_lits(&parse_braced_operand("a(b)c").unwrap()), "a(b)c");
    }

    #[test]
    fn operand_expansion_with_parens() {
        // `($x)` → literal "(", Var x, literal ")"
        let w = parse_braced_operand("($x)").unwrap();
        assert_eq!(operand_lits(&w), "($x)");
    }

    #[test]
    fn operand_single_quote_is_literal_span() {
        // '|;()' inside single quotes → one quoted literal "|;()"
        let w = parse_braced_operand("'|;()'").unwrap();
        assert_eq!(operand_lits(&w), "|;()");
        assert!(matches!(w.0.as_slice(), [WordPart::Literal { quoted: true, .. }]));
    }

    #[test]
    fn operand_enclosing_dquote_keeps_single_quotes_literal() {
        // M-15b (v200): with enclosing_dquote=true, single quotes are LITERAL
        // characters (kept), not a quote span — `'a|b'` → the 5 chars `'a|b'`.
        let w = parse_braced_operand_opts("'a|b'", true, LexerOptions::default()).unwrap();
        assert_eq!(operand_lits(&w), "'a|b'");
        // Control: with enclosing_dquote=false the single quotes are stripped.
        let w0 = parse_braced_operand_opts("'a|b'", false, LexerOptions::default()).unwrap();
        assert_eq!(operand_lits(&w0), "a|b");
    }

    #[test]
    fn operand_enclosing_dquote_restricts_backslash() {
        // dquote backslash: special only before `$ ` " \`; `\*`/`\n` keep the
        // backslash, `\$` drops it.
        let star = parse_braced_operand_opts("\\*", true, LexerOptions::default()).unwrap();
        assert_eq!(operand_lits(&star), "\\*");
        let en = parse_braced_operand_opts("a\\nb", true, LexerOptions::default()).unwrap();
        assert_eq!(operand_lits(&en), "a\\nb");
        let dollar = parse_braced_operand_opts("a\\$b", true, LexerOptions::default()).unwrap();
        assert_eq!(operand_lits(&dollar), "a$b");
    }

    #[test]
    fn operand_double_quote_keeps_expansion() {
        // "a $x b" → quoted literal "a ", Var x (quoted), quoted literal " b"
        let w = parse_braced_operand("\"a $x b\"").unwrap();
        assert_eq!(operand_lits(&w), "a $x b");
        // Parts inside the double-quoted span carry quoted: true.
        assert!(w.0.iter().all(|p| match p {
            WordPart::Literal { quoted, .. } => *quoted,
            WordPart::Var { quoted, .. } => *quoted,
            _ => true,
        }));
    }

    #[test]
    fn operand_nested_brace() {
        let w = parse_braced_operand("${y:-z}").unwrap();
        assert!(matches!(w.0.as_slice(), [WordPart::ParamExpansion { .. }]));
    }

    #[test]
    fn operand_plain_words_split_friendly() {
        // "foo bar" → unquoted literal "foo bar" (one run; splits downstream).
        let w = parse_braced_operand("foo bar").unwrap();
        assert_eq!(operand_lits(&w), "foo bar");
        assert!(w.0.iter().all(|p| matches!(p, WordPart::Literal { quoted: false, .. })));
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
        let WordPart::ParamExpansion { name, modifier, quoted, .. } = &parts[0] else {
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
    fn tokenize_indirect_bare() {
        // `${!ref}` — v95 indirect scalar expansion, no modifier.
        let tokens = tokenize("${!ref}").unwrap();
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        let WordPart::ParamExpansion { name, modifier, subscript, indirect, .. } = &parts[0]
        else { panic!("expected ParamExpansion, got {:?}", parts[0]) };
        assert_eq!(name, "ref");
        assert!(*indirect);
        assert!(matches!(modifier, ParamModifier::None));
        assert!(subscript.is_none());
    }

    #[test]
    fn tokenize_indirect_with_default_modifier() {
        // `${!ref-w}` — v95 indirect + trailing UseDefault modifier.
        let tokens = tokenize("${!ref-w}").unwrap();
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        let WordPart::ParamExpansion { name, modifier, indirect, .. } = &parts[0]
        else { panic!("expected ParamExpansion, got {:?}", parts[0]) };
        assert_eq!(name, "ref");
        assert!(*indirect);
        assert!(matches!(modifier, ParamModifier::UseDefault { colon: false, .. }));
    }

    #[test]
    fn tokenize_indirect_array_keys_is_not_indirect() {
        // Regression: `${!a[@]}` is array-keys (IndirectKeys), NOT the
        // scalar indirect path — it must keep `indirect: false`.
        let tokens = tokenize("${!a[@]}").unwrap();
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        let WordPart::ParamExpansion { name, modifier, indirect, .. } = &parts[0]
        else { panic!("expected ParamExpansion, got {:?}", parts[0]) };
        assert_eq!(name, "a");
        assert!(!(*indirect));
        assert!(matches!(modifier, ParamModifier::IndirectKeys));
    }

    #[test]
    fn indirect_keys_with_suffix_op_is_indirect_not_keys() {
        // `${!v[@]%b}` — trailing `%b` makes it indirect-through-${v[@]} + RemoveSuffix,
        // NOT the array-keys operator.
        let toks = tokenize("${!v[@]%b}").unwrap();
        let Token::Word(Word(parts)) = &toks[0] else { panic!() };
        let WordPart::ParamExpansion { indirect, subscript, modifier, .. } = &parts[0]
        else { panic!("expected ParamExpansion, got {:?}", parts[0]) };
        assert!(*indirect);
        assert!(matches!(subscript, Some(SubscriptKind::All)));
        assert!(matches!(modifier, ParamModifier::RemoveSuffix { .. }));
    }

    #[test]
    fn indirect_keys_with_transform_op_is_indirect() {
        // `${!v[@]@Q}` — was wrongly BadSubst in v233; now indirect + transform.
        let toks = tokenize("${!v[@]@Q}").unwrap();
        let Token::Word(Word(parts)) = &toks[0] else { panic!() };
        let WordPart::ParamExpansion { indirect, subscript, modifier, .. } = &parts[0]
        else { panic!("expected ParamExpansion, got {:?}", parts[0]) };
        assert!(*indirect);
        assert!(matches!(subscript, Some(SubscriptKind::All)));
        assert!(matches!(modifier, ParamModifier::Transform { .. }));
    }

    #[test]
    fn indirect_keys_bare_still_keys() {
        // Regression: `${!v[@]}` with NOTHING after `]` stays the keys operator.
        let toks = tokenize("${!v[@]}").unwrap();
        let Token::Word(Word(parts)) = &toks[0] else { panic!() };
        let WordPart::ParamExpansion { modifier, .. } = &parts[0] else { panic!() };
        assert!(matches!(modifier, ParamModifier::IndirectKeys));
    }

    #[test]
    fn tokenize_prefix_names_star() {
        // `${!pfx*}` — prefix-name expansion, `*` form (at=false).
        let tokens = tokenize("${!_Q*}").unwrap();
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        let WordPart::ParamExpansion { name, modifier, indirect, subscript, .. } = &parts[0]
        else { panic!("expected ParamExpansion, got {:?}", parts[0]) };
        assert_eq!(name, "_Q");
        assert!(!(*indirect));
        assert!(subscript.is_none());
        assert!(matches!(modifier, ParamModifier::PrefixNames { at: false }));
    }

    #[test]
    fn tokenize_prefix_names_at() {
        // `${!pfx@}` — prefix-name expansion, `@` form (at=true).
        let tokens = tokenize("${!_Q@}").unwrap();
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        let WordPart::ParamExpansion { name, modifier, indirect, subscript, .. } = &parts[0]
        else { panic!("expected ParamExpansion, got {:?}", parts[0]) };
        assert_eq!(name, "_Q");
        assert!(!(*indirect));
        assert!(subscript.is_none());
        assert!(matches!(modifier, ParamModifier::PrefixNames { at: true }));
    }

    #[test]
    fn tokenize_indirect_transform_not_prefix_names() {
        // Regression: `${!ref@Q}` is a transform on an indirect ref, NOT the
        // prefix-name form — the `@` is not immediately followed by `}`.
        let tokens = tokenize("${!ref@Q}").unwrap();
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        let WordPart::ParamExpansion { name, modifier, indirect, .. } = &parts[0]
        else { panic!("expected ParamExpansion, got {:?}", parts[0]) };
        assert_eq!(name, "ref");
        assert!(*indirect);
        assert!(matches!(modifier, ParamModifier::Transform { .. }));
    }

    #[test]
    fn length_of_special_param_hash() {
        // v233 M2: `${##}` = `${#<#>}` = length of `$#`.
        let mut toks = tokenize("${##}").unwrap();
        let part = single_param_expansion(&mut toks);
        assert!(matches!(part,
            WordPart::ParamExpansion { ref name, modifier: ParamModifier::Length, indirect: false, .. } if name == "#"),
            "got {part:?}");
    }

    #[test]
    fn length_of_special_param_others() {
        // v233 M2: `${#?}`, `${#-}`, `${#$}`, `${#!}` = length of the special
        // param. `@`/`*` stay arg-count (not exercised here).
        for (src, want) in [("${#?}", "?"), ("${#-}", "-"), ("${#$}", "$"), ("${#!}", "!")] {
            let mut toks = tokenize(src).unwrap();
            let part = single_param_expansion(&mut toks);
            assert!(matches!(part,
                WordPart::ParamExpansion { ref name, modifier: ParamModifier::Length, indirect: false, .. } if name == want),
                "{src} -> got {part:?}");
        }
    }

    #[test]
    fn indirect_of_special_param_hash() {
        // v233 M2: `${!#}` = indirect through `$#`.
        let mut toks = tokenize("${!#}").unwrap();
        let part = single_param_expansion(&mut toks);
        assert!(matches!(part,
            WordPart::ParamExpansion { ref name, indirect: true, .. } if name == "#"),
            "got {part:?}");
    }

    #[test]
    fn indirect_of_special_param_star_at() {
        // v233 M2: `${!*}` / `${!@}` / `${!?}` route to special-param indirect
        // (NOT PrefixNames, NOT BadSubst). Distinct from `${!pfx*}` prefix form.
        for (src, want) in [("${!*}", "*"), ("${!@}", "@"), ("${!?}", "?")] {
            let mut toks = tokenize(src).unwrap();
            let part = single_param_expansion(&mut toks);
            assert!(matches!(part,
                WordPart::ParamExpansion { ref name, indirect: true, modifier: ParamModifier::None, .. } if name == want),
                "{src} -> got {part:?}");
        }
    }

    #[test]
    fn indirect_special_dollar_bang_stay_badsubst() {
        // v233 M2: `${!$}` / `${!!}` are bad substitutions in bash — they must
        // NOT route to special-param indirect. They scan to `}` and defer.
        for src in ["${!$}", "${!!}"] {
            let mut toks = tokenize(src).unwrap();
            let part = single_param_expansion(&mut toks);
            // recover_bad_subst emits a ParamExpansion carrying a BadSubst marker.
            assert!(matches!(part, WordPart::ParamExpansion { modifier: ParamModifier::BadSubst { .. }, .. }),
                "{src} -> expected BadSubst, got {part:?}");
        }
    }

    #[test]
    fn tokenize_braced_dash_bare() {
        // v102: `${-}` — option-flags special param, no modifier. Like
        // `${a}`, the bare form is emitted as a plain Var, not ParamExpansion.
        let tokens = tokenize("${-}").unwrap();
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        let WordPart::Var { name, .. } = &parts[0]
        else { panic!("expected Var, got {:?}", parts[0]) };
        assert_eq!(name, "-");
    }

    #[test]
    fn tokenize_braced_dash_remove_prefix() {
        // v102: `${-#*e}` — nvm's errexit driver, RemovePrefix modifier.
        let tokens = tokenize("${-#*e}").unwrap();
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        let WordPart::ParamExpansion { name, modifier, indirect, .. } = &parts[0]
        else { panic!("expected ParamExpansion, got {:?}", parts[0]) };
        assert_eq!(name, "-");
        assert!(!(*indirect));
        assert!(matches!(modifier, ParamModifier::RemovePrefix { longest: false, .. }));
    }

    #[test]
    fn tokenize_braced_status_bare() {
        // v102: `${?}` — exit-status special param, bare form is a Var.
        let tokens = tokenize("${?}").unwrap();
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        let WordPart::Var { name, .. } = &parts[0]
        else { panic!("expected Var, got {:?}", parts[0]) };
        assert_eq!(name, "?");
    }

    #[test]
    fn tokenize_braced_pid_bare() {
        // v102: `${$}` — shell-pid special param, bare form is a Var.
        let tokens = tokenize("${$}").unwrap();
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        let WordPart::Var { name, .. } = &parts[0]
        else { panic!("expected Var, got {:?}", parts[0]) };
        assert_eq!(name, "$");
    }

    #[test]
    fn tokenize_braced_bgpid_bare() {
        // v102: bare `${!}` is the `$!` last-bg-pid special param,
        // emitted as a plain Var, NOT the v95 indirect path.
        let tokens = tokenize("${!}").unwrap();
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        let WordPart::Var { name, .. } = &parts[0]
        else { panic!("expected Var, got {:?}", parts[0]) };
        assert_eq!(name, "!");
    }

    #[test]
    fn tokenize_braced_indirect_still_indirect() {
        // Regression: `${!var}` (non-`}` after `!`) stays the v95 indirect path.
        let tokens = tokenize("${!var}").unwrap();
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        let WordPart::ParamExpansion { name, indirect, .. } = &parts[0]
        else { panic!("expected ParamExpansion, got {:?}", parts[0]) };
        assert_eq!(name, "var");
        assert!(*indirect);
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
            WordPart::ParamExpansion { name, modifier, quoted, .. } => {
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
        let [WordPart::Quoted { style: QuoteStyle::Double, parts: inner }] = &parts[..]
        else { panic!("expected one double-quote run: {parts:?}") };
        let WordPart::ParamExpansion { quoted, .. } = &inner[0] else { panic!() };
        assert!(*quoted);
    }

    #[test]
    fn tokenize_invalid_modifier_parses_as_substring() {
        // ${X:&Y}: `:` followed by `&` — `&` is not `-=?+` so falls through
        // to substring dispatch; after v84, `&` in the operand is literal
        // (no longer InvalidBraceOperand). The result is a Substring expansion
        // with offset "&Y" — parses successfully (arith eval errors later at
        // runtime when `&Y` is not a valid arith expression).
        match tokenize("${X:&Y}") {
            Ok(_) => {} // fine — operand parsed as word
            Err(e) => panic!("unexpected error after v84: {e:?}"),
        }
    }

    #[test]
    fn tokenize_empty_param_name_errors() {
        // v233: `${:-foo}` has an empty param name before `:` — now a runtime
        // BadSubst (matching bash) rather than a parse error.
        let toks = tokenize("${:-foo}").unwrap();
        let Token::Word(Word(parts)) = &toks[0] else { panic!() };
        assert!(matches!(&parts[0],
            WordPart::ParamExpansion { modifier: ParamModifier::BadSubst { .. }, .. }
        ), "expected BadSubst, got {:?}", parts[0]);
    }

    #[test]
    fn tokenize_unterminated_brace_modifier_errors() {
        let err = tokenize("${X:-foo").unwrap_err();
        assert_eq!(err, LexError::UnterminatedBrace);
    }

    #[test]
    fn tokenize_pipe_in_operand_ok() {
        // After v84: pipe in operand is literal — ${X:-foo | bar} parses successfully.
        let tokens = tokenize("${X:-foo | bar}").unwrap();
        assert_eq!(tokens.len(), 1);
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        // Should be a ParamExpansion with UseDefault modifier.
        assert!(matches!(parts[0], WordPart::ParamExpansion { .. }));
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
            vec![wqd("a\nb")]
        );
    }

    #[test]
    fn newline_inside_single_quotes_stays_literal() {
        let tokens = tokenize("'a\nb'").unwrap();
        assert_eq!(
            tokens,
            vec![wq("a\nb")]
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
            vec![Token::Word(Word(vec![qdouble(vec![WordPart::AllArgs {
                joined: false, quoted: true
            }])]))]
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
        let [WordPart::Quoted { style: QuoteStyle::Double, parts: inner }] = &parts[..]
        else { panic!("expected one double-quote run: {parts:?}") };
        assert!(matches!(&inner[0], WordPart::Var { name, quoted: true } if name == "$"));
    }

    #[test]
    fn lexer_dollar_bang_inside_double_quotes() {
        let tokens = tokenize("\"$!\"").unwrap();
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        let [WordPart::Quoted { style: QuoteStyle::Double, parts: inner }] = &parts[..]
        else { panic!("expected one double-quote run: {parts:?}") };
        assert!(matches!(&inner[0], WordPart::Var { name, quoted: true } if name == "!"));
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
        let [WordPart::Quoted { style: QuoteStyle::Double, parts: inner }] = &parts[..]
        else { panic!("expected one double-quote run: {parts:?}") };
        assert!(matches!(&inner[0], WordPart::Literal { text, quoted: true } if text == "hi there"));
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
    // >| clobber redirect tests (v123)
    // ──────────────────────────────────────────────────────────────

    #[test]
    fn lex_clobber_stdout() {
        assert_eq!(tokenize(">|").unwrap(), vec![Token::Op(Operator::RedirClobber)]);
        assert_eq!(tokenize("1>|").unwrap(), vec![Token::Op(Operator::RedirClobber)]);
    }

    #[test]
    fn lex_clobber_stderr() {
        assert_eq!(tokenize("2>|").unwrap(), vec![Token::Op(Operator::RedirErrClobber)]);
    }

    #[test]
    fn lex_clobber_with_target() {
        assert_eq!(
            tokenize("cmd >|f").unwrap(),
            vec![w("cmd"), Token::Op(Operator::RedirClobber), w("f")]
        );
    }

    #[test]
    fn lex_redir_then_pipe_unaffected() {
        assert_eq!(
            tokenize("> |").unwrap(),
            vec![Token::Op(Operator::RedirOut), Token::Op(Operator::Pipe)]
        );
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
        if let WordPart::ParamExpansion { name, modifier: ParamModifier::Substring { offset, length }, quoted, .. } = part {
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
        // After v84 the operand is parsed as a word (char-walk), so the
        // leading space is preserved as a literal " -3"; the arith evaluator
        // handles the leading whitespace at runtime (${name: -3} == ${name: -3}).
        let mut t = tokenize_words("${name: -3}").unwrap();
        let part = single_param_expansion(&mut t);
        if let WordPart::ParamExpansion { modifier: ParamModifier::Substring { offset, .. }, .. } = part {
            // Leading space is now present in the literal (arith eval trims it).
            let lit = word_to_literal(&offset);
            assert!(lit == "-3" || lit == " -3", "unexpected offset literal: {lit:?}");
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
        // v233: `${var:}` — colon followed immediately by close brace — is
        // now a runtime BadSubst (matching bash) rather than a parse error.
        let toks = tokenize_words("${name:}").unwrap();
        let Token::Word(Word(parts)) = &toks[0] else { panic!() };
        assert!(matches!(&parts[0],
            WordPart::ParamExpansion { modifier: ParamModifier::BadSubst { raw }, .. }
            if raw == "${name:}"
        ), "expected BadSubst, got {:?}", parts[0]);
    }

    #[test]
    fn brace_case_upper_all() {
        let mut t = tokenize_words("${name^^}").unwrap();
        let part = single_param_expansion(&mut t);
        if let WordPart::ParamExpansion { name, modifier: ParamModifier::Case { direction, all, pattern }, quoted, .. } = part {
            assert_eq!(name, "name");
            assert!(!quoted);
            assert_eq!(direction, CaseDirection::Upper);
            assert!(all);
            assert!(pattern.is_none());
        } else { panic!("expected Case, got {part:?}") }
    }

    #[test]
    fn brace_case_upper_first() {
        let mut t = tokenize_words("${name^}").unwrap();
        let part = single_param_expansion(&mut t);
        if let WordPart::ParamExpansion { modifier: ParamModifier::Case { direction, all, pattern }, .. } = part {
            assert_eq!(direction, CaseDirection::Upper);
            assert!(!all);
            assert!(pattern.is_none());
        } else { panic!("expected Case") }
    }

    #[test]
    fn brace_case_lower_all() {
        let mut t = tokenize_words("${name,,}").unwrap();
        let part = single_param_expansion(&mut t);
        if let WordPart::ParamExpansion { modifier: ParamModifier::Case { direction, all, pattern }, .. } = part {
            assert_eq!(direction, CaseDirection::Lower);
            assert!(all);
            assert!(pattern.is_none());
        } else { panic!("expected Case") }
    }

    #[test]
    fn brace_case_lower_first() {
        let mut t = tokenize_words("${name,}").unwrap();
        let part = single_param_expansion(&mut t);
        if let WordPart::ParamExpansion { modifier: ParamModifier::Case { direction, all, pattern }, .. } = part {
            assert_eq!(direction, CaseDirection::Lower);
            assert!(!all);
            assert!(pattern.is_none());
        } else { panic!("expected Case") }
    }

    #[test]
    fn brace_case_upper_all_with_pattern() {
        let mut t = tokenize_words("${name^^[aeiou]}").unwrap();
        let part = single_param_expansion(&mut t);
        if let WordPart::ParamExpansion { modifier: ParamModifier::Case { direction, all, pattern }, .. } = part {
            assert_eq!(direction, CaseDirection::Upper);
            assert!(all);
            let p = pattern.expect("pattern");
            assert_eq!(word_to_literal(&p), "[aeiou]");
        } else { panic!("expected Case") }
    }

    #[test]
    fn brace_case_positional() {
        // `${1^^}` — emits ParamExpansion (not Var) so the modifier runs.
        let mut t = tokenize_words("${1^^}").unwrap();
        let part = single_param_expansion(&mut t);
        if let WordPart::ParamExpansion { name, modifier: ParamModifier::Case { all, .. }, .. } = part {
            assert_eq!(name, "1");
            assert!(all);
        } else { panic!("expected Case on positional, got {part:?}") }
    }

    #[test]
    fn brace_case_unterminated_is_error() {
        assert!(matches!(
            tokenize_words("${name^^"),
            Err(LexError::UnterminatedBrace)
        ));
    }

    #[test]
    fn brace_length_positional() {
        let mut t = tokenize_words("${#1}").unwrap();
        let part = single_param_expansion(&mut t);
        if let WordPart::ParamExpansion { name, modifier, quoted, .. } = part {
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

    #[test]
    fn ansi_c_quote_newline_escape() {
        let toks = tokenize("$'a\\nb'").expect("lex");
        // Single Word token with one quoted Literal containing "a\nb"
        match &toks[0] {
            Token::Word(Word(parts)) => {
                assert_eq!(parts.len(), 1);
                match ansi_c_inner(&parts[0]) {
                    WordPart::Literal { text, quoted } => {
                        assert_eq!(text, "a\nb");
                        assert!(*quoted, "expected quoted Literal");
                    }
                    other => panic!("expected Literal, got {:?}", other),
                }
            }
            other => panic!("expected Word token, got {:?}", other),
        }
    }

    #[test]
    fn ansi_c_quote_tab_escape() {
        let toks = tokenize("$'a\\tb'").expect("lex");
        let Token::Word(Word(parts)) = &toks[0] else { panic!("expected Word") };
        assert_eq!(parts.len(), 1);
        let WordPart::Literal { text, quoted } = ansi_c_inner(&parts[0]) else { panic!("expected Literal") };
        assert_eq!(text, "a\tb");
        assert!(*quoted);
    }

    #[test]
    fn ansi_c_quote_backslash_and_quote() {
        // $'\\\'' → literal backslash + literal quote (two chars)
        let toks = tokenize("$'\\\\\\''").expect("lex");
        let Token::Word(Word(parts)) = &toks[0] else { panic!("expected Word") };
        assert_eq!(parts.len(), 1);
        let WordPart::Literal { text, .. } = ansi_c_inner(&parts[0]) else { panic!("expected Literal") };
        assert_eq!(text, "\\'");
    }

    #[test]
    fn ansi_c_quote_hex_escapes() {
        // \x48\x69 → "Hi"
        let toks = tokenize("$'\\x48\\x69'").expect("lex");
        let Token::Word(Word(parts)) = &toks[0] else { panic!("expected Word") };
        let WordPart::Literal { text, .. } = ansi_c_inner(&parts[0]) else { panic!("expected Literal") };
        assert_eq!(text, "Hi");
    }

    #[test]
    fn ansi_c_quote_octal_escapes() {
        // \110\151 → "Hi"  (0o110=72='H', 0o151=105='i')
        let toks = tokenize("$'\\110\\151'").expect("lex");
        let Token::Word(Word(parts)) = &toks[0] else { panic!("expected Word") };
        let WordPart::Literal { text, .. } = ansi_c_inner(&parts[0]) else { panic!("expected Literal") };
        assert_eq!(text, "Hi");
    }

    #[test]
    fn ansi_c_quote_octal_greedy_stops_at_non_octal() {
        // \18 → \1 followed by literal '8' → "\x01" + "8"
        let toks = tokenize("$'\\18'").expect("lex");
        let Token::Word(Word(parts)) = &toks[0] else { panic!("expected Word") };
        let WordPart::Literal { text, .. } = ansi_c_inner(&parts[0]) else { panic!("expected Literal") };
        assert_eq!(text, "\x018");
    }

    #[test]
    fn ansi_c_quote_unicode_4digit() {
        // é → é (U+00E9, "LATIN SMALL LETTER E WITH ACUTE")
        let toks = tokenize("$'\\u00e9'").expect("lex");
        let Token::Word(Word(parts)) = &toks[0] else { panic!("expected Word") };
        let WordPart::Literal { text, .. } = ansi_c_inner(&parts[0]) else { panic!("expected Literal") };
        assert_eq!(text, "é");
    }

    #[test]
    fn ansi_c_quote_unicode_8digit() {
        // \U0001F600 → 😀 (grinning face)
        let toks = tokenize("$'\\U0001F600'").expect("lex");
        let Token::Word(Word(parts)) = &toks[0] else { panic!("expected Word") };
        let WordPart::Literal { text, .. } = ansi_c_inner(&parts[0]) else { panic!("expected Literal") };
        assert_eq!(text, "\u{1F600}");
    }

    #[test]
    fn ansi_c_quote_control_chars() {
        // \cA → \x01, \cZ → \x1A
        let toks = tokenize("$'\\cA\\cZ'").expect("lex");
        let Token::Word(Word(parts)) = &toks[0] else { panic!("expected Word") };
        let WordPart::Literal { text, .. } = ansi_c_inner(&parts[0]) else { panic!("expected Literal") };
        assert_eq!(text, "\x01\x1a");
    }

    #[test]
    fn ansi_c_quote_unknown_escape_preserves_both() {
        // \q → literal "\q" (two chars)
        let toks = tokenize("$'\\q'").expect("lex");
        let Token::Word(Word(parts)) = &toks[0] else { panic!("expected Word") };
        let WordPart::Literal { text, .. } = ansi_c_inner(&parts[0]) else { panic!("expected Literal") };
        assert_eq!(text, "\\q");
    }

    #[test]
    fn ansi_c_quote_empty() {
        let toks = tokenize("$''").expect("lex");
        let Token::Word(Word(parts)) = &toks[0] else { panic!("expected Word") };
        assert_eq!(parts.len(), 1);
        let WordPart::Literal { text, quoted } = ansi_c_inner(&parts[0]) else { panic!("expected Literal") };
        assert_eq!(text, "");
        assert!(*quoted);
    }

    #[test]
    fn ansi_c_quote_unterminated_is_error() {
        let err = tokenize("$'foo").unwrap_err();
        assert_eq!(err, LexError::UnterminatedQuote);
    }

    #[test]
    fn ansi_c_quote_invalid_codepoint_is_error() {
        // \uD800 is a surrogate, not a valid Unicode scalar
        let err = tokenize("$'\\uD800'").unwrap_err();
        assert_eq!(err, LexError::AnsiCInvalidCodepoint(0xD800));
    }

    #[test]
    fn ansi_c_quote_concatenates_with_adjacent_word() {
        // $'a\nb'foo → single Word with two Literal parts
        let toks = tokenize("$'a\\nb'foo").expect("lex");
        let Token::Word(Word(parts)) = &toks[0] else { panic!("expected Word") };
        assert_eq!(parts.len(), 2);
        let WordPart::Literal { text, quoted } = ansi_c_inner(&parts[0]) else { panic!("expected Literal at [0]") };
        assert_eq!(text, "a\nb");
        assert!(*quoted);
        let WordPart::Literal { text, quoted } = &parts[1] else { panic!("expected Literal at [1]") };
        assert_eq!(text, "foo");
        assert!(!*quoted);
    }

    #[test]
    fn tokenize_brace_emits_multiple_words() {
        let toks = tokenize("echo {a,b,c}").expect("lex");
        // Should produce 4 Word tokens: echo, a, b, c (plus any
        // separators we don't care about).
        let word_texts: Vec<String> = toks
            .iter()
            .filter_map(|t| match t {
                Token::Word(Word(parts)) => {
                    let s: String = parts
                        .iter()
                        .filter_map(|p| match p {
                            WordPart::Literal { text, .. } => Some(text.clone()),
                            _ => None,
                        })
                        .collect();
                    Some(s)
                }
                _ => None,
            })
            .collect();
        assert_eq!(word_texts, vec!["echo", "a", "b", "c"]);
    }

    #[test]
    fn tokenize_brace_preserves_var() {
        let toks = tokenize("echo $x{a,b}").expect("lex");
        // First word is `echo`. Then two more Words, each with
        // a Var part followed by a Literal part.
        let word_tokens: Vec<&Vec<WordPart>> = toks
            .iter()
            .filter_map(|t| match t {
                Token::Word(Word(parts)) => Some(parts),
                _ => None,
            })
            .collect();
        assert_eq!(word_tokens.len(), 3);
        // word_tokens[0] is `echo` (one Literal part).
        // word_tokens[1] and [2] are Var+Literal pairs.
        for w in &word_tokens[1..] {
            assert!(matches!(w[0], WordPart::Var { .. }));
            assert!(matches!(w[1], WordPart::Literal { quoted: false, .. }));
        }
    }

    #[test]
    fn tokenize_quoted_brace_not_expanded() {
        let toks = tokenize("echo \"{a,b}\"").expect("lex");
        let word_count = toks.iter().filter(|t| matches!(t, Token::Word(_))).count();
        assert_eq!(word_count, 2, "expected 2 Words (echo + the quoted literal), got {word_count}");
    }

    #[test]
    fn tokenize_single_quoted_brace_not_expanded() {
        let toks = tokenize("echo '{a,b}'").expect("lex");
        let word_count = toks.iter().filter(|t| matches!(t, Token::Word(_))).count();
        assert_eq!(word_count, 2, "expected 2 Words, got {word_count}");
    }

    #[test]
    fn tokenize_backslash_brace_not_expanded() {
        // The lexer's `\X` arm pushes each escaped char as a
        // one-char QUOTED Literal (quoted: true). Brace expansion
        // only fires on UNQUOTED Literals, so `\{a,b\}` survives
        // as a single Word.
        let toks = tokenize("echo \\{a,b\\}").expect("lex");
        let word_count = toks.iter().filter(|t| matches!(t, Token::Word(_))).count();
        assert_eq!(word_count, 2, "expected 2 Words, got {word_count}");
    }

    #[test]
    fn arith_block_simple() {
        let tokens = tokenize("((1+2))").unwrap();
        assert_eq!(tokens.len(), 1);
        match &tokens[0] {
            Token::ArithBlock(s, _) => assert_eq!(s, "1+2"),
            other => panic!("expected ArithBlock, got {other:?}"),
        }
    }

    #[test]
    fn scan_arith_block_bails_on_unbalanced_close() {
        // v185 (L-51): a `)` at depth 0 not forming `))` means the `((` can't be
        // a balanced arith block — bail (Err) immediately instead of scanning on
        // for a distant `))`. The caller then falls back to nested subshells.
        let mut chars = CharCursor::new("echo a) z))");
        assert!(scan_arith_block(&mut chars).is_err());
    }

    #[test]
    fn scan_arith_block_valid_inner_group() {
        // Regression: a valid arith block whose content closes a paren group
        // (`(a)`) before the final `))` still scans — the inner `)` is processed
        // at depth 1 (decrement 1->0), never the depth-0 bail branch.
        let mut chars = CharCursor::new("(a)+1))");
        assert_eq!(scan_arith_block(&mut chars).unwrap(), "(a)+1");
    }

    #[test]
    fn double_paren_no_wander_to_distant_close() {
        // v185 (L-51): `((echo a)|cat)` has no matching `))`; the scanner must
        // NOT wander to a later `$((1+1))`'s `))`. The head lexes as nested
        // subshells (two LParens), not an ArithBlock.
        let toks = tokenize("((echo a)|cat); x=$((1+1))").unwrap();
        assert!(
            !matches!(toks[0], Token::ArithBlock(..)),
            "head must not be an ArithBlock: {toks:?}"
        );
        assert!(matches!(toks[0], Token::Op(Operator::LParen)));
        assert!(matches!(toks[1], Token::Op(Operator::LParen)));
    }

    #[test]
    fn double_paren_nested_subshell_not_arith() {
        // v184: `((echo a) | cat)` has no matching `))` → nested subshells `( (`,
        // NOT an arithmetic block. Lexes to two LParens, no ArithBlock.
        let toks = tokenize("((echo a) | cat)").unwrap();
        assert!(
            !toks.iter().any(|t| matches!(t, Token::ArithBlock(..))),
            "must not be an ArithBlock: {toks:?}"
        );
        assert!(matches!(toks[0], Token::Op(Operator::LParen)));
        assert!(matches!(toks[1], Token::Op(Operator::LParen)));
    }

    #[test]
    fn double_paren_real_arith_still_arithblock() {
        // v184 regression: a `((` that DOES close as `))` stays an ArithBlock.
        let toks = tokenize("((1+2))").unwrap();
        assert_eq!(toks.len(), 1);
        assert!(matches!(toks[0], Token::ArithBlock(..)));
    }

    #[test]
    fn double_paren_deep_nesting_not_arith() {
        // v184: `((( echo a ) ) )` — the closing parens are not adjacent, so no
        // `))` for the outer `((` → LParens, not an ArithBlock.
        let toks = tokenize("((( echo a ) ) )").unwrap();
        assert!(
            !toks.iter().any(|t| matches!(t, Token::ArithBlock(..))),
            "must not be an ArithBlock: {toks:?}"
        );
    }

    #[test]
    fn arith_block_with_semicolons() {
        let tokens = tokenize("((a;b;c))").unwrap();
        assert_eq!(tokens.len(), 1);
        match &tokens[0] {
            Token::ArithBlock(s, _) => assert_eq!(s, "a;b;c"),
            other => panic!("expected ArithBlock, got {other:?}"),
        }
    }

    #[test]
    fn arith_block_nested_parens() {
        // Outer `((` / `))` is the delimiter; inner parens belong to the body.
        // Body has TWO layers of inner parens — the matching `))` close
        // is the final two `)` of the input.
        let tokens = tokenize("((((a+b)*c)))").unwrap();
        assert_eq!(tokens.len(), 1);
        match &tokens[0] {
            Token::ArithBlock(s, _) => assert_eq!(s, "((a+b)*c)"),
            other => panic!("expected ArithBlock, got {other:?}"),
        }
    }

    #[test]
    fn arith_block_with_internal_whitespace() {
        let tokens = tokenize("((  1 + 2  ))").unwrap();
        assert_eq!(tokens.len(), 1);
        match &tokens[0] {
            Token::ArithBlock(s, _) => assert_eq!(s, "  1 + 2  "),
            other => panic!("expected ArithBlock, got {other:?}"),
        }
    }

    #[test]
    fn arith_block_empty_body() {
        let tokens = tokenize("(())").unwrap();
        assert_eq!(tokens.len(), 1);
        match &tokens[0] {
            Token::ArithBlock(s, _) => assert_eq!(s, ""),
            other => panic!("expected ArithBlock, got {other:?}"),
        }
    }

    #[test]
    fn arith_block_unclosed_falls_back_to_lparens() {
        // `((1+2` — no closing `))`. As of v184, an unterminated `((` is no
        // longer a lex error: it rewinds and emits a single LParen (the second
        // `(` re-lexes as another), so this lexes as nested subshells `( (`.
        // bash treats `((` as arithmetic only when a matching `))` is found.
        let toks = tokenize("((1+2").unwrap();
        assert!(
            !toks.iter().any(|t| matches!(t, Token::ArithBlock(..))),
            "must not be an ArithBlock: {toks:?}"
        );
        assert!(matches!(toks[0], Token::Op(Operator::LParen)));
        assert!(matches!(toks[1], Token::Op(Operator::LParen)));
    }

    #[test]
    fn arith_block_single_paren_at_end_falls_back_to_lparens() {
        // `((1+2)` — one closing paren, not two, so no matching `))`. As of
        // v184 this falls back to nested subshells `( (` rather than erroring.
        let toks = tokenize("((1+2)").unwrap();
        assert!(
            !toks.iter().any(|t| matches!(t, Token::ArithBlock(..))),
            "must not be an ArithBlock: {toks:?}"
        );
        assert!(matches!(toks[0], Token::Op(Operator::LParen)));
        assert!(matches!(toks[1], Token::Op(Operator::LParen)));
    }

    #[test]
    fn space_between_parens_is_not_arith_block() {
        // `( (cmd) )` — whitespace between the two `(`s. Should tokenize
        // as two LParen ops, a Word, and two RParen ops (nested-subshell
        // path per M-11). The arith-block detector must NOT fire.
        let tokens = tokenize("( (cmd) )").unwrap();
        let lparen_count = tokens
            .iter()
            .filter(|t| matches!(t, Token::Op(Operator::LParen)))
            .count();
        let arith_count = tokens
            .iter()
            .filter(|t| matches!(t, Token::ArithBlock(..)))
            .count();
        assert_eq!(lparen_count, 2, "expected two LParen tokens: {tokens:?}");
        assert_eq!(arith_count, 0, "did not expect ArithBlock: {tokens:?}");
    }
}

#[cfg(test)]
mod array_parse_tests {
    use super::*;
    use crate::command::{AssignTarget, Assignment, Command, SimpleCommand};

    /// Parse a single line and return its first SimpleCommand's
    /// assignments. Works for both `name=value` standalone and
    /// `name=value cmd args` inline-prefix shapes.
    fn parse_assignments(input: &str) -> Vec<Assignment> {
        let tokens = crate::lexer::tokenize(input).expect("lex");
        let seq = crate::command::parse(tokens).expect("parse").expect("non-empty");
        let pipeline = match seq.first {
            Command::Pipeline(p) => p,
            other => panic!("expected Pipeline, got {other:?}"),
        };
        match &pipeline.commands[0] {
            Command::Simple(SimpleCommand::Assign(items, _)) => items.clone(),
            Command::Simple(SimpleCommand::Exec(e)) => e.inline_assignments.clone(),
            other => panic!("expected SimpleCommand, got {other:?}"),
        }
    }

    /// Walk a Word looking for the first WordPart::ParamExpansion whose
    /// name matches.
    fn find_param_expansion(input: &str, name: &str) -> WordPart {
        let tokens = crate::lexer::tokenize(input).expect("lex");
        let seq = crate::command::parse(tokens).expect("parse").expect("non-empty");
        let pipeline = match seq.first {
            Command::Pipeline(p) => p,
            other => panic!("expected Pipeline, got {other:?}"),
        };
        for cmd in &pipeline.commands {
            if let Command::Simple(SimpleCommand::Exec(e)) = cmd {
                for w in std::iter::once(&e.program).chain(e.args.iter()) {
                    // Flatten one level of quoted runs so a `"${a[@]}"` expansion
                    // (now nested inside a double-quote run) is still found.
                    let flat = w.0.iter().flat_map(|p| match p {
                        WordPart::Quoted { parts, .. } => parts.iter().collect::<Vec<_>>(),
                        other => vec![other],
                    });
                    for part in flat {
                        if let WordPart::ParamExpansion { name: n, .. } = part
                            && n == name
                        {
                            return part.clone();
                        }
                    }
                }
            }
        }
        panic!("ParamExpansion for {name} not found");
    }

    #[test]
    fn compound_rhs_is_array_literal() {
        let assigns = parse_assignments("a=(x y z)");
        assert_eq!(assigns.len(), 1);
        assert_eq!(assigns[0].target.name(), "a");
        assert!(!assigns[0].append);
        // Value: [Literal(""), ArrayLiteral([x, y, z])].
        // (Bare `name=` keeps the existing Literal-`name=` prefix shape;
        // the rest-of-first-Literal is the empty string before the open
        // paren, then ArrayLiteral follows.)
        let parts = &assigns[0].value.0;
        let array_part = parts.iter().find_map(|p| match p {
            WordPart::ArrayLiteral(els) => Some(els),
            _ => None,
        });
        let els = array_part.expect("ArrayLiteral part present");
        assert_eq!(els.len(), 3);
        assert!(els.iter().all(|e| e.subscript.is_none()));
    }

    #[test]
    fn array_assignment_with_line_continuation() {
        // `arr=\<NL>(a b c)` — the \<NL> between `=` and `(` is a line
        // continuation (deleted pre-tokenization), so this is `arr=(a b c)`.
        let assigns = parse_assignments("arr=\\\n(a b c)");
        assert_eq!(assigns.len(), 1);
        assert_eq!(assigns[0].target.name(), "arr");
        assert!(!assigns[0].append);
        let els = assigns[0]
            .value
            .0
            .iter()
            .find_map(|p| match p {
                WordPart::ArrayLiteral(els) => Some(els),
                _ => None,
            })
            .expect("ArrayLiteral part present");
        assert_eq!(els.len(), 3);
    }

    #[test]
    fn array_append_with_line_continuation() {
        let assigns = parse_assignments("arr+=\\\n(d)");
        assert_eq!(assigns.len(), 1);
        assert!(assigns[0].append);
        let els = assigns[0]
            .value
            .0
            .iter()
            .find_map(|p| match p {
                WordPart::ArrayLiteral(els) => Some(els),
                _ => None,
            })
            .expect("ArrayLiteral part present");
        assert_eq!(els.len(), 1);
    }

    #[test]
    fn array_assignment_with_line_continuation_between_elements() {
        // `arr=([a]=1 \<NL> [b]=2)` — the \<NL> BETWEEN elements is a separator,
        // not the start of a bare element. Both subscripted elements survive
        // (previously the \<NL> produced a spurious no-subscript element).
        let assigns = parse_assignments("arr=([a]=1 \\\n [b]=2)");
        assert_eq!(assigns.len(), 1);
        let els = assigns[0]
            .value
            .0
            .iter()
            .find_map(|p| match p {
                WordPart::ArrayLiteral(els) => Some(els),
                _ => None,
            })
            .expect("ArrayLiteral part present");
        assert_eq!(els.len(), 2, "two subscripted elements, no spurious one");
        assert!(els.iter().all(|e| e.subscript.is_some()),
            "every element keeps its [key]= subscript");
    }

    #[test]
    fn array_assignment_with_stacked_line_continuations() {
        // `arr=\<NL>\<NL>(x)` — two stacked continuations, both deleted, so this
        // is `arr=(x)` (exercises the loop in skip_line_continuations).
        let assigns = parse_assignments("arr=\\\n\\\n(x)");
        assert_eq!(assigns.len(), 1);
        let els = assigns[0]
            .value
            .0
            .iter()
            .find_map(|p| match p {
                WordPart::ArrayLiteral(els) => Some(els),
                _ => None,
            })
            .expect("ArrayLiteral part present");
        assert_eq!(els.len(), 1);
    }

    #[test]
    fn backslash_escape_after_eq_is_not_continuation() {
        // `arr=\x` — `\x` is a literal escape, NOT a continuation; no array.
        let assigns = parse_assignments("arr=\\x");
        assert_eq!(assigns.len(), 1);
        assert!(
            !assigns[0]
                .value
                .0
                .iter()
                .any(|p| matches!(p, WordPart::ArrayLiteral(_))),
            "a backslash-escape must not be treated as a line continuation"
        );
    }

    #[test]
    fn sparse_compound_rhs_carries_subscripts() {
        let assigns = parse_assignments("a=([5]=x [2]=y)");
        let array_part = assigns[0].value.0.iter().find_map(|p| match p {
            WordPart::ArrayLiteral(els) => Some(els),
            _ => None,
        });
        let els = array_part.expect("ArrayLiteral part present");
        assert_eq!(els.len(), 2);
        assert!(els[0].subscript.is_some());
        assert!(els[1].subscript.is_some());
    }

    #[test]
    fn subscripted_lvalue_parses() {
        let assigns = parse_assignments("a[5]=v");
        assert_eq!(assigns.len(), 1);
        match &assigns[0].target {
            AssignTarget::Indexed { name, .. } => assert_eq!(name, "a"),
            other => panic!("expected Indexed, got {other:?}"),
        }
        assert!(!assigns[0].append);
    }

    #[test]
    fn subscripted_lvalue_append_parses() {
        let assigns = parse_assignments("a[5]+=v");
        match &assigns[0].target {
            AssignTarget::Indexed { name, .. } => assert_eq!(name, "a"),
            other => panic!("expected Indexed, got {other:?}"),
        }
        assert!(assigns[0].append);
    }

    #[test]
    fn compound_append_parses() {
        let assigns = parse_assignments("a+=(x y)");
        match &assigns[0].target {
            AssignTarget::Bare(n) => assert_eq!(n, "a"),
            other => panic!("expected Bare, got {other:?}"),
        }
        assert!(assigns[0].append);
        let array_part = assigns[0].value.0.iter().find_map(|p| match p {
            WordPart::ArrayLiteral(els) => Some(els),
            _ => None,
        });
        assert!(array_part.is_some(), "expected ArrayLiteral part");
    }

    #[test]
    fn subscripted_ref_at_all() {
        let pe = find_param_expansion(r#"echo "${a[@]}""#, "a");
        match pe {
            WordPart::ParamExpansion { subscript, .. } => {
                assert!(matches!(subscript, Some(SubscriptKind::All)));
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn subscripted_ref_at_star() {
        let pe = find_param_expansion(r#"echo "${a[*]}""#, "a");
        match pe {
            WordPart::ParamExpansion { subscript, .. } => {
                assert!(matches!(subscript, Some(SubscriptKind::Star)));
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn subscripted_ref_index_carries_word() {
        let pe = find_param_expansion("echo ${a[3]}", "a");
        match pe {
            WordPart::ParamExpansion { subscript, .. } => {
                assert!(matches!(subscript, Some(SubscriptKind::Index(_))));
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn bare_param_expansion_has_no_subscript() {
        // `${a}` (no subscript) is emitted as WordPart::Var, NOT
        // ParamExpansion. Verify by checking that no ParamExpansion
        // appears at all.
        let tokens = crate::lexer::tokenize("echo ${a}").expect("lex");
        let seq = crate::command::parse(tokens).expect("parse").expect("non-empty");
        let pipeline = match seq.first {
            Command::Pipeline(p) => p,
            _ => panic!(),
        };
        for cmd in &pipeline.commands {
            if let Command::Simple(SimpleCommand::Exec(e)) = cmd {
                for w in std::iter::once(&e.program).chain(e.args.iter()) {
                    for part in &w.0 {
                        if matches!(part, WordPart::ParamExpansion { .. }) {
                            panic!("expected Var, got ParamExpansion for ${{a}}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn unterminated_subscript_errors() {
        let result = crate::lexer::tokenize("echo ${a[3");
        assert!(
            matches!(result, Err(LexError::UnterminatedSubscript)),
            "expected UnterminatedSubscript, got {result:?}"
        );
    }

    #[test]
    fn unterminated_array_literal_errors() {
        let result = crate::lexer::tokenize("a=(x y");
        assert!(
            matches!(result, Err(LexError::UnterminatedArrayLiteral)),
            "expected UnterminatedArrayLiteral, got {result:?}"
        );
    }

    #[test]
    fn array_literal_subscript_missing_equals_errors() {
        let result = crate::lexer::tokenize("a=([3] x)");
        assert!(
            matches!(result, Err(LexError::ArrayLiteralMissingEquals)),
            "expected ArrayLiteralMissingEquals, got {result:?}"
        );
    }

    #[test]
    fn bare_subscripted_ref_has_none_modifier() {
        let pe = find_param_expansion("echo ${a[3]}", "a");
        if let WordPart::ParamExpansion { modifier, .. } = pe {
            assert!(
                matches!(modifier, ParamModifier::None),
                "expected ParamModifier::None, got {modifier:?}"
            );
        } else {
            panic!("expected ParamExpansion");
        }
    }

    #[test]
    fn parse_transform_ops() {
        for (src, want) in [
            ("${v@P}", TransformOp::PromptExpand),
            ("${v@Q}", TransformOp::Quote),
            ("${v@U}", TransformOp::Upper),
            ("${v@L}", TransformOp::Lower),
            ("${v@u}", TransformOp::UpperFirst),
            ("${v@E}", TransformOp::EscapeExpand),
            ("${v@A}", TransformOp::AssignDecl),
            ("${v@K}", TransformOp::KvString),
            ("${v@k}", TransformOp::KvWords),
            ("${v@a}", TransformOp::AttrFlags),
        ] {
            let parts = match &tokenize(src).unwrap()[0] {
                Token::Word(Word(p)) => p.clone(),
                _ => panic!(),
            };
            let WordPart::ParamExpansion {
                modifier: ParamModifier::Transform { op },
                ..
            } = &parts[0]
            else {
                panic!("expected Transform for {src}")
            };
            assert_eq!(*op, want);
        }
        // v233: `@Z` and other unknown letters are runtime BadSubst, not parse errors.
        let toks = tokenize("${v@Z}").unwrap();
        assert!(matches!(&toks[0],
            Token::Word(Word(p)) if matches!(&p[0],
                WordPart::ParamExpansion { modifier: ParamModifier::BadSubst { .. }, .. }
            )
        ), "expected BadSubst for @Z");
    }

    fn array_lit(w: &Word) -> &[ArrayLiteralElement] {
        w.0.iter()
            .find_map(|p| match p {
                WordPart::ArrayLiteral(els) => Some(els.as_slice()),
                _ => None,
            })
            .expect("ArrayLiteral part present")
    }

    #[test]
    fn array_literal_skips_comment_with_paren() {
        // v183: a `#` comment between elements (incl. one whose text contains
        // `)`) is skipped — the `)` must NOT close the array early.
        let assigns = parse_assignments("a=(\n# c )\n1\n)");
        let els = array_lit(&assigns[0].value);
        assert_eq!(els.len(), 1);
        assert!(els[0].subscript.is_none());
    }

    #[test]
    fn array_literal_midword_hash_is_literal() {
        // v183 regression: a `#` MID-word (`x#y`) is NOT a comment.
        let assigns = parse_assignments("a=(x#y z)");
        let els = array_lit(&assigns[0].value);
        assert_eq!(els.len(), 2);
    }

    #[test]
    fn brace_expand_parts_literal_splits() {
        let parts = vec![WordPart::Literal { text: "x{a,b}".to_string(), quoted: false }];
        let out = brace_expand_parts(parts).unwrap();
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn brace_expand_parts_no_brace_passthrough() {
        let parts = vec![WordPart::Literal { text: "plain".to_string(), quoted: false }];
        let out = brace_expand_parts(parts).unwrap();
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn array_literal_brace_expands_bare_range() {
        let assigns = parse_assignments("a=({1..3} z)");
        let els = array_lit(&assigns[0].value);
        assert_eq!(els.len(), 4); // 1 2 3 z
        assert!(els.iter().all(|e| e.subscript.is_none()));
    }

    #[test]
    fn array_literal_brace_cartesian() {
        let assigns = parse_assignments("a=({a,b}{1,2})");
        let els = array_lit(&assigns[0].value);
        assert_eq!(els.len(), 4); // a1 a2 b1 b2
    }

    #[test]
    fn array_literal_single_element_brace_is_literal() {
        let assigns = parse_assignments("a=({1} z)");
        let els = array_lit(&assigns[0].value);
        assert_eq!(els.len(), 2); // {1} stays one element
    }

    #[test]
    fn array_literal_quoted_brace_not_expanded() {
        let assigns = parse_assignments("a=(\"{1,2}\" x)");
        let els = array_lit(&assigns[0].value);
        assert_eq!(els.len(), 2); // "{1,2}" stays one element
    }

    #[test]
    fn array_literal_subscripted_brace_stays_single() {
        let assigns = parse_assignments("a=([2]=x{a,b} z)");
        let els = array_lit(&assigns[0].value);
        // subscripted element NOT brace-expanded (1) + bare `z` (1) = 2
        assert_eq!(els.len(), 2);
        assert!(els[0].subscript.is_some());
        assert!(els[1].subscript.is_none());
    }

    #[test]
    fn array_literal_brace_adjacent_var_keeps_expansion() {
        // `a=(x{1,2}$v)` must brace-expand AND keep $v as a real variable part
        // (one element per brace product, each with the Variable preserved).
        let assigns = parse_assignments("a=(x{1,2}$v)");
        let els = array_lit(&assigns[0].value);
        assert_eq!(els.len(), 2);
        // each element must still contain a Variable/expansion part (NOT a single
        // literal "x1$v") — assert no element is a lone Literal containing '$'.
        for e in els {
            let lone_literal_with_dollar = e.value.0.len() == 1
                && matches!(&e.value.0[0], WordPart::Literal { text, .. } if text.contains('$'));
            assert!(!lone_literal_with_dollar, "expansion was flattened to literal: {:?}", e.value.0);
        }
    }

    #[test]
    fn array_literal_brace_adjacent_cmdsub_keeps_expansion() {
        let assigns = parse_assignments("a=(x{1,2}$(echo Q))");
        let els = array_lit(&assigns[0].value);
        assert_eq!(els.len(), 2);
        for e in els {
            let lone_literal_with_dollar = e.value.0.len() == 1
                && matches!(&e.value.0[0], WordPart::Literal { text, .. } if text.contains('$'));
            assert!(!lone_literal_with_dollar, "cmdsub flattened to literal: {:?}", e.value.0);
        }
    }

    #[test]
    fn array_literal_pure_brace_still_expands() {
        // regression: pure-literal brace unchanged.
        let assigns = parse_assignments("a=({1..3} z)");
        assert_eq!(array_lit(&assigns[0].value).len(), 4);
    }

    // --- process substitution lexer tests ---

    #[test]
    fn process_sub_in_is_a_word_part() {
        let toks = tokenize("cat <(echo hi)").unwrap();
        let words: Vec<&Word> = toks.iter().filter_map(|t| match t {
            Token::Word(w) => Some(w), _ => None,
        }).collect();
        assert_eq!(words.len(), 2, "cat + one process-sub word");
        match &words[1].0[..] {
            [WordPart::ProcessSub { dir: ProcDir::In, .. }] => {}
            other => panic!("expected ProcessSub In, got {other:?}"),
        }
    }

    #[test]
    fn process_sub_out_direction() {
        let toks = tokenize("tee >(cat)").unwrap();
        let w = toks.iter().find_map(|t| match t {
            Token::Word(w) if matches!(w.0.first(), Some(WordPart::ProcessSub { .. })) => Some(w),
            _ => None,
        }).expect("a process-sub word");
        assert!(matches!(w.0[0], WordPart::ProcessSub { dir: ProcDir::Out, .. }));
    }

    #[test]
    fn quoted_process_sub_is_literal() {
        let toks = tokenize("echo \"<(echo hi)\"").unwrap();
        let has = toks.iter().any(|t| matches!(t,
            Token::Word(w) if w.0.iter().any(|p| matches!(p, WordPart::ProcessSub { .. }))));
        assert!(!has, "quoted <( must stay literal");
    }

    #[test]
    fn lone_redirect_still_redirect() {
        let toks = tokenize("cat < file").unwrap();
        assert!(toks.iter().any(|t| matches!(t, Token::Op(Operator::RedirIn))),
            "`< file` is still a redirect");
    }

    #[test]
    fn nested_process_sub_balances() {
        let toks = tokenize("cat <(cat <(echo deep))").unwrap();
        let outer = toks.iter().find_map(|t| match t {
            Token::Word(w) if matches!(w.0.first(), Some(WordPart::ProcessSub { .. })) => Some(w),
            _ => None,
        }).expect("outer process sub");
        assert!(matches!(outer.0[0], WordPart::ProcessSub { dir: ProcDir::In, .. }));
    }

    #[test]
    fn redirect_from_process_sub_tokenizes() {
        // `wc < <(cmd)` -> Word("wc"), Op(RedirIn), Word(ProcessSub{In})
        let toks = tokenize("wc < <(printf hi)").unwrap();
        assert!(toks.iter().any(|t| matches!(t, Token::Op(Operator::RedirIn))),
            "the standalone `<` is still a redirect operator");
        let last_word = toks.iter().rev().find_map(|t| match t {
            Token::Word(w) => Some(w), _ => None,
        }).expect("a trailing word");
        assert!(matches!(last_word.0.first(), Some(WordPart::ProcessSub { dir: ProcDir::In, .. })),
            "the `<(printf hi)` is a process-sub word");
    }

    // --- bad-substitution lexer tests (v233) ---

    /// Extract the first WordPart from a single-word tokenization result.
    fn first_word_part(input: &str) -> WordPart {
        let mut toks = crate::lexer::tokenize(input).expect("lex");
        let word = match toks.remove(0) {
            Token::Word(w) => w,
            other => panic!("expected Word, got {other:?}"),
        };
        word.0.into_iter().next().expect("non-empty word")
    }

    #[test]
    fn bad_subst_dollar_name_defers() {
        // `${$x}` has a `$` as name — lexable but invalid. Must parse OK and
        // produce a BadSubst node with the raw `${$x}` text.
        let part = first_word_part("${$x}");
        assert!(matches!(&part,
            WordPart::ParamExpansion { modifier: ParamModifier::BadSubst { raw }, .. }
            if raw == "${$x}"
        ), "got {:?}", part);
    }

    #[test]
    fn bad_subst_empty_transform_op_defers() {
        // `${V@}` — `@` with no op letter — bad substitution but must parse.
        assert!(crate::lexer::tokenize("${V@}").is_ok(), "should lex without error");
    }

    #[test]
    fn bad_subst_dash_digit_defers() {
        // `${-3}` and `${-3:-x}` — digit after special name `-` — must parse.
        assert!(crate::lexer::tokenize("${-3}").is_ok(), "should lex ${{-3}}");
        assert!(crate::lexer::tokenize("${-3:-x}").is_ok(), "should lex ${{-3:-x}}");
    }

    #[test]
    fn bad_subst_star_modifier_defers() {
        // `${H*}` — `*` is not a valid modifier char — must parse.
        assert!(crate::lexer::tokenize("${H*}").is_ok(), "should lex ${{H*}}");
    }

    #[test]
    fn unterminated_brace_still_errors() {
        assert_eq!(
            crate::lexer::tokenize("${x").unwrap_err(),
            LexError::UnterminatedBrace
        );
    }
}

