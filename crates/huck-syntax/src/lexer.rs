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
    column: u32,            // NEW: 1-based character column
    peeked: Option<char>,
    peeked_len: usize,
}

impl<'a> CharCursor<'a> {
    pub fn new(s: &'a str) -> Self {
        CharCursor { s, pos: 0, line: 1, column: 1, peeked: None, peeked_len: 0 }
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

    /// 1-based character column of the next char to be produced.
    pub fn column(&self) -> u32 { self.column }

    /// Byte slice of the source from `start` to the current offset. Used to
    /// reconstruct the raw `${…}` text for a deferred bad-substitution.
    pub fn slice_from(&self, start: usize) -> &str {
        &self.s[start..self.pos]
    }

    /// Reposition the cursor to a byte offset with explicit line/column, clearing
    /// any pending 1-char peek. Used by `Lexer::rewind` to re-lex from a checkpoint.
    pub fn seek(&mut self, offset: usize, line: u32, column: u32) {
        self.pos = offset;
        self.line = line;
        self.column = column;
        self.peeked = None;
        self.peeked_len = 0;
    }

    /// Peek at the nth character (0-indexed) from the current position WITHOUT
    /// consuming anything.  `n=0` is equivalent to `peek()` (but returns by value).
    /// Used by `Mode::Backtick` to look past a run of backslashes before a backtick.
    ///
    /// Bounded: scans at most `n+1` chars forward; does NOT advance `pos` or modify
    /// `peeked`.  Never panics — returns `None` when fewer than `n+1` chars remain.
    pub fn peek_nth(&self, n: usize) -> Option<char> {
        // `self.pos` always points at the raw byte offset of the next character
        // (the `peeked` buffer, if any, starts at `self.pos`).
        let mut it = self.s[self.pos..].chars();
        for _ in 0..n {
            it.next()?;
        }
        it.next()
    }
}

impl Iterator for CharCursor<'_> {
    type Item = char;
    fn next(&mut self) -> Option<char> {
        if let Some(c) = self.peeked.take() {
            self.pos += self.peeked_len;
            self.peeked_len = 0;
            if c == '\n' { self.line += 1; self.column = 1; } else { self.column += 1; }
            Some(c)
        } else if let Some(c) = self.s[self.pos..].chars().next() {
            self.pos += c.len_utf8();
            if c == '\n' { self.line += 1; self.column = 1; } else { self.column += 1; }
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

#[allow(dead_code)] // Phase C atoms; dormant in v241
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SubstKind { First, All, Prefix, Suffix }   // / , // , /# , /%

#[allow(dead_code)] // Phase C atoms; dormant in v241
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ParamOpKind {
    UseDefault(bool), AssignDefault(bool), ErrorIfUnset(bool), UseAlternate(bool), // bool = colon-prefixed
    RemovePrefix(bool), RemoveSuffix(bool),   // bool = longest (## / %%)
    Substitute(SubstKind),
    Case(CaseDirection, bool),                // bool = all (^^ / ,,)
    Transform(TransformOp),
    Substring,
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

/// A token's source location. `column` is a 1-based character column
/// (Unicode scalars from the line start; a tab is one column).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub offset: usize,
    pub line: u32,
    pub column: u32,
}

impl Span {
    /// A span at byte `offset`, 1-based source `line`, and 1-based character
    /// `column`. Built at lex time from the `CharCursor` position of a token's
    /// first character, so location travels with the token (no sidecar arrays).
    pub fn new(offset: usize, line: u32, column: u32) -> Span { Span { offset, line, column } }
    /// Placeholder span for synthesized tokens / test fixtures (line 0 = unknown).
    pub fn unknown() -> Span { Span { offset: 0, line: 0, column: 0 } }
}

#[derive(Debug, PartialEq, Eq, Clone)]
#[non_exhaustive]
pub enum TokenKind {
    Word(Word),
    Op(Operator),
    Newline,
    /// A complete here-doc with its body already collected. The lexer
    /// builds this in two phases: the `<<DELIM` opener is seen on one
    /// line, the body lines are consumed after the line's `\n`. The
    /// resulting TokenKind::Heredoc occupies the position where `<<DELIM`
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
    // --- Phase C parser-driven atoms (dormant in v241; emitted only under the
    // ParamExpansion/operand modes, consumed only by parser.rs). ---
    ParamOpen { quoted: bool }, ParamClose, LBracket, RBracket, ParamSep,
    ParamName(String),
    /// v264: a braced parameter NAME assembled with `extquote` `$'…'` decoding
    /// (`${a$'b'}` → `ab`). Distinct from `ParamName` so the parser can apply
    /// the M-156 dquote gate's Var→ParamExpansion{None} promotion (mirrors the
    /// oracle's `name_decoded` promotion in `scan_braced_param_expansion`).
    ParamNameDecoded(String),
    /// v264: a deferred bad-substitution detected by the head scanner (a `$"…"`
    /// or bare-quote name, a decoded name in the wrong quote context, an invalid
    /// decoded identifier, or an unrecognised post-name modifier char). Carries
    /// the full `${…}` verbatim source so the parser can build
    /// `ParamModifier::BadSubst { raw }` (mirrors the oracle's `recover_bad_subst`).
    ParamBadSubst { raw: String },
    /// v264: the `*}` / `@}` tail of a `${!prefix*}` / `${!prefix@}` prefix-name
    /// expansion (both the sigil and the closing `}` are consumed). `at` is true
    /// for the `@` form. Mirrors the oracle's `ParamModifier::PrefixNames { at }`.
    ParamPrefixClose { at: bool },
    ParamLengthPrefix, ParamIndirect,
    #[allow(private_interfaces)] // ParamOpKind is pub(crate); TokenKind is pub — dormant in v241
    ParamOp(ParamOpKind),
    Lit { text: String, quoted: bool },
    DollarName { name: String, quoted: bool },
    DeferredExpansion,   // $(...) inside a nested "..." operand span (both the continuing-dquote site and the first-char-of-a-newly-opened-dquote site) — still deferred: unquoted-operand $(cmd) handled by CmdSubOpen in v244; unquoted+continuing-dquote-operand backtick handled by BeginBacktick in v245 T6; unquoted-operand $(( handled by ArithOpen in v246 T6 (the in-dquote sites for $(cmd)/$(( stay deferred — see the ArithOpen wiring note at the continuing-dquote `$(` site)
    CmdSubOpen,          // $( opener atom — dual role: signal in an operand mode (v244 wiring), real opener in CommandSub mode
    ProcSubOpen { dir: ProcDir },  // v251: `<(`/`>(` word-part signal (unquoted); parser assembles WordPart::ProcessSub via Mode::CommandSub. Cursor is left on `(`.
    ArrayOpen,   // v252: zero-width signal that a compound array RHS `(…)` follows an assignment prefix; cursor left on `(`. Parser pushes Mode::ArrayLiteral.
    ArrayClose,  // v252: the `)` closing an array literal, emitted by Mode::ArrayLiteral.
    // --- Phase C v245: backtick command-substitution atoms (dormant until Task 2). ---
    BeginBacktick,       // opening ` — dual role: signal in an operand mode (v245 T6 wiring), real opener in Backtick mode
    EndBacktick,         // closing ` — emitted by scan_step_backtick when depth unwinds
    // --- Phase C v246: arithmetic-expansion atoms (dormant). ---
    ArithOpen,   // opening `$((` — dual role: zero-width signal in an operand mode, real opener in Arith mode
    ArithClose,  // closing `))` — emitted by scan_step_arith at paren_depth 0
    ArithBail,   // a `)` at paren_depth 0 NOT followed by `)` — parser rewinds and retries as `$( (…) )`
    ArithSemi,   // v256: a `;` at paren_depth 0 inside a for-header — section separator
    LegacyArithOpen,  // v258: opening `$[` of a legacy `$[ … ]` arith expansion — dual role like ArithOpen (zero-width operand signal + real opener in Arith{delim:Bracket})
    // --- Phase C v247: atom-emitting Command-mode scaffolding (dormant). ---
    Blank,   // v247: a run of unquoted inter-word whitespace in the atom-command stream (word boundary)
    /// v247 T2: ONE complete quoted run in command-word context — `'…'`, `"…"`
    /// (T2 scope: literal-only body, no embedded expansions), a single
    /// backslash escape `\c`, or a `$'…'` ANSI-C run (already escape-decoded).
    /// The parser wraps this in `WordPart::Quoted { style, parts: vec![Literal
    /// { text, quoted: true }] }` — mirrors the oracle's `scan_step_command`
    /// quoting, which always wraps a quote run (never leaves it as a bare
    /// `Literal`). Kept separate from `Lit` (which is for UNQUOTED literal runs,
    /// glued Word assembly) so the oracle's `QuoteStyle` survives atom-ization.
    QuoteRun { style: QuoteStyle, text: String },
    /// v247: a literal `$` that is not an expansion opener; a standalone Literal
    /// that must NOT coalesce with neighbors — mirrors the oracle flushing its
    /// buffer and pushing `$` alone.
    DollarLit { quoted: bool },
    // --- Phase C v247 T3: command-position expansions + parser-driven double quotes. ---
    /// v247 T3: a command-position tilde construct (`~`, `~user`, `~+`, `~-`,
    /// `~/…`). Emitted by `scan_command_word_atom` ONLY at word start (mirrors
    /// the oracle's `!has_token` guard); the parser turns it into
    /// `WordPart::Tilde(spec)`.
    Tilde(TildeSpec),
    /// v247 T3: zero-width opener signal for a `"…"` double-quoted span in
    /// command-word context. The lexer does NOT consume the `"`; the parser
    /// (`parse_dquote`) pushes `Mode::DoubleQuote`, whose first scan consumes the
    /// opening `"`. Mirrors the `CmdSubOpen`/`ArithOpen` zero-width signal
    /// pattern so a `"…"` body containing `$(…)`/`` `…` ``/`$((…))` is parsed
    /// recursively (the parser owns delimiter-matching), never flat-scanned.
    BeginDquote,
    /// v247 T3: closing `"` of a `Mode::DoubleQuote` span — emitted by
    /// `scan_step_dquote` when the closing quote is reached; consumed by
    /// `parse_dquote`, which then pops the mode.
    EndDquote,
    /// v250: opens one heredoc body's part atoms (atom path); parser assembles
    /// until `HeredocBodyEnd`. `expand` mirrors the heredoc's `expand` flag
    /// (unquoted/interpolated delimiter): the parser needs it to pick the LITERAL
    /// batch-split assembly (`expand:false`, one `Lit{quoted:true}` spanning the
    /// whole body) vs the EXPANDING per-atom assembly (`expand:true`, a stream of
    /// literal runs + expansion parts + `"\n"` separators), which produce DIFFERENT
    /// part lists for the SAME token bytes (e.g. a single blank line: literal keeps
    /// an empty `Literal`, expanding drops it).
    HeredocBodyBegin { expand: bool },
    /// v250: closes one heredoc body's part atoms.
    HeredocBodyEnd,
    /// v247 T4: an assignment-prefix atom for the STRUCTURED assignment lvalues
    /// `name+=` / `name[sub]=` / `name[sub]+=`. Emitted by `scan_command_word_atom`
    /// at word start when an identifier prefix is followed by `+=`, or by a
    /// bracketed subscript immediately followed by `=`/`+=`. The parser turns it
    /// into the leading `WordPart::AssignPrefix { target, append }` of the word,
    /// which `try_split_assignment` consumes. Plain `name=value` gets NO
    /// AssignPrefix — the `name=` flows into the literal run and the splitter
    /// breaks on the first unquoted `=` (mirrors the oracle's `scan_step_command`).
    AssignPrefix { target: crate::command::AssignTarget, append: bool },
    /// v254: zero-width terminator of a `Mode::Regex` pattern operand — emitted
    /// (and the mode popped) at the depth-0 whitespace or EOF that ends the `=~`
    /// operand. The parser's `parse_regex_operand` assembles pattern atoms until
    /// this, then consumes it. Cursor is left ON the terminating whitespace (so
    /// command mode re-consumes it as a `Blank`/`Newline`); at EOF the cursor is
    /// at end.
    RegexEnd,
    /// v264: zero-width signal that a command-word atom is one of `? * + @ !`
    /// directly followed by `(` (an extglob group opener, gated by
    /// `LexerOptions::extglob`). Mirrors the `CmdSubOpen`/`ArithOpen` pattern:
    /// the lexer does NOT consume the prefix char or `(` — the parser's
    /// `parse_extglob_group` pushes `Mode::Extglob`, whose first scan consumes
    /// both and emits the opening `Lit`. `prefix` is carried only for
    /// documentation/debugging; the mode's own first-entry scan re-reads it
    /// from the cursor.
    ExtglobOpen { prefix: char },
    /// v264: zero-width terminator of a `Mode::Extglob` group, emitted (and the
    /// mode popped) by the lexer itself the moment the group's own `(` is
    /// balanced (`paren_depth` returns to 0 after a `)`). Consumed by
    /// `parse_extglob_group`. Unlike `RegexEnd`, this is emitted in the SAME
    /// `scan_step_extglob` call that produced the closing `)`'s `Lit` atom —
    /// scan_step is never invoked again for a popped mode frame, so there is no
    /// separate "already closed" state to track between calls.
    ExtglobEnd,
}

/// A token paired with its source location. Equality and hashing are by `kind`
/// only — `span` is positional metadata, NOT part of token identity. This keeps
/// equality-based lexer tests valid (they ignore position) and is the deliberate
/// design choice for v237.
#[derive(Debug, Clone)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

impl Token {
    pub fn new(kind: TokenKind, span: Span) -> Token { Token { kind, span } }
    pub fn kind(&self) -> &TokenKind { &self.kind }
}

impl From<TokenKind> for Token {
    /// Wrap a kind with a placeholder span (line 0 = unknown). Used by test
    /// fixtures and synthesized tokens; real lexer output carries true spans.
    fn from(kind: TokenKind) -> Token { Token { kind, span: Span::unknown() } }
}

impl PartialEq for Token {
    fn eq(&self, other: &Self) -> bool { self.kind == other.kind }
}
impl Eq for Token {}

#[cfg(test)]
impl PartialEq<TokenKind> for Token {
    fn eq(&self, other: &TokenKind) -> bool { &self.kind == other }
}
#[cfg(test)]
impl PartialEq<Token> for TokenKind {
    fn eq(&self, other: &Token) -> bool { self == &other.kind }
}

/// State for a heredoc whose body hasn't been collected yet.
#[derive(Clone)]
struct PendingHeredoc {
    delim: String,
    expand: bool,
    strip_tabs: bool,
    /// Index into `tokens` of the `TokenKind::Heredoc` placeholder to patch.
    token_idx: usize,
    /// v264 (atom path only): the mode-stack depth (`self.modes.len()`) at which
    /// the `<<` opener was registered. A heredoc introduced inside a cmdsub/
    /// backtick body registers at depth ≥ 2 and its body must be emitted at that
    /// body's own newline — BEFORE a shallower (outer-line) heredoc that was
    /// registered EARLIER in source but belongs to the enclosing line. Selection
    /// by matching depth (not FIFO front) keeps the two independent. Unused by
    /// the oracle `pending_heredocs` queue.
    reg_depth: usize,
}

/// Lexer feature toggles resolved from shell state at tokenize time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LexerOptions {
    pub extglob: bool,
    /// True when the `${…}` currently being scanned is inside double quotes.
    /// Read ONLY by the extquote `$'…'`-name gate (M-156); it does NOT affect
    /// glob-literalness, word-splitting, or quoting of operands.
    pub in_dquote: bool,
}

impl LexerOptions {
    /// Returns a copy with `in_dquote` set — used to seed the extquote
    /// double-quote context for a pattern-operand re-parse.
    fn with_in_dquote(self, b: bool) -> Self {
        LexerOptions { in_dquote: b, ..self }
    }
}

/// Raw output of the partial tokenizer: the spanned `tokens` (each carrying its
/// own start `Span`) and an optional trailing lex error + the byte offset where
/// lexing failed.
type PartialTokens = (Vec<Token>, Option<(LexError, usize)>);

/// Back-compat entry: lexes with all options off (current behavior).
pub fn tokenize(input: &str) -> Result<Vec<Token>, LexError> {
    tokenize_with_opts(input, LexerOptions::default())
}

pub fn tokenize_with_opts(input: &str, opts: LexerOptions) -> Result<Vec<Token>, LexError> {
    match tokenize_partial(input, opts) {
        (tokens, None) => Ok(tokens),
        (_, Some((e, _off))) => Err(e),
    }
}

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
fn fd_prefix_of(token: Option<&TokenKind>) -> Option<crate::command::RedirFd> {
    let Some(TokenKind::Word(w)) = token else { return None };
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

/// v247 T5: fd-prefix classification for the atom command scanner. Mirrors the
/// oracle's `take_fd_prefix` look-back (which operates on an already-emitted
/// `TokenKind::Word`), but takes the RAW literal text of an atom word-run —
/// under the atom scanner a fd-prefix like `3`/`{fd}` is a plain `Lit` run, not
/// a `Word`. Reuses `fd_prefix_of` verbatim by wrapping the text in a throwaway
/// single-literal Word, so the digit-run / `{ident}` classification cannot drift.
fn fd_prefix_of_text(text: &str) -> Option<crate::command::RedirFd> {
    fd_prefix_of(Some(&TokenKind::Word(Word(vec![WordPart::Literal {
        text: text.to_string(),
        quoted: false,
    }]))))
}

/// One scan_step outcome: `Produced` = made progress (more input remains,
/// call again), `Eof` = end of input reached.
enum Step {
    Produced,
    Eof,
}

/// v258: which bracket delimits an arith body. `Paren` = `$(( … ))` / `(( … ))`
/// (paren-depth, closes on `))`); `Bracket` = `$[ … ]` legacy arith (bracket-depth,
/// closes on a single `]`, parens are literal body chars).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArithDelim { Paren, Bracket }

/// The lexing-rule context the lexer scans under. v240 implements only
/// `Command`; the other variants are forward declarations for later Phase C
/// iterations and are never the active mode in production yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // variants used in Phase C iterations; dormant in v240
pub(crate) enum Mode {
    Command,        // default: today's scan_step body (the ONLY mode implemented in v240)
    Subshell,       // ( … )
    CommandSub { body_started: bool },  // $( … ) / `…`
    Backtick { depth: u32 },           // `…` — v245; depth tracks nested `` `\`…\`` `` escaping
    // `seen_name` — set once the NAME atom (or length/indirect prefix's name)
    // has been emitted. `indirect` — set when the `${!…}` indirect prefix was
    // emitted (needed by Phase 2 to route `*}`/`@}` to the prefix-name form).
    // `start_off` — byte offset of the leading `$` of `${` (set by Phase 0's
    // write-back), used to reconstruct a bad-substitution's verbatim `${…}` raw.
    ParamExpansion { seen_name: bool, indirect: bool, start_off: usize }, // ${ … }
    // `enclosing_dquote` — the OUTER `"…"` context the `${…}` itself sits in
    // (seeded once at push time from the parser's `quoted` flag), distinct from
    // `in_dquote` which is the operand scanner's INTERNAL `'…'`/`"…"`-span state
    // that toggles as the operand text is scanned. See v264 operand-dquote fix.
    ParamWordOperand            { in_dquote: bool, enclosing_dquote: bool },
    ParamSubstPatternOperand    { in_dquote: bool, enclosing_dquote: bool },
    ParamSubstringOffsetOperand { in_dquote: bool, enclosing_dquote: bool },
    ParamSubscriptOperand       { in_dquote: bool, enclosing_dquote: bool },
    // v261 T1: `in_squote`/`in_dquote` are the scanner's INTERNAL quote-span state
    // (repurposed from a formerly-dead `in_dquote` field) — NOT the outer
    // quoted-context of the enclosing word (arith bodies are always emitted as
    // `quoted: true`, matching the oracle's `arith_string_to_word`, so there is no
    // outer quoted/unquoted branch to select). Both flags start `false` at every
    // push site (a fresh body opens outside any quote span) and are written back
    // to this frame by `sync_quotes!` on every `'`/`"` toggle in `scan_step_arith`.
    Arith { paren_depth: u32, in_squote: bool, in_dquote: bool, body_started: bool, for_header: bool, delim: ArithDelim }, // $(( … )) / (( … )) / for (( … )) / $[ … ]
    DoubleQuote { body_started: bool }, // "…" — v247 T3 (parser-driven word-level mode)
    ArrayLiteral { body_started: bool, expect_subscript_eq: bool, at_element_start: bool },   // a=( … ) — v252; expect_subscript_eq: control returned from a `[expr]` subscript scan, the required `=` must follow. at_element_start: PERSISTENT across scan_step calls — true only after `(`/a separator and before any value/subscript atom of the current element (so a `[` mid-value stays literal).
    DoubleBracket,  // [[ … ]]
    Regex { paren_depth: u32, body_started: bool },  // v254: RHS of =~ inside [[ … ]]
    /// v264: an extglob group `<prefix>( … )` (`?(...)`/`*(...)`/`+(...)`/
    /// `@(...)`/`!(...)`, gated by `LexerOptions::extglob`). `paren_depth == 0`
    /// on push means "not yet entered" — the first scan consumes the prefix
    /// char + `(` and sets it to 1; the group closes (and this mode is popped)
    /// when a `)` brings `paren_depth` back to 0.
    Extglob { paren_depth: u32 },
    HeredocBody,    // <<EOF …
}

/// A checkpoint of the lexer's scanning state. `rewind` restores it and re-lexes
/// from `resume`. Taken only at a pull boundary (no word mid-accumulation), so
/// the word-accumulation buffers need not be captured.
///
/// NOTE: `mark`/`rewind` must not span heredoc-body collection — `pending_heredocs`
/// is intentionally not captured; that interaction is designed when heredocs enter
/// the mode stack.
#[derive(Debug, Clone)]
pub(crate) struct Mark {
    pos: usize,                 // self.pos (pull index) at mark time
    resume: (usize, u32, u32),  // (offset, line, column) to resume scanning from
    brace_expand: bool,
    has_token: bool,
    in_assignment_value: bool,
    dbracket_depth: u32,
    expect_regex: bool,
    opts: LexerOptions,
    alias_trailing_eligible: bool,
    modes: Vec<Mode>,
    retokenize_arith_as_cmdsub: bool,
    cmd_at_word_start: bool,
    assign_val_tilde_ok: bool,
    /// v250 T6: see `Lexer::heredoc_gen`. Captured so `rewind` can assert the
    /// mark did not span an atom-path heredoc-state change.
    heredoc_gen: u64,
}

/// v250: lexer-internal state while emitting a heredoc body as atoms (atom path).
/// `began` tracks whether `HeredocBodyBegin` was already emitted for the FRONT
/// entry of `atom_pending_heredocs`. Self-started at the newline; cleared when
/// the queue drains. The lexer detects the close-delimiter line itself — it
/// never consults the parser.
#[derive(Debug, Clone)]
struct HeredocEmit {
    began: bool,
    /// v250 T4: true at a LOGICAL body-line start (used only by the EXPANDING
    /// per-atom path). At a logical line start the emitter strips `<<-` tabs and
    /// runs the close-delimiter check before emitting body atoms; a `\<NL>` line
    /// continuation joins physical lines WITHOUT resetting this to true.
    at_line_start: bool,
    /// v264: the mode-stack depth (`self.modes.len()`) at the newline that
    /// triggered this emission. A heredoc registered INSIDE a cmdsub/backtick
    /// body triggers at depth ≥ 2, and its body must be emitted while that body
    /// mode is on the stack — `scan_step_command_body` diverts to
    /// `scan_step_heredoc_body` only when the CURRENT depth matches this, so an
    /// ENCLOSING heredoc (whose expanding body merely CONTAINS the cmdsub, and
    /// was triggered at a shallower depth) does NOT hijack the nested body scan.
    trigger_depth: usize,
}

/// Incremental tokenizer state (v238 Phase A). Holds what were
/// `tokenize_partial_inner`'s locals so the scan logic can be reused; the public
/// `tokenize*` APIs still drain it into a `Vec<Token>`. Phase A.T1 keeps the loop
/// intact (batch internally); T2 splits it into a pull `next_token`.
pub struct Lexer<'a> {
    cursor: CharCursor<'a>,
    opts: LexerOptions,
    brace_expand: bool,
    /// Tokens produced so far (was the local `tokens` vec); `pos` is the
    /// index of the next token next_token() will hand out (pull + future rewind).
    history: Vec<Token>,
    pos: usize,
    /// True for a from_tokens() replay lexer: history is pre-filled, never scans.
    replay: bool,
    parts: Vec<WordPart>,
    current: String,
    has_token: bool,
    token_start: usize,
    token_start_line: u32,
    token_start_col: u32,
    in_assignment_value: bool,
    dbracket_depth: u32,
    expect_regex: bool,
    pending_heredocs: std::collections::VecDeque<PendingHeredoc>,
    /// v250: atom-path heredoc queue — SEPARATE from `pending_heredocs` so the
    /// production `fill_to`/`backfill_pending_at` never gate the atom opener.
    atom_pending_heredocs: std::collections::VecDeque<PendingHeredoc>,
    /// v250: Some while emitting heredoc body atoms after a line's newline.
    emitting_heredoc: Option<HeredocEmit>,
    /// v250 T3: heredoc body `Word`s the atom-command PARSER has assembled so
    /// far, in source order. `skip_newlines` (the atom path's single
    /// newline-consumption choke point) drains each `HeredocBodyBegin`…`End`
    /// group it encounters into here; `parse_sequence` takes the whole vec via
    /// `take_heredoc_bodies` once the top-level sequence is fully parsed and
    /// zips it (source order == emission order) into the still-empty
    /// `RedirOp::Heredoc { body }` placeholders via `attach_heredoc_bodies`.
    /// Lexer-owned (rather than threaded through the ~24 `skip_newlines`
    /// call-sites as a parameter) so no caller signature changes.
    parsed_heredoc_bodies: Vec<Word>,
    aliases: std::collections::HashMap<String, String>,
    active: std::collections::HashSet<String>,
    /// Carries bash's trailing-blank rule across one expansion: a body ending in
    /// whitespace makes the NEXT word command-position eligible.
    alias_trailing_eligible: bool,
    /// Parser-controlled lexing-mode stack (Phase C). Never empty; `Command` is
    /// the floor. Dormant in v240 — only `Command` is pushed in production.
    /// Each `ParamExpansion` frame carries its own `seen_name` phase flag so
    /// nested `${…}` expansions and `mark`/`rewind` are both stack-safe.
    modes: Vec<Mode>,
    /// One-shot v246 flag: when set, the CommandSub scanner treats a `$((` opener
    /// as `$(` + a subshell `(` (the `$( (…) )` wrinkle) instead of deferring it
    /// as arithmetic. Set by `parse_arith_expansion` on an `ArithBail` rewind,
    /// cleared the moment `scan_step_command_sub` consumes the `$(`. Captured in
    /// `Mark`/restored by `rewind` so a rewind that spans setting it stays
    /// consistent.
    retokenize_arith_as_cmdsub: bool,
    /// v247: when true, `Mode::Command` scans via `scan_step_command_atoms`
    /// (emits word-atoms + `Blank` + structural tokens) instead of the
    /// Word-emitting `scan_step_command`. Default false (production). Set only by
    /// the dormant atom path (differential harness + the eventual live flip).
    command_atoms: bool,
    /// v247 T3: true when the next command-word atom begins a fresh word (i.e.
    /// the previous atom was a `Blank`/start-of-input). Mirrors the oracle's
    /// `!has_token` guard: a `~` is tilde-special only at word start; mid-word
    /// (`a~b`, `$x~`) it is literal. Reset to true after a `Blank`, cleared
    /// after any word-content atom. Only meaningful under `command_atoms`.
    cmd_at_word_start: bool,
    /// v247 T4: true when a `~` scanned next would be a tilde CONSTRUCT because
    /// we are inside an assignment value AND positioned right after a `=`/`:`
    /// (mirrors the oracle's `tilde_eligible_in_assignment`, which checks that
    /// the accumulated literal buffer ends in `=` or `:`). Set after the plain
    /// `name=` prefix and after an unquoted `=`/`:` in the value literal run;
    /// cleared whenever a non-literal part is emitted (which flushes the
    /// oracle's buffer). Only meaningful under `command_atoms`.
    assign_val_tilde_ok: bool,
    /// v250 T6 (+ fix pass): monotonic counter bumped on EVERY mutation of
    /// `atom_pending_heredocs`/`emitting_heredoc`: the push at the atom `<<`
    /// opener site, `emitting_heredoc` set at the newline trigger, the
    /// `HeredocEmit.began` false→true flip (first body atom of an entry), each
    /// `at_line_start` set/clear in `scan_step_heredoc_body_expanding`, and the
    /// pop_front/re-arm in `emit_heredoc_body_end`. Captured in `Mark` and
    /// checked in `rewind` (`debug_assert_eq!`) so a mark/rewind that spans ANY
    /// heredoc-state change is caught loudly (debug builds) instead of
    /// silently desyncing `atom_pending_heredocs`/`emitting_heredoc` (`rewind`
    /// does not restore either field). The only live mark/rewind pair reachable
    /// on the atom command path is the arith `$((`-bail rewind in
    /// `parse_arith_expansion` — funcdef detection uses seed-not-rewind (v248),
    /// not mark/rewind. See `atoms_heredoc_marks_dont_span_bodies` for a case
    /// that drives that rewind while a heredoc body is actively emitting.
    heredoc_gen: u64,
}

impl<'a> Lexer<'a> {
    fn new(input: &'a str, opts: LexerOptions, brace_expand: bool) -> Self {
        Lexer {
            cursor: CharCursor::new(input),
            opts,
            brace_expand,
            history: Vec::new(),
            pos: 0,
            replay: false,
            parts: Vec::new(),
            current: String::new(),
            has_token: false,
            token_start: 0,
            token_start_line: 1,
            token_start_col: 1,
            in_assignment_value: false,
            dbracket_depth: 0,
            expect_regex: false,
            pending_heredocs: std::collections::VecDeque::new(),
            atom_pending_heredocs: std::collections::VecDeque::new(),
            emitting_heredoc: None,
            parsed_heredoc_bodies: Vec::new(),
            aliases: std::collections::HashMap::new(),
            active: std::collections::HashSet::new(),
            alias_trailing_eligible: false,
            modes: vec![Mode::Command],
            retokenize_arith_as_cmdsub: false,
            command_atoms: false,
            cmd_at_word_start: true,
            assign_val_tilde_ok: false,
            heredoc_gen: 0,
        }
    }

    pub(crate) fn current_mode(&self) -> Mode {
        *self.modes.last().expect("mode stack is never empty (Command is the floor)")
    }

    /// v250 T3: record one assembled heredoc body `Word` (parser-owned
    /// assembly of a `HeredocBodyBegin`…`End` atom group) in source order.
    pub(crate) fn push_heredoc_body(&mut self, w: Word) {
        self.parsed_heredoc_bodies.push(w);
    }

    /// v250 T3: drain all heredoc body `Word`s collected so far, in source
    /// order. Called once by `parse_sequence` after the top-level sequence is
    /// fully parsed, to feed `attach_heredoc_bodies`.
    pub(crate) fn take_heredoc_bodies(&mut self) -> Vec<Word> {
        std::mem::take(&mut self.parsed_heredoc_bodies)
    }

    /// v264: set the M-156 extquote double-quote context flag (`opts.in_dquote`),
    /// returning the previous value so the caller can restore it. The braced-param
    /// head scanner reads this flag to decide whether a `$'…'`-decoded NAME is
    /// valid (bash: only inside double quotes). `parse_param_expansion` folds its
    /// `quoted` argument into this flag, mirroring the oracle's `quoted ||
    /// opts.in_dquote` gate.
    pub(crate) fn set_in_dquote(&mut self, v: bool) -> bool {
        let old = self.opts.in_dquote;
        self.opts.in_dquote = v;
        old
    }

    /// v264: read the current M-156 extquote double-quote context flag.
    pub(crate) fn in_dquote(&self) -> bool {
        self.opts.in_dquote
    }

    /// v264: record the `${`'s `$` byte offset in the current ParamExpansion frame,
    /// computed from the live cursor (which sits just past `${`, 2 ASCII bytes, once
    /// the ENCLOSING scanner has consumed the opener — in production the head
    /// scanner's Phase 0 never runs). Used so a deferred bad-substitution can
    /// reconstruct the verbatim `${…}` raw.
    pub(crate) fn set_param_start_off_from_cursor(&mut self) {
        let off = self.cursor.offset().saturating_sub(2);
        if let Some(Mode::ParamExpansion { start_off, .. }) = self.modes.last_mut() {
            *start_off = off;
        }
    }

    #[allow(dead_code)] // called by parser in Phase C iterations; dormant in v240
    pub(crate) fn push_mode(&mut self, m: Mode) {
        self.modes.push(m);
    }

    #[allow(dead_code)] // called by parser in Phase C iterations; dormant in v240
    pub(crate) fn pop_mode(&mut self) -> Mode {
        // Guard BEFORE popping so the floor is protected even in release builds:
        // popping the last element would leave `modes` empty and make the next
        // `current_mode()` panic with a confusing message.
        debug_assert!(self.modes.len() > 1, "Command is the floor and must never be popped");
        self.modes.pop().expect("pop_mode on an empty mode stack")
    }

    /// v254 T1 fix: set the current `Mode::Regex { body_started }` flag from the
    /// parser (mirrors the `paren_depth` write-back in `scan_step_regex`). The
    /// parser calls this after each assembled pattern atom with "has any content
    /// been produced," so `body_started` tracks the oracle's
    /// `!(lit.is_empty() && parts.is_empty())` — an empty `""` leaves it false. A
    /// no-op if the top of the mode stack is not `Mode::Regex` (defensive).
    pub(crate) fn set_regex_body_started(&mut self, v: bool) {
        if let Some(Mode::Regex { body_started, .. }) = self.modes.last_mut() {
            *body_started = v;
        }
    }

    /// Arm the one-shot v246 wrinkle flag: the next `$((` the CommandSub scanner
    /// sees is tokenized as `$(` + a subshell `(` rather than deferred as arith.
    /// Cleared the moment `scan_step_command_sub` consumes that `$(`.
    #[allow(dead_code)] // called by parser in Phase C iterations; dormant in v240
    pub(crate) fn set_retokenize_arith_as_cmdsub(&mut self) {
        self.retokenize_arith_as_cmdsub = true;
    }

    /// Checkpoint the scanning state for a later `rewind`. Must be called at a
    /// pull boundary (no partial word). The resume point is the span of the
    /// next-to-hand-out token when lookahead is buffered, else the live cursor.
    ///
    /// NOTE: `mark`/`rewind` must not span heredoc-body collection —
    /// `pending_heredocs` is intentionally not captured; that interaction is
    /// designed when heredocs enter the mode stack.
    #[allow(dead_code)] // dormant until parser calls it in Phase C iterations
    pub(crate) fn mark(&self) -> Mark {
        debug_assert!(
            self.current.is_empty() && self.parts.is_empty() && !self.has_token,
            "mark() must be taken at a pull boundary (no word mid-accumulation)"
        );
        let resume = if self.pos < self.history.len() {
            let s = self.history[self.pos].span;
            (s.offset, s.line, s.column)
        } else {
            (self.cursor.offset(), self.cursor.line(), self.cursor.column())
        };
        Mark {
            pos: self.pos,
            resume,
            brace_expand: self.brace_expand,
            has_token: self.has_token,
            in_assignment_value: self.in_assignment_value,
            dbracket_depth: self.dbracket_depth,
            expect_regex: self.expect_regex,
            opts: self.opts,
            alias_trailing_eligible: self.alias_trailing_eligible,
            modes: self.modes.clone(),
            retokenize_arith_as_cmdsub: self.retokenize_arith_as_cmdsub,
            cmd_at_word_start: self.cmd_at_word_start,
            assign_val_tilde_ok: self.assign_val_tilde_ok,
            heredoc_gen: self.heredoc_gen,
        }
    }

    /// Restore a `Mark`: discard buffered/produced tokens at/after it, seek the
    /// cursor back, and restore flags + mode stack. The next pull re-lexes from
    /// the checkpoint under the now-current mode. A replay (`from_tokens`) lexer
    /// never scans, so history is left intact and only `pos`/flags are reset.
    #[allow(dead_code)] // dormant until parser calls it in Phase C iterations
    pub(crate) fn rewind(&mut self, m: &Mark) {
        debug_assert!(m.pos <= self.history.len(), "rewind target beyond history");
        // Like `mark`, `rewind` is only valid at a pull boundary: it does not
        // clear the word-accumulation buffers, so a mid-word rewind would leak
        // partial state into the next token. (Both points are clean today.)
        debug_assert!(
            self.current.is_empty() && self.parts.is_empty() && !self.has_token,
            "rewind() must be called at a pull boundary (no word mid-accumulation)"
        );
        if !self.replay {
            self.history.truncate(m.pos);
            self.cursor.seek(m.resume.0, m.resume.1, m.resume.2);
        }
        self.pos = m.pos;
        self.brace_expand = m.brace_expand;
        self.has_token = m.has_token;
        self.in_assignment_value = m.in_assignment_value;
        self.dbracket_depth = m.dbracket_depth;
        self.expect_regex = m.expect_regex;
        self.opts = m.opts;
        self.alias_trailing_eligible = m.alias_trailing_eligible;
        self.modes = m.modes.clone();
        self.retokenize_arith_as_cmdsub = m.retokenize_arith_as_cmdsub;
        self.cmd_at_word_start = m.cmd_at_word_start;
        self.assign_val_tilde_ok = m.assign_val_tilde_ok;
        debug_assert_eq!(
            self.heredoc_gen, m.heredoc_gen,
            "mark/rewind must not span heredoc-body emission (v250)"
        );
    }

    /// Scan one step under the current mode. v241 T2 implements `ParamExpansion`;
    /// v241 T3 implements the four operand modes; remaining Phase C modes are
    /// forward declarations (never pushed in production).
    fn scan_step(&mut self) -> Result<Step, LexError> {
        match self.current_mode() {
            Mode::Command if self.command_atoms => self.scan_step_command_atoms(),
            Mode::Command => self.scan_step_command(),
            Mode::ParamExpansion { .. } => self.scan_step_param_head(),
            Mode::ParamWordOperand            { in_dquote, enclosing_dquote } => self.scan_step_param_operand(None,      '}', in_dquote, enclosing_dquote),
            Mode::ParamSubstPatternOperand    { in_dquote, enclosing_dquote } => self.scan_step_param_operand(Some('/'), '}', in_dquote, enclosing_dquote),
            Mode::ParamSubstringOffsetOperand { in_dquote, enclosing_dquote } => self.scan_step_param_operand(Some(':'), '}', in_dquote, enclosing_dquote),
            Mode::ParamSubscriptOperand       { in_dquote, enclosing_dquote } => self.scan_step_param_operand(None,      ']', in_dquote, enclosing_dquote),
            Mode::CommandSub { body_started } => self.scan_step_command_sub(body_started),
            Mode::Backtick { depth } => self.scan_step_backtick(depth),
            Mode::Arith { paren_depth, in_squote, in_dquote, body_started, for_header, delim } =>
                self.scan_step_arith(paren_depth, in_squote, in_dquote, body_started, for_header, delim),
            Mode::DoubleQuote { body_started } => self.scan_step_dquote(body_started),
            Mode::ArrayLiteral { body_started, expect_subscript_eq, at_element_start } =>
                self.scan_step_array_literal(body_started, expect_subscript_eq, at_element_start),
            Mode::Regex { paren_depth, body_started } => self.scan_step_regex(paren_depth, body_started),
            Mode::Extglob { paren_depth } => self.scan_step_extglob(paren_depth),
            other => unreachable!("Mode::{other:?} not implemented until its Phase C iteration"),
        }
    }

    /// Emits one head atom of a `${…}` expansion under `Mode::ParamExpansion`.
    ///
    /// The atom sequence is:
    ///   ParamOpen → [ParamLengthPrefix|ParamIndirect] → ParamName
    ///   → [LBracket (yields; parser pushes subscript mode)] → [ParamOp] → ParamClose
    ///
    /// One atom per call. Per-frame `seen_name` in `Mode::ParamExpansion { seen_name }`
    /// tracks pre-/post-name phase; stored in the mode stack so nested `${…}` and
    /// `mark`/`rewind` are both stack-safe.
    /// Mirrors `scan_braced_param_expansion` (lexer.rs:3284) char-by-char for
    /// operator recognition; emits atoms instead of building WordParts.
    fn scan_step_param_head(&mut self) -> Result<Step, LexError> {
        // ── Phase 0: opener `${` (cursor sits on `$` at mode entry) ──────────────
        if self.cursor.peek() == Some(&'$') {
            let mut probe = self.cursor.clone();
            probe.next(); // skip `$`
            if probe.peek() == Some(&'{') {
                let off = self.cursor.offset();
                let l   = self.cursor.line();
                let c   = self.cursor.column();
                self.cursor.next(); // `$`
                self.cursor.next(); // `{`
                // seen_name is already false on the freshly-pushed frame; no reset
                // needed. Record the `$` offset so a later bad-substitution can
                // reconstruct the verbatim `${…}` raw (mirrors `dollar_start`).
                if let Some(Mode::ParamExpansion { start_off, .. }) = self.modes.last_mut() {
                    *start_off = off;
                }
                self.history.push(Token::new(TokenKind::ParamOpen { quoted: false }, Span::new(off, l, c)));
                return Ok(Step::Produced);
            }
            // Cursor is at `$` but not followed by `{` — shouldn't happen in
            // normal usage (the parser only pushes this mode when it sees `${`).
            // Fall through; EOF path below handles it gracefully.
        }

        // Copy `seen_name` out of the mode frame so we don't hold a &mut borrow
        // across cursor work.
        let seen_name = matches!(self.modes.last(), Some(Mode::ParamExpansion { seen_name: true, .. }));

        // Shared helper: emit a deferred bad-substitution. Consumes the rest of
        // the `${…}` body through the matching `}` (depth/quote/`$'…'`-aware, via
        // `scan_braced_operand`), reconstructs the verbatim `${…}` raw from the
        // recorded `start_off`, marks the frame `seen_name` (so the head mode
        // terminates), and emits `ParamBadSubst { raw }`. Mirrors the oracle's
        // `recover_bad_subst`. Usable from both Phase 1 (name) and Phase 2 (op).
        macro_rules! emit_bad_subst {
            () => {{
                let sp = Span::new(self.cursor.offset(), self.cursor.line(), self.cursor.column());
                let start_off = match self.modes.last() {
                    Some(Mode::ParamExpansion { start_off, .. }) => *start_off,
                    _ => 0,
                };
                let _ = scan_braced_operand(&mut self.cursor)?;
                let raw = self.cursor.slice_from(start_off).to_string();
                if let Some(Mode::ParamExpansion { seen_name, .. }) = self.modes.last_mut() {
                    *seen_name = true;
                }
                self.history.push(Token::new(TokenKind::ParamBadSubst { raw }, sp));
                return Ok(Step::Produced);
            }};
        }

        // ── Phase 1: pre-name (emit prefix and/or ParamName) ─────────────────────
        if !seen_name {
            let off = self.cursor.offset();
            let l   = self.cursor.line();
            let c   = self.cursor.column();

            // Helper macro: mark the current ParamExpansion frame as having seen
            // a name, then push `tok` and return Produced. The `self.modes` write
            // comes AFTER all cursor work is done to avoid borrow conflicts.
            macro_rules! emit_param_name {
                ($tok:expr) => {{
                    if let Some(Mode::ParamExpansion { seen_name, .. }) = self.modes.last_mut() {
                        *seen_name = true;
                    }
                    self.history.push($tok);
                    return Ok(Step::Produced);
                }};
            }

            // Helper macro: assemble a possibly-`$'…'`-decoded name via the shared
            // `scan_braced_name_ext` leaf, apply the M-156 dquote gate + identifier
            // validity check (both resolved in the lexer via `opts.in_dquote`,
            // which `parse_param_expansion` seeds from `quoted`), and emit either
            // `ParamNameDecoded`/`ParamName` or a bad-subst. Mirrors the oracle's
            // `scan_braced_name_ext` handling at lexer.rs:6842.
            macro_rules! emit_ext_name {
                () => {{
                    match scan_braced_name_ext(&mut self.cursor)? {
                        NameScan::BadSubst => emit_bad_subst!(),
                        NameScan::Name { name, decoded } => {
                            if decoded {
                                // A `$'…'`-decoded name is valid ONLY in double-quote
                                // context and must be a valid identifier.
                                if !self.opts.in_dquote || !is_valid_param_name(&name) {
                                    emit_bad_subst!();
                                }
                                emit_param_name!(Token::new(TokenKind::ParamNameDecoded(name), Span::new(off, l, c)));
                            } else if name.is_empty() {
                                // `${}` / `${'x'}` etc. — no name char → bad subst.
                                emit_bad_subst!();
                            } else {
                                emit_param_name!(Token::new(TokenKind::ParamName(name), Span::new(off, l, c)));
                            }
                        }
                    }
                }};
            }

            match self.cursor.peek().copied() {
                // `${#}` = arg-count special param; `${#name}` = length prefix.
                Some('#') => {
                    let mut probe = self.cursor.clone();
                    probe.next(); // skip `#`
                    if probe.peek() == Some(&'}') {
                        // Bare `${#}` — emit the `#` as the ParamName.
                        self.cursor.next();
                        emit_param_name!(Token::new(TokenKind::ParamName("#".into()), Span::new(off, l, c)));
                    } else {
                        // `${#name}` — emit length prefix; name comes next call.
                        self.cursor.next();
                        self.history.push(Token::new(TokenKind::ParamLengthPrefix, Span::new(off, l, c)));
                    }
                    return Ok(Step::Produced);
                }

                // `${!}` = last-bg-pid special param; `${!name}` = indirect.
                Some('!') => {
                    let mut probe = self.cursor.clone();
                    probe.next(); // skip `!`
                    if probe.peek() == Some(&'}') {
                        // Bare `${!}` — emit the `!` as the ParamName.
                        self.cursor.next();
                        emit_param_name!(Token::new(TokenKind::ParamName("!".into()), Span::new(off, l, c)));
                    } else {
                        // `${!name…}` — emit indirect prefix; name comes next call.
                        // Record `indirect` in the frame so Phase 2 can route a
                        // trailing `*}`/`@}` to the prefix-name form.
                        self.cursor.next();
                        if let Some(Mode::ParamExpansion { indirect, .. }) = self.modes.last_mut() {
                            *indirect = true;
                        }
                        self.history.push(Token::new(TokenKind::ParamIndirect, Span::new(off, l, c)));
                    }
                    return Ok(Step::Produced);
                }

                // `$` — either the `$` special param (`${$}`, `${$:-x}`), a
                // `$'…'` extquote NAME (`${$'x1'}`), or `$"…"` (locale) → bad subst.
                Some('$') => {
                    let mut probe = self.cursor.clone();
                    probe.next(); // skip `$`
                    match probe.peek().copied() {
                        // `${$'…'}` — decode the ANSI-C run into the name.
                        Some('\'') => emit_ext_name!(),
                        // `${$"…"}` — locale quote in name position → bad subst.
                        Some('"') => emit_bad_subst!(),
                        // `${$}` / `${$:-x}` / `${$x}` — treat `$` as the name; a
                        // following non-modifier char is bad-subst'd in Phase 2.
                        _ => {
                            self.cursor.next();
                            emit_param_name!(Token::new(TokenKind::ParamName("$".into()), Span::new(off, l, c)));
                        }
                    }
                }

                // Special single-char names: @ * - ?
                // These are consumed as the full name.
                Some(sc @ ('@' | '*' | '-' | '?')) => {
                    self.cursor.next();
                    emit_param_name!(Token::new(TokenKind::ParamName(sc.to_string()), Span::new(off, l, c)));
                }

                // Positional parameter: ${1}, ${10}, ${42}
                Some(d) if d.is_ascii_digit() => {
                    let mut name = String::new();
                    while let Some(&dc) = self.cursor.peek() {
                        if dc.is_ascii_digit() { name.push(dc); self.cursor.next(); } else { break; }
                    }
                    emit_param_name!(Token::new(TokenKind::ParamName(name), Span::new(off, l, c)));
                }

                // Regular identifier name: [_A-Za-z][_A-Za-z0-9]*, extended across
                // any `$'…'` extquote runs (`${a$'b'}` → `ab`).
                Some(nc) if nc == '_' || nc.is_ascii_alphabetic() => emit_ext_name!(),

                // EOF inside `${` — error.
                None => return Err(LexError::UnterminatedBrace),

                // Unrecognised char in name position (`${'x'}`, `${"x"}`, `${.}`,
                // …) — bad substitution with the verbatim `${…}` raw.
                Some(_) => emit_bad_subst!(),
            }
        }

        // ── Phase 2: post-name (emit LBracket, ParamOp, or ParamClose) ──────────
        let off = self.cursor.offset();
        let l   = self.cursor.line();
        let c   = self.cursor.column();

        // Was the `${!…}` indirect prefix emitted? Needed to route a trailing
        // `*}`/`@}` to the prefix-name form (`${!pfx*}` / `${!pfx@}`).
        let indirect = matches!(self.modes.last(), Some(Mode::ParamExpansion { indirect: true, .. }));

        match self.cursor.peek().copied() {
            // Closing brace → ParamClose (bare name or after subscript/op).
            Some('}') => {
                self.cursor.next();
                self.history.push(Token::new(TokenKind::ParamClose, Span::new(off, l, c)));
                Ok(Step::Produced)
            }

            // Subscript opener → LBracket; parser will push subscript mode.
            Some('[') => {
                self.cursor.next();
                self.history.push(Token::new(TokenKind::LBracket, Span::new(off, l, c)));
                Ok(Step::Produced)
            }

            // `:` — either `:-`, `:=`, `:?`, `:+` (colon forms) or `:` alone (Substring).
            Some(':') => {
                self.cursor.next(); // consume `:`
                let kind = match self.cursor.peek().copied() {
                    Some('-') => { self.cursor.next(); ParamOpKind::UseDefault(true)    }
                    Some('=') => { self.cursor.next(); ParamOpKind::AssignDefault(true) }
                    Some('?') => { self.cursor.next(); ParamOpKind::ErrorIfUnset(true)  }
                    Some('+') => { self.cursor.next(); ParamOpKind::UseAlternate(true)  }
                    // `:` not followed by one of the four → Substring; only `:` consumed.
                    _         =>                       ParamOpKind::Substring,
                };
                self.history.push(Token::new(TokenKind::ParamOp(kind), Span::new(off, l, c)));
                Ok(Step::Produced)
            }

            // Bare (non-colon) forms.
            Some('-') => { self.cursor.next(); self.history.push(Token::new(TokenKind::ParamOp(ParamOpKind::UseDefault(false)),    Span::new(off, l, c))); Ok(Step::Produced) }
            Some('=') => { self.cursor.next(); self.history.push(Token::new(TokenKind::ParamOp(ParamOpKind::AssignDefault(false)), Span::new(off, l, c))); Ok(Step::Produced) }
            Some('?') => { self.cursor.next(); self.history.push(Token::new(TokenKind::ParamOp(ParamOpKind::ErrorIfUnset(false)),  Span::new(off, l, c))); Ok(Step::Produced) }
            Some('+') => { self.cursor.next(); self.history.push(Token::new(TokenKind::ParamOp(ParamOpKind::UseAlternate(false)),  Span::new(off, l, c))); Ok(Step::Produced) }

            // `#` / `##` → RemovePrefix.
            Some('#') => {
                self.cursor.next();
                let longest = self.cursor.peek() == Some(&'#');
                if longest { self.cursor.next(); }
                self.history.push(Token::new(TokenKind::ParamOp(ParamOpKind::RemovePrefix(longest)), Span::new(off, l, c)));
                Ok(Step::Produced)
            }

            // `%` / `%%` → RemoveSuffix.
            Some('%') => {
                self.cursor.next();
                let longest = self.cursor.peek() == Some(&'%');
                if longest { self.cursor.next(); }
                self.history.push(Token::new(TokenKind::ParamOp(ParamOpKind::RemoveSuffix(longest)), Span::new(off, l, c)));
                Ok(Step::Produced)
            }

            // `/` / `//` / `/#` / `/%` → Substitute.
            Some('/') => {
                self.cursor.next();
                let kind = if self.cursor.peek() == Some(&'/') {
                    self.cursor.next();
                    SubstKind::All
                } else {
                    match self.cursor.peek().copied() {
                        Some('#') => { self.cursor.next(); SubstKind::Prefix }
                        Some('%') => { self.cursor.next(); SubstKind::Suffix }
                        _         =>                       SubstKind::First
                    }
                };
                self.history.push(Token::new(TokenKind::ParamOp(ParamOpKind::Substitute(kind)), Span::new(off, l, c)));
                Ok(Step::Produced)
            }

            // `^` / `^^` → Case(Upper).
            Some('^') => {
                self.cursor.next();
                let all = self.cursor.peek() == Some(&'^');
                if all { self.cursor.next(); }
                self.history.push(Token::new(TokenKind::ParamOp(ParamOpKind::Case(CaseDirection::Upper, all)), Span::new(off, l, c)));
                Ok(Step::Produced)
            }

            // `,` / `,,` → Case(Lower).
            Some(',') => {
                self.cursor.next();
                let all = self.cursor.peek() == Some(&',');
                if all { self.cursor.next(); }
                self.history.push(Token::new(TokenKind::ParamOp(ParamOpKind::Case(CaseDirection::Lower, all)), Span::new(off, l, c)));
                Ok(Step::Produced)
            }

            // `${!pfx*}` — prefix-name expansion (star form). Only valid after the
            // `${!…}` indirect prefix and only when `*` is IMMEDIATELY followed by
            // `}`. Any other `*` after a name is a bad substitution.
            Some('*') => {
                let mut probe = self.cursor.clone();
                probe.next(); // skip `*`
                if indirect && probe.peek() == Some(&'}') {
                    self.cursor.next(); // `*`
                    self.cursor.next(); // `}`
                    self.history.push(Token::new(TokenKind::ParamPrefixClose { at: false }, Span::new(off, l, c)));
                    Ok(Step::Produced)
                } else {
                    emit_bad_subst!()
                }
            }

            // `@…` — `${!pfx@}` prefix-name (at form), `@<letter>` transform, or
            // (no valid op letter) bad substitution.
            Some('@') => {
                self.cursor.next(); // consume `@`
                // `${!pfx@}` — prefix-name expansion (at form).
                if indirect && self.cursor.peek() == Some(&'}') {
                    self.cursor.next(); // `}`
                    self.history.push(Token::new(TokenKind::ParamPrefixClose { at: true }, Span::new(off, l, c)));
                    return Ok(Step::Produced);
                }
                let transform_op = match self.cursor.peek().copied() {
                    Some('P') => { self.cursor.next(); Some(TransformOp::PromptExpand) }
                    Some('Q') => { self.cursor.next(); Some(TransformOp::Quote) }
                    Some('U') => { self.cursor.next(); Some(TransformOp::Upper) }
                    Some('L') => { self.cursor.next(); Some(TransformOp::Lower) }
                    Some('u') => { self.cursor.next(); Some(TransformOp::UpperFirst) }
                    Some('E') => { self.cursor.next(); Some(TransformOp::EscapeExpand) }
                    Some('A') => { self.cursor.next(); Some(TransformOp::AssignDecl) }
                    Some('K') => { self.cursor.next(); Some(TransformOp::KvString) }
                    Some('k') => { self.cursor.next(); Some(TransformOp::KvWords) }
                    Some('a') => { self.cursor.next(); Some(TransformOp::AttrFlags) }
                    _         => None,
                };
                match transform_op {
                    Some(op) => {
                        self.history.push(Token::new(TokenKind::ParamOp(ParamOpKind::Transform(op)), Span::new(off, l, c)));
                        Ok(Step::Produced)
                    }
                    // Unknown/missing op letter (`${V@}`, `${x@Z}`) — bad substitution.
                    None => emit_bad_subst!(),
                }
            }

            // EOF inside the expansion.
            None => Err(LexError::UnterminatedBrace),

            // Unrecognised char after name (`${x!}`, `${-3}`, …) — bad substitution.
            Some(_) => emit_bad_subst!(),
        }
    }

    /// Emits one operand atom under one of the four `Param*Operand` modes.
    ///
    /// `sep`  — the mode's active separator char (`/` for pattern, `:` for offset-length,
    ///          `None` for word/subscript). An unquoted `sep` char emits `ParamSep`.
    /// `end`  — the terminator: `}` for `${…}` operands, `]` for subscript.
    ///          An unquoted `end` char emits `ParamClose` (for `}`) or `RBracket` (for `]`).
    ///
    /// One atom per call (except for `"…"` double-quoted spans, which emit all
    /// their content atoms into `self.history` in one call — avoids per-frame
    /// dquote state).  Mirrors `parse_braced_operand_opts` (lexer.rs ~3389) for
    /// char decisions but emits `TokenKind` atoms instead of building `WordPart`s.
    /// Updates the `in_dquote` field of whichever operand `Mode` is currently on
    /// the mode stack.  Called after all cursor borrows are released to avoid
    /// borrow-checker conflicts (mirrors the `seen_name` write-back pattern from T2).
    fn set_operand_in_dquote(&mut self, val: bool) {
        match self.modes.last_mut() {
            Some(
                Mode::ParamWordOperand            { in_dquote, .. }
                | Mode::ParamSubstPatternOperand    { in_dquote, .. }
                | Mode::ParamSubstringOffsetOperand { in_dquote, .. }
                | Mode::ParamSubscriptOperand       { in_dquote, .. },
            ) => *in_dquote = val,
            _ => {}
        }
    }

    /// Emits ONE atom per call for a parameter operand.
    ///
    /// `sep`       — the optional intra-operand separator char (`/` or `:`), or `None`.
    /// `end`       — the operand terminator: `}` for word/pattern/offset, `]` for subscript.
    /// `in_dquote` — snapshot of this frame's `in_dquote` flag (copied from the mode
    ///               variant by `scan_step`; written back via `set_operand_in_dquote`).
    ///               This is the scanner's INTERNAL `"…"`-span state, NOT the
    ///               enclosing quote context — see `enclosing_dquote` below.
    /// `enclosing_dquote` — v264: whether the `${…}` this operand belongs to is
    ///               itself inside a double-quoted word (seeded once at the mode's
    ///               push site from the parser's `quoted` flag; only the
    ///               UseDefault/AssignDefault/UseAlternate value-operand sites seed
    ///               `true` for a quoted enclosing `"…"` — see `parse_param_expansion`).
    ///               When set, the NORMAL (non-`in_dquote`) branch below applies
    ///               dquote rules instead of unquoted rules: a bare `'` is a
    ///               literal char (not a `'…'` span), and `\` is special only
    ///               before `$ ` " \` (mirrors `parse_braced_operand_opts`).
    ///
    /// **Flat, non-recursive tokenization of `"…"` spans:**
    ///
    /// When `in_dquote == false` and the next char is `"`, the opening `"` is consumed
    /// and the FIRST interior atom is emitted in the SAME call (to guarantee forward
    /// progress).  If the interior contains a `$`/backtick trigger or if the literal
    /// run didn't reach the closing `"`, `in_dquote` is set to `true` in the mode
    /// frame so subsequent calls scan the rest of the span.  A closing `"` while
    /// `in_dquote == true` consumes it, flips the frame back to `false`, and returns
    /// `Step::Produced` without a token (the cursor advanced — no spin).
    ///
    /// The critical invariant: inside a `"…"` span, `}`, `/`, `:`, etc. are treated
    /// as ordinary literals and are NOT emitted as `ParamClose`/`ParamSep`.  Only
    /// `"`, `$`, `` ` ``, and `\` are special.
    fn scan_step_param_operand(&mut self, sep: Option<char>, end: char, in_dquote: bool, enclosing_dquote: bool) -> Result<Step, LexError> {
        let off = self.cursor.offset();
        let l   = self.cursor.line();
        let c   = self.cursor.column();

        if in_dquote {
            // ── Inside a double-quoted span ───────────────────────────────────
            match self.cursor.peek().copied() {
                None => return Err(LexError::UnterminatedQuote),

                // Closing `"` — flip frame back to unquoted; no token emitted this call.
                // Returning `Step::Produced` is safe: the cursor advanced, so no spin.
                Some('"') => {
                    self.cursor.next();
                    self.set_operand_in_dquote(false);
                    return Ok(Step::Produced);
                }

                // Backslash: special only before `$`, `` ` ``, `"`, `\` inside `"…"`.
                Some('\\') => {
                    self.cursor.next(); // consume `\`
                    match self.cursor.peek().copied() {
                        Some(e @ ('$' | '`' | '"' | '\\')) => {
                            self.cursor.next();
                            self.history.push(Token::new(
                                TokenKind::Lit { text: e.to_string(), quoted: true },
                                Span::new(off, l, c),
                            ));
                        }
                        _ => {
                            let mut s = String::from("\\");
                            if let Some(ch) = self.cursor.next() { s.push(ch); }
                            self.history.push(Token::new(
                                TokenKind::Lit { text: s, quoted: true },
                                Span::new(off, l, c),
                            ));
                        }
                    }
                    return Ok(Step::Produced);
                }

                // Backtick command substitution — emit BeginBacktick SIGNAL without consuming `` ` ``.
                // Cursor stays at `` ` `` so parse_backtick_sub (which pushes Mode::Backtick)
                // can own consuming the `` ` `` via scan_step_backtick(depth=0).
                // Mirrors the CmdSubOpen-signal pattern for `$(` (v244 T4).
                Some('`') => {
                    self.history.push(Token::new(TokenKind::BeginBacktick, Span::new(off, l, c)));
                    return Ok(Step::Produced);
                }

                // `$` — nested expansion inside `"…"`.
                Some('$') => {
                    let mut probe = self.cursor.clone();
                    probe.next(); // skip `$`
                    match probe.peek().copied() {
                        // `${` — emit ParamOpen; the parser pushes a nested ParamExpansion.
                        // Our frame's `in_dquote=true` is preserved in `modes` and restored
                        // after the parser pops the nested frame (mark/rewind is stack-safe).
                        Some('{') => {
                            self.cursor.next(); // `$`
                            self.cursor.next(); // `{`
                            self.history.push(Token::new(TokenKind::ParamOpen { quoted: true }, Span::new(off, l, c)));
                        }
                        Some('(') => {
                            // NOTE (v246 T6): unlike the unquoted operand site below,
                            // this continuing-nested-dquote site is NOT wired to
                            // ArithOpen. `CmdSubOpen` was likewise never wired here for
                            // `$(cmd)` (still deferred) — see the `DeferredExpansion`
                            // TokenKind doc comment. Signal atoms (ArithOpen/CmdSubOpen/
                            // BeginBacktick) carry no `quoted` bit of their own, so
                            // wiring this site would silently drop the "inside a nested
                            // `"…"`" quoted-context onto the resulting WordPart (verified:
                            // produces `quoted:false` where the oracle emits `true`).
                            // Both `$((` and `$(cmd)` remain deferred here.
                            self.cursor.next(); // `$`
                            self.cursor.next(); // `(`
                            self.history.push(Token::new(TokenKind::DeferredExpansion, Span::new(off, l, c)));
                        }
                        Some(sp @ ('?' | '@' | '*' | '#' | '$' | '!' | '-')) => {
                            self.cursor.next(); // `$`
                            self.cursor.next(); // special char
                            self.history.push(Token::new(TokenKind::DollarName { name: sp.to_string(), quoted: true }, Span::new(off, l, c)));
                        }
                        Some(d) if d.is_ascii_digit() => {
                            self.cursor.next(); // `$`
                            let digit = self.cursor.next().unwrap();
                            self.history.push(Token::new(TokenKind::DollarName { name: digit.to_string(), quoted: true }, Span::new(off, l, c)));
                        }
                        Some(nc) if is_name_start(nc) => {
                            self.cursor.next(); // `$`
                            let name = scan_var_name(&mut self.cursor);
                            self.history.push(Token::new(TokenKind::DollarName { name, quoted: true }, Span::new(off, l, c)));
                        }
                        _ => {
                            self.cursor.next(); // lone `$`
                            self.history.push(Token::new(
                                TokenKind::Lit { text: "$".into(), quoted: true },
                                Span::new(off, l, c),
                            ));
                        }
                    }
                    return Ok(Step::Produced);
                }

                // Literal run inside `"…"`: `}`, `/`, `:`, `]` etc. are NOT terminators.
                Some(_) => {
                    let mut text = String::new();
                    while let Some(&ch) = self.cursor.peek() {
                        if matches!(ch, '"' | '$' | '`' | '\\') { break; }
                        text.push(ch);
                        self.cursor.next();
                    }
                    // text is non-empty: the match arm fired on a non-special char.
                    self.history.push(Token::new(
                        TokenKind::Lit { text, quoted: true },
                        Span::new(off, l, c),
                    ));
                    return Ok(Step::Produced);
                }
            }
        } else {
            // ── Outside double-quoted span ────────────────────────────────────
            match self.cursor.peek().copied() {
                // EOF inside an operand — the enclosing `${…}` is unterminated.
                None => return Err(LexError::UnterminatedBrace),

                // Unquoted `end` char (`}` or `]`) — emit the matching close atom.
                Some(ch) if ch == end => {
                    self.cursor.next();
                    let kind = if end == '}' { TokenKind::ParamClose } else { TokenKind::RBracket };
                    self.history.push(Token::new(kind, Span::new(off, l, c)));
                    return Ok(Step::Produced);
                }

                // Unquoted separator char (`/` or `:`) — emit ParamSep.
                Some(ch) if Some(ch) == sep => {
                    self.cursor.next();
                    self.history.push(Token::new(TokenKind::ParamSep, Span::new(off, l, c)));
                    return Ok(Step::Produced);
                }

                // `$` — decide based on the next char.
                Some('$') => {
                    let mut probe = self.cursor.clone();
                    probe.next(); // skip `$`
                    match probe.peek().copied() {
                        // `${` — emit ParamOpen; the parser pushes a nested ParamExpansion.
                        Some('{') => {
                            self.cursor.next(); // `$`
                            self.cursor.next(); // `{`
                            self.history.push(Token::new(TokenKind::ParamOpen { quoted: false }, Span::new(off, l, c)));
                        }
                        // `$(` — v244 T4: distinguish `$(cmd)` from `$((`.
                        // `probe` is cloned past `$`; peek() is at the first `(`.
                        // Probe one more to detect `$((` (arithmetic, still deferred).
                        Some('(') => {
                            let mut probe2 = probe.clone();
                            probe2.next(); // skip first `(`
                            if probe2.peek() == Some(&'(') {
                                // v246: `$((` — arithmetic expansion. Emit a ZERO-WIDTH
                                // ArithOpen signal (do NOT consume `$((`); cursor stays at
                                // `$` so parse_arith_expansion (which pushes Mode::Arith,
                                // whose first scan consumes `$((`) can own it.
                                self.history.push(Token::new(TokenKind::ArithOpen, Span::new(off, l, c)));
                            } else {
                                // `$(cmd)` — emit CmdSubOpen SIGNAL without consuming `$(`.
                                // Cursor stays at `$` so parse_command_sub (which pushes
                                // Mode::CommandSub) can own consuming `$(` via
                                // scan_step_command_sub(false).
                                self.history.push(Token::new(TokenKind::CmdSubOpen, Span::new(off, l, c)));
                            }
                        }
                        // `$[expr]` legacy arith (v258) — zero-width `LegacyArithOpen`
                        // signal (cursor stays on `$`); mirrors the `$((` ArithOpen arm
                        // above. parse_legacy_arith_expansion (which pushes
                        // Mode::Arith{delim:Bracket}) owns consuming `$[`.
                        Some('[') => {
                            self.history.push(Token::new(TokenKind::LegacyArithOpen, Span::new(off, l, c)));
                        }
                        // `$'…'` ANSI-C quoting — decode via the shared leaf and emit
                        // a `QuoteRun{AnsiC}` (parse_word wraps it in Quoted{AnsiC}).
                        // Mirrors the command scanner's `$'…'` arm (lexer.rs ~3792)
                        // and the oracle's `scan_dollar_expansion` `Some('\'')` arm.
                        Some('\'') => {
                            self.cursor.next(); // `$`
                            self.cursor.next(); // `'`
                            let text = scan_ansi_c_quoted(&mut self.cursor)?;
                            self.history.push(Token::new(TokenKind::QuoteRun { style: QuoteStyle::AnsiC, text }, Span::new(off, l, c)));
                        }
                        // `$"…"` locale quoting (identity) — drop the `$`, emit the
                        // zero-width BeginDquote (cursor left on `"`); mirrors the
                        // CF4 emit_unquoted_dollar_atom arm and the oracle's
                        // scan_dollar_expansion `Some('"') if !quoted => {}`. UNLIKE
                        // a BARE `"…"` in this same operand scanner (which this
                        // function's own "outside dquote" `Some('"')` arm below
                        // inlines flat, no BeginDquote signal, no Mode::DoubleQuote),
                        // `$"…"` goes through the general BeginDquote/parse_dquote
                        // path — `parse_word`'s new BeginDquote arm then decides
                        // flat-vs-wrapped per the enclosing operand mode (v259 F3).
                        // Only outside an enclosing double quote (in_dquote guard).
                        Some('"') if !in_dquote => {
                            self.cursor.next(); // consume `$` only, leave `"`
                            self.history.push(Token::new(TokenKind::BeginDquote, Span::new(off, l, c)));
                        }
                        // Special single-char params: `$?` `$@` `$*` `$#` `$$` `$!` `$-`.
                        Some(sp @ ('?' | '@' | '*' | '#' | '$' | '!' | '-')) => {
                            self.cursor.next(); // `$`
                            self.cursor.next(); // special char
                            self.history.push(Token::new(TokenKind::DollarName { name: sp.to_string(), quoted: false }, Span::new(off, l, c)));
                        }
                        // Positional parameter: `$0`–`$9`.
                        Some(d) if d.is_ascii_digit() => {
                            self.cursor.next(); // `$`
                            let digit = self.cursor.next().unwrap();
                            self.history.push(Token::new(TokenKind::DollarName { name: digit.to_string(), quoted: false }, Span::new(off, l, c)));
                        }
                        // Regular variable name: `$name`.
                        Some(nc) if is_name_start(nc) => {
                            self.cursor.next(); // `$`
                            let name = scan_var_name(&mut self.cursor);
                            self.history.push(Token::new(TokenKind::DollarName { name, quoted: false }, Span::new(off, l, c)));
                        }
                        // Lone `$` — literal.
                        _ => {
                            self.cursor.next();
                            self.history.push(Token::new(
                                TokenKind::Lit { text: "$".into(), quoted: false },
                                Span::new(off, l, c),
                            ));
                        }
                    }
                    return Ok(Step::Produced);
                }

                // Backtick command-substitution — emit BeginBacktick SIGNAL without consuming `` ` ``.
                // Cursor stays at `` ` `` so parse_backtick_sub (which pushes Mode::Backtick)
                // can own consuming the `` ` `` via scan_step_backtick(depth=0).
                // Mirrors the CmdSubOpen-signal pattern for `$(` (v244 T4).
                Some('`') => {
                    self.history.push(Token::new(TokenKind::BeginBacktick, Span::new(off, l, c)));
                    return Ok(Step::Produced);
                }

                // Single-quoted span: everything literal (backslash NOT special inside `'…'`).
                // v264: NOT reached when `enclosing_dquote` — a `'` is just an
                // ordinary literal char in dquote context (dquote semantics: single
                // quotes don't quote anything), so it falls through to the
                // "Unquoted literal run" arm below, which excludes `'` from its
                // stop-set in that case. Mirrors `parse_braced_operand_opts`'s
                // `'\'' if enclosing_dquote => cur.push('\'')` arm.
                Some('\'') if !enclosing_dquote => {
                    self.cursor.next(); // consume opening `'`
                    let mut text = String::new();
                    loop {
                        match self.cursor.next() {
                            None => return Err(LexError::UnterminatedQuote),
                            Some('\'') => break,
                            Some(ch) => text.push(ch),
                        }
                    }
                    // v263: a subscript operand wraps a bare `'…'` in
                    // Quoted{Single} (oracle scan_subscript). Emit QuoteRun{Single}
                    // so parse_word wraps it; value families keep the flat Lit.
                    let tok = if end == ']' {
                        TokenKind::QuoteRun { style: QuoteStyle::Single, text }
                    } else {
                        TokenKind::Lit { text, quoted: true }
                    };
                    self.history.push(Token::new(tok, Span::new(off, l, c)));
                    return Ok(Step::Produced);
                }

                // Opening `"` — begin a double-quoted span.
                //
                // To guarantee forward progress (emit at least one token per call),
                // we consume the `"` and immediately scan the FIRST interior atom.
                // The `in_dquote` frame flag is set to `true` only when more atoms
                // remain inside the span (i.e. the literal run didn't reach the
                // closing `"` in this same call, or the first interior char is a
                // `$`/backtick trigger that the parser must handle before we see `"`).
                Some('"') => {
                    // v263: in a subscript operand, wrap a bare `"…"` in
                    // Quoted{Double} (like the oracle's scan_subscript). Emit a
                    // zero-width BeginDquote — leave the `"` for parse_dquote,
                    // exactly like the `$"` arm — so parse_word's F3 arm wraps it;
                    // the mode switch to Mode::DoubleQuote guarantees forward
                    // progress. Value families (end == '}') keep the flat inline.
                    if end == ']' {
                        self.history.push(Token::new(TokenKind::BeginDquote, Span::new(off, l, c)));
                        return Ok(Step::Produced);
                    }
                    self.cursor.next(); // consume opening `"`
                    let in_off = self.cursor.offset();
                    let in_l   = self.cursor.line();
                    let in_c   = self.cursor.column();
                    match self.cursor.peek().copied() {
                        None => return Err(LexError::UnterminatedQuote),

                        // Empty `""` — emit empty quoted Lit (preserves `""` = empty-string
                        // semantics); in_dquote stays false.
                        Some('"') => {
                            self.cursor.next(); // consume closing `"`
                            self.history.push(Token::new(
                                TokenKind::Lit { text: String::new(), quoted: true },
                                Span::new(in_off, in_l, in_c),
                            ));
                            return Ok(Step::Produced);
                        }

                        // Backslash as the first char inside `"…"`.
                        Some('\\') => {
                            self.cursor.next(); // consume `\`
                            match self.cursor.peek().copied() {
                                Some(e @ ('$' | '`' | '"' | '\\')) => {
                                    self.cursor.next();
                                    self.history.push(Token::new(
                                        TokenKind::Lit { text: e.to_string(), quoted: true },
                                        Span::new(in_off, in_l, in_c),
                                    ));
                                }
                                _ => {
                                    let mut s = String::from("\\");
                                    if let Some(ch) = self.cursor.next() { s.push(ch); }
                                    self.history.push(Token::new(
                                        TokenKind::Lit { text: s, quoted: true },
                                        Span::new(in_off, in_l, in_c),
                                    ));
                                }
                            }
                            // We don't know if we've reached the closing `"` yet;
                            // set in_dquote=true so subsequent calls stay inside the span.
                            self.set_operand_in_dquote(true);
                            return Ok(Step::Produced);
                        }

                        // Backtick as the first char inside `"…"`.
                        Some('`') => {
                            self.cursor.next();
                            self.history.push(Token::new(
                                TokenKind::DeferredExpansion,
                                Span::new(in_off, in_l, in_c),
                            ));
                            self.set_operand_in_dquote(true);
                            return Ok(Step::Produced);
                        }

                        // `$` as the first char inside `"…"`.
                        Some('$') => {
                            let mut probe = self.cursor.clone();
                            probe.next(); // skip `$`
                            match probe.peek().copied() {
                                Some('{') => {
                                    self.cursor.next(); // `$`
                                    self.cursor.next(); // `{`
                                    // Emit ParamOpen; in_dquote=true survives the nested
                                    // ParamExpansion push/pop because it lives in our frame.
                                    self.history.push(Token::new(TokenKind::ParamOpen { quoted: true }, Span::new(in_off, in_l, in_c)));
                                    self.set_operand_in_dquote(true);
                                }
                                Some('(') => {
                                    self.cursor.next(); // `$`
                                    self.cursor.next(); // `(`
                                    self.history.push(Token::new(TokenKind::DeferredExpansion, Span::new(in_off, in_l, in_c)));
                                    self.set_operand_in_dquote(true);
                                }
                                Some(sp @ ('?' | '@' | '*' | '#' | '$' | '!' | '-')) => {
                                    self.cursor.next(); // `$`
                                    self.cursor.next(); // special char
                                    self.history.push(Token::new(TokenKind::DollarName { name: sp.to_string(), quoted: true }, Span::new(in_off, in_l, in_c)));
                                    self.set_operand_in_dquote(true);
                                }
                                Some(d) if d.is_ascii_digit() => {
                                    self.cursor.next(); // `$`
                                    let digit = self.cursor.next().unwrap();
                                    self.history.push(Token::new(TokenKind::DollarName { name: digit.to_string(), quoted: true }, Span::new(in_off, in_l, in_c)));
                                    self.set_operand_in_dquote(true);
                                }
                                Some(nc) if is_name_start(nc) => {
                                    self.cursor.next(); // `$`
                                    let name = scan_var_name(&mut self.cursor);
                                    self.history.push(Token::new(TokenKind::DollarName { name, quoted: true }, Span::new(in_off, in_l, in_c)));
                                    self.set_operand_in_dquote(true);
                                }
                                _ => {
                                    self.cursor.next(); // lone `$`
                                    self.history.push(Token::new(
                                        TokenKind::Lit { text: "$".into(), quoted: true },
                                        Span::new(in_off, in_l, in_c),
                                    ));
                                    self.set_operand_in_dquote(true);
                                }
                            }
                            return Ok(Step::Produced);
                        }

                        // Literal run as the first content inside `"…"`.
                        // Stop at the next `"`, `$`, `` ` ``, or `\`.
                        // If the run ends at the closing `"`, consume it in this same call
                        // (avoids an extra round-trip for the common `"plain text"` case).
                        Some(_) => {
                            let mut text = String::new();
                            while let Some(&ch) = self.cursor.peek() {
                                if matches!(ch, '"' | '$' | '`' | '\\') { break; }
                                text.push(ch);
                                self.cursor.next();
                            }
                            // text is non-empty: the match arm matched a non-special char.
                            self.history.push(Token::new(
                                TokenKind::Lit { text, quoted: true },
                                Span::new(in_off, in_l, in_c),
                            ));
                            // If we stopped exactly at the closing `"`, consume it here so
                            // in_dquote stays false; otherwise set in_dquote=true.
                            if self.cursor.peek() == Some(&'"') {
                                self.cursor.next(); // consume closing `"`
                                // in_dquote stays false — no frame update needed
                            } else {
                                self.set_operand_in_dquote(true);
                            }
                            return Ok(Step::Produced);
                        }
                    }
                }

                // Backslash escape outside `"…"`.
                //
                // v264: when `enclosing_dquote`, apply DOUBLE-QUOTE escape rules
                // (mirrors the internal-span branch above and
                // `parse_braced_operand_opts`'s `'\\' if enclosing_dquote` arm):
                // only `$ ` " \` are escape targets (emit just that char); any
                // other char keeps the backslash (`\` + char both literal).
                Some('\\') if enclosing_dquote => {
                    self.cursor.next(); // consume `\`
                    match self.cursor.peek().copied() {
                        Some(e @ ('$' | '`' | '"' | '\\')) => {
                            self.cursor.next();
                            self.history.push(Token::new(
                                TokenKind::Lit { text: e.to_string(), quoted: true },
                                Span::new(off, l, c),
                            ));
                        }
                        _ => {
                            let mut s = String::from("\\");
                            if let Some(ch) = self.cursor.next() { s.push(ch); }
                            self.history.push(Token::new(
                                TokenKind::Lit { text: s, quoted: true },
                                Span::new(off, l, c),
                            ));
                        }
                    }
                    return Ok(Step::Produced);
                }

                // Backslash escape outside `"…"`: the next char is always literal
                // (mirrors `parse_braced_operand_opts` for the unquoted path).
                Some('\\') => {
                    self.cursor.next(); // consume `\`
                    let text = match self.cursor.next() {
                        Some(ch) => ch.to_string(),
                        None     => "\\".to_string(), // trailing `\` at EOF — keep as literal
                    };
                    self.history.push(Token::new(
                        TokenKind::Lit { text, quoted: true },
                        Span::new(off, l, c),
                    ));
                    return Ok(Step::Produced);
                }

                // Unquoted literal run: accumulate until the next special char or terminator.
                //
                // v264: when `enclosing_dquote`, dquote rules apply INSIDE this run
                // (rather than as separate one-char/one-atom arms) so consecutive
                // dquote-literal text merges into ONE `Lit` atom — matching
                // `parse_braced_operand_opts`'s single flushed `cur` buffer:
                //   - `'` is not special — accumulated like any other char (see the
                //     guarded single-quote arm above, which only fires `!enclosing_dquote`).
                //   - `\` followed by a non-escape-target char (not `$ ` " \`) keeps
                //     BOTH chars literally and the run continues (mirrors the
                //     oracle's `_ => cur.push('\\')` arm, which does NOT flush).
                //   - `\` followed by an escape target (or at EOF) stops the run so
                //     the dedicated backslash arm below can flush+emit its own atom
                //     (mirrors the oracle's `flush_literal` there).
                Some(_) => {
                    let mut text = String::new();
                    while let Some(&ch) = self.cursor.peek() {
                        let is_term = ch == end || Some(ch) == sep;
                        if is_term { break; }
                        if ch == '\\' {
                            if !enclosing_dquote { break; }
                            let mut probe = self.cursor.clone();
                            probe.next(); // skip `\`
                            match probe.peek().copied() {
                                Some('$' | '`' | '"' | '\\') | None => break,
                                Some(next_ch) => {
                                    self.cursor.next(); // consume `\`
                                    self.cursor.next(); // consume next_ch
                                    text.push('\\');
                                    text.push(next_ch);
                                    continue;
                                }
                            }
                        }
                        if ch == '\'' && !enclosing_dquote { break; }
                        if matches!(ch, '$' | '"' | '`') { break; }
                        text.push(ch);
                        self.cursor.next();
                    }
                    // `text` is non-empty: the outer match arm matched `Some(_)` (a non-special
                    // char) so we consumed at least one char into the run above.
                    self.history.push(Token::new(
                        TokenKind::Lit { text, quoted: false },
                        Span::new(off, l, c),
                    ));
                    return Ok(Step::Produced);
                }
            }
        }
    }

    /// Emits atoms for `Mode::CommandSub { body_started }`.
    ///
    /// When `body_started == false` (the frame was just pushed), the cursor sits on
    /// `$`; consume `$` and `(`, then:
    ///   - next char is `(` → emit `DeferredExpansion` (defer `$((`).
    ///   - otherwise → flip the frame to `body_started: true` and emit `CmdSubOpen`.
    ///
    /// When `body_started == true`, the opener has already been emitted; delegate
    /// entirely to `scan_step_command()` so body tokens are produced one at a time
    /// in Command mode.  The terminating `)` comes out as `Op(RParen)`.
    /// Route a nested command/backtick BODY step to the same scanner the
    /// top-level `Mode::Command` dispatch uses (lexer.rs:1032): the atom scanner
    /// when `command_atoms` is on (production after the v264 flip), else the
    /// oracle. Before v264 the nested-body sites hardcoded `scan_step_command`
    /// (oracle), which mis-parsed / hung on `[[ … ]]` and extglob groups inside
    /// `$( … )` / `` `…` `` because the enclosing parser is the atom parser.
    ///
    /// NOTE: this routes to `scan_step_command_atoms_core`, NOT
    /// `scan_step_command_atoms`, so it BYPASSES the heredoc-body emission guard.
    /// A nested cmdsub/backtick body embedded in an EXPANDING heredoc body is
    /// scanned while `emitting_heredoc` is still `Some` (we are mid-body). The
    /// oracle `scan_step_command` had no such guard, so it scanned the embedded
    /// `$( … )` body directly; routing through the guarded entry would instead
    /// re-enter heredoc-body emission and error `UnsupportedExpansion` (v264).
    fn scan_step_command_body(&mut self) -> Result<Step, LexError> {
        if self.command_atoms {
            // A heredoc registered WITHIN this cmdsub/backtick body (`$(sh <<B …)`)
            // triggers emission at THIS depth; its body must be emitted here (the
            // guarded `scan_step_command_atoms` entry is only reached at the
            // `Mode::Command` floor, never for a pushed body mode). Gate on the
            // trigger depth so an ENCLOSING expanding heredoc — whose body merely
            // CONTAINS this cmdsub and which triggered at a shallower depth — does
            // NOT divert here (its cmdsub body is real command text).
            if let Some(state) = self.emitting_heredoc.as_ref() {
                if state.trigger_depth == self.modes.len() {
                    return self.scan_step_heredoc_body();
                }
            }
            self.scan_step_command_atoms_core()
        } else {
            self.scan_step_command()
        }
    }

    fn scan_step_command_sub(&mut self, body_started: bool) -> Result<Step, LexError> {
        if !body_started {
            // Record position BEFORE consuming the opener.
            let off = self.cursor.offset();
            let l   = self.cursor.line();
            let c   = self.cursor.column();
            match self.cursor.peek().copied() {
                Some('$') => {
                    self.cursor.next(); // consume `$`
                    // If the char after `$` is not `(`, defer gracefully.
                    if self.cursor.peek() != Some(&'(') {
                        self.history.push(Token::new(TokenKind::DeferredExpansion, Span::new(off, l, c)));
                        return Ok(Step::Produced);
                    }
                    self.cursor.next(); // consume `(`
                    if self.cursor.peek() == Some(&'(') && !self.retokenize_arith_as_cmdsub {
                        // `$((` — arithmetic expansion; defer to runtime.
                        self.history.push(Token::new(TokenKind::DeferredExpansion, Span::new(off, l, c)));
                        return Ok(Step::Produced);
                    }
                    // Either a real `$(cmd)`, OR the v246 wrinkle retry: treat `$((` as
                    // `$(` + a subshell `(`. Clear the one-shot flag; the second `(`
                    // stays unconsumed (cursor is on it) and lexes as the subshell
                    // opener in the body.
                    self.retokenize_arith_as_cmdsub = false;
                }
                Some('(') => {
                    // v251: process-substitution opener. The `<`/`>` was already
                    // consumed by scan_command_operator_atom; consume the `(`.
                    self.cursor.next();
                }
                _ => {
                    // Cursor is not on `$` or `(` (e.g. a backtick `` ` `` — its own
                    // iteration), emit DeferredExpansion rather than panicking. This
                    // keeps the dormant CommandSub mode robust when tests call
                    // parse_command_sub with non-`$(`/non-`(` input.
                    self.history.push(Token::new(TokenKind::DeferredExpansion, Span::new(off, l, c)));
                    return Ok(Step::Produced);
                }
            }
            // Flip the top-of-stack frame to body_started: true.
            if let Some(Mode::CommandSub { body_started }) = self.modes.last_mut() {
                *body_started = true;
            }
            // v264: the cmdsub body begins at a FRESH command/word start — the
            // outer `cmd_at_word_start` reflects the mid-word `$(` position
            // (false), but a `#` at `$(#…` opens a comment (the oracle uses
            // `!has_token`, which is fresh in the isolated body). Set it so the
            // first body atom is treated as word-start (comment / keyword / tilde).
            self.cmd_at_word_start = true;
            self.history.push(Token::new(TokenKind::CmdSubOpen, Span::new(off, l, c)));
            Ok(Step::Produced)
        } else {
            // Body is Command-mode tokens; the parser owns the terminating `)`.
            self.scan_step_command_body()
        }
    }

    /// `Mode::Backtick { depth }` scanner — v245 Task 2.
    ///
    /// **depth 0 — entry:** cursor sits on the opening `` ` ``.  Consume it,
    /// flip the top-of-stack frame to `depth = 1`, emit `BeginBacktick`.
    ///
    /// **depth 1 — body:** pre-peek the next char:
    /// - `` ` `` → closing backtick (terminator): flush any pending word token,
    ///   flip depth back to 0, emit `EndBacktick`.
    /// - EOF → defer to `finish()` (unterminated; parser surfaces the error).
    /// - anything else → delegate to `scan_step_command()`.  Because we've
    ///   already confirmed the char is NOT `` ` ``, the `` '`' `` arm inside
    ///   `scan_step_command` can never fire for this step, keeping production
    ///   `scan_step_command` behavior byte-identical.
    fn scan_step_backtick(&mut self, depth: u32) -> Result<Step, LexError> {
        if depth == 0 {
            // ENTRY: consume the opening backtick and emit BeginBacktick.
            let off = self.cursor.offset();
            let l   = self.cursor.line();
            let c   = self.cursor.column();
            debug_assert_eq!(self.cursor.peek(), Some(&'`'), "scan_step_backtick depth=0: expected opening `");
            self.cursor.next(); // consume '`'
            // Flip the mode frame: depth 0 → 1.
            if let Some(Mode::Backtick { depth: d }) = self.modes.last_mut() {
                *d = 1;
            }
            // v264: the backtick body begins at a FRESH command/word start (see
            // the cmdsub body note) so a `#` at `` `#… `` opens a comment.
            self.cmd_at_word_start = true;
            self.history.push(Token::new(TokenKind::BeginBacktick, Span::new(off, l, c)));
            Ok(Step::Produced)
        } else {
            // BODY at depth D = `depth` (≥ 1): pre-peek to intercept nested
            // delimiters (open a child / close this level) vs. body content.
            match self.cursor.peek() {
                None => {
                    // EOF inside body — flush any pending word, signal Eof.
                    self.finish()
                }
                Some(&'`') | Some(&'\\') => {
                    // THE UNIFIED DEPTH-AWARE `\`-RUN DECODE.  Peek the CONTIGUOUS
                    // backslash run (length B) and the char after it — a bounded
                    // LOCAL peek (≤ 2^D chars), NOT a scan for a matching '`':
                    //   B backslashes then '`' with B = 2^(D-1) − 1 → close (D → D−1).
                    //   B backslashes then '`' with B = 2^D − 1     → open  (D → D+1).
                    //   otherwise → the run is ESCAPE / body content.
                    // At D=1: close needs B=0 (bare '`'); open needs B=1 (`\``).
                    let mut b = 0usize;
                    while self.cursor.peek_nth(b) == Some('\\') {
                        b += 1;
                    }
                    let after   = self.cursor.peek_nth(b);
                    let close_b = (1usize << (depth - 1)) - 1; // 2^(D-1) − 1
                    let open_b  = (1usize << depth) - 1;       // 2^D − 1
                    if after == Some('`') && b == close_b {
                        // Close this level: consume the run + '`', flip depth D→D-1.
                        self.emit_backtick_delim(b, /*open=*/ false)?;
                        return Ok(Step::Produced);
                    } else if after == Some('`') && b == open_b {
                        // Open a child: consume the run + '`', flip depth D→D+1.
                        self.emit_backtick_delim(b, /*open=*/ true)?;
                        return Ok(Step::Produced);
                    } else if self.cursor.peek() == Some(&'\\') {
                        // ESCAPE / body content (the run is NOT a delimiter).
                        // Process by peeking the WHOLE run (never one-at-a-time):
                        //   \$  → consume `\`, expose `$` to the next scan_step_backtick
                        //         call (expandable dollar: `\$x` → variable `$x`).
                        //   \\  → consume both `\`; the surviving `\` re-tokenizes: emits
                        //         Quoted{Backslash,[Literal(X)]} for `\\X`, unquoted literal
                        //         `\` before body-end/terminator, line-continuation for `\\<NL>`.
                        //   \c  → delegate to scan_step_command (produces quoted literal `c`).
                        match self.cursor.peek_nth(1) {
                            Some('$') => {
                                // \$ → drop the `\`; `$` becomes the next char for
                                // scan_step_command to process as an expandable dollar.
                                self.cursor.next(); // consume '\'
                                return Ok(Step::Produced);
                            }
                            Some('\\') => {
                                // \\ → consume both backslashes, then re-tokenize the
                                // surviving `\` inline (mirroring scan_step_command's `\`
                                // arm on the NEXT char, but without delegating so the
                                // closing '`' terminator is NOT consumed here).
                                let off = self.cursor.offset();
                                let l   = self.cursor.line();
                                let c   = self.cursor.column();
                                self.cursor.next(); // consume first '\'
                                self.cursor.next(); // consume second '\'
                                match self.cursor.peek().copied() {
                                    None | Some('`') => {
                                        // Lone `\` before body-end or terminator: unquoted literal.
                                        if !self.has_token {
                                            self.token_start      = off;
                                            self.token_start_line = l;
                                            self.token_start_col  = c;
                                        }
                                        self.has_token = true;
                                        self.current.push('\\');
                                    }
                                    Some('\n') => { self.cursor.next(); } // line continuation: drop both
                                    Some(ch) => {
                                        self.cursor.next(); // consume the escaped char
                                        flush_literal(&mut self.parts, &mut self.current, false);
                                        if !self.has_token {
                                            self.token_start      = off;
                                            self.token_start_line = l;
                                            self.token_start_col  = c;
                                        }
                                        self.has_token = true;
                                        self.parts.push(WordPart::Quoted {
                                            style: QuoteStyle::Backslash,
                                            parts: vec![WordPart::Literal { text: ch.to_string(), quoted: true }],
                                        });
                                    }
                                }
                                return Ok(Step::Produced);
                            }
                            _ => {
                                // \c or trailing `\`: let scan_step_command handle it
                                // (produces WordPart::Quoted { Backslash, [Literal(c)] }).
                            }
                        }
                    } else if self.cursor.peek() == Some(&'`') {
                        // A BARE '`' (B = 0) that is NOT a delimiter at this depth.
                        // Only reachable at D ≥ 2, where a close needs B = 2^(D−1)−1 ≥ 1
                        // and an open needs B = 2^D−1 ≥ 3 — a lone '`' matches neither.
                        // Treat it as ORDINARY body content (a literal '`'): NEVER
                        // delegate to scan_step_command's production '`' arm, which
                        // would invoke the fat recursive backtick scanner (wrong under
                        // Mode::Backtick).
                        //
                        // KNOWN DIVERGENCE [deferred, v245, dormant]: this is a LENIENT
                        // ACCEPT.  The recursive production oracle rejects these malformed
                        // inputs at the lex stage (LexError::UnterminatedSubstitution), but
                        // the new path produces Ok.  Well-formed inputs are byte-identical
                        // (see bt_depth2_nesting); the divergence is malformed-input-only.
                        // Pinned by bt_malformed_divergence_deferred — that test must be
                        // updated (or deleted) when Stage-2 live-wiring reconciles this by
                        // making the new path reject these inputs too.
                        let off = self.cursor.offset();
                        let l   = self.cursor.line();
                        let c   = self.cursor.column();
                        self.cursor.next(); // consume the bare '`'
                        if !self.has_token {
                            self.token_start      = off;
                            self.token_start_line = l;
                            self.token_start_col  = c;
                        }
                        self.has_token = true;
                        self.current.push('`');
                        return Ok(Step::Produced);
                    }
                    // \c / trailing `\` (the run is an escape, not a delimiter) —
                    // delegate to Command-mode scanning for the escaped char.  A bare
                    // '`' can no longer reach here: at D=1 it is the close (handled
                    // above), at D≥2 it is body content (handled just above).
                    self.scan_step_command_body()
                }
                _ => {
                    // Normal body character — delegate to Command-mode scanning.
                    // The '`' arm inside scan_step_command cannot fire because we've
                    // already confirmed the next char is neither '`' nor '\'.
                    self.scan_step_command_body()
                }
            }
        }
    }

    /// Emit a backtick delimiter atom (`BeginBacktick` on open, `EndBacktick` on
    /// close) for a `Mode::Backtick` body.  Consumes the `run_len` contiguous
    /// backslashes and the delimiting `` ` ``, flushes any pending word token that
    /// immediately precedes the delimiter, and mutates the top `Backtick` frame's
    /// depth in place (+1 on open, −1 on close).  The LEXER owns depth.
    fn emit_backtick_delim(&mut self, run_len: usize, open: bool) -> Result<(), LexError> {
        // Span at the START of the backslash run (or the '`' when run_len == 0).
        let off = self.cursor.offset();
        let l   = self.cursor.line();
        let c   = self.cursor.column();
        for _ in 0..run_len {
            self.cursor.next(); // consume a run backslash
        }
        self.cursor.next(); // consume the delimiting '`'
        // Flush any pending word token that immediately precedes the delimiter.
        if self.has_token {
            flush_literal(&mut self.parts, &mut self.current, false);
            emit_word_with_braces(
                &mut self.history,
                std::mem::take(&mut self.parts),
                self.brace_expand,
                Span::new(self.token_start, self.token_start_line, self.token_start_col),
            )?;
            self.has_token = false;
        }
        // Mutate depth in the top Backtick frame (single continuous counter).
        if let Some(Mode::Backtick { depth: d }) = self.modes.last_mut() {
            if open { *d += 1; } else { *d -= 1; }
        }
        let kind = if open { TokenKind::BeginBacktick } else { TokenKind::EndBacktick };
        self.history.push(Token::new(kind, Span::new(off, l, c)));
        Ok(())
    }

    /// `Mode::Arith { paren_depth, in_squote, in_dquote, body_started, for_header, delim }` scanner — v246.
    /// Emits `$((` (ArithOpen) on entry, then body atoms, then `))` (ArithClose).
    /// `for_header` (v256) additionally emits `ArithSemi` at a depth-0 `;`.
    /// `delim` (v258) selects `$((`/ArithClose(`))`) vs `$[`/LegacyArithOpen/ArithClose(`]`).
    fn scan_step_arith(&mut self, paren_depth: u32, in_squote: bool, in_dquote: bool, body_started: bool, for_header: bool, delim: ArithDelim) -> Result<Step, LexError> {
        if !body_started {
            let off = self.cursor.offset();
            let l = self.cursor.line();
            let c = self.cursor.column();
            debug_assert_eq!(self.cursor.peek(), Some(&'$'), "scan_step_arith entry: expected `$` of `$((`/`$[`");
            match delim {
                ArithDelim::Paren => {
                    self.cursor.next(); // `$`
                    self.cursor.next(); // `(`
                    self.cursor.next(); // `(`
                    if let Some(Mode::Arith { body_started, .. }) = self.modes.last_mut() { *body_started = true; }
                    self.history.push(Token::new(TokenKind::ArithOpen, Span::new(off, l, c)));
                }
                ArithDelim::Bracket => {
                    self.cursor.next(); // `$`
                    self.cursor.next(); // `[`
                    if let Some(Mode::Arith { body_started, .. }) = self.modes.last_mut() { *body_started = true; }
                    self.history.push(Token::new(TokenKind::LegacyArithOpen, Span::new(off, l, c)));
                }
            }
            return Ok(Step::Produced);
        }

        // Body: accumulate a literal run until a paren event / EOF.  Use a LOCAL
        // `depth` (seeded from the frame's value) and sync it back to the field on
        // every return, so `(a)` handled within ONE call counts correctly.
        let off = self.cursor.offset();
        let l = self.cursor.line();
        let c = self.cursor.column();
        let mut text = String::new();
        let mut depth = paren_depth;
        // Helper: write `depth` back into the top Arith frame before returning.
        macro_rules! sync_depth { () => {
            if let Some(Mode::Arith { paren_depth, .. }) = self.modes.last_mut() { *paren_depth = depth; }
        }; }
        let mut squote = in_squote;
        let mut dquote = in_dquote;
        // Write the current quote-span state back to the top Arith frame. Called
        // on every `'`/`"` toggle so the flag survives a `$`/backtick sub-parse
        // round-trip WITHOUT adding a sync to every `return` site (mirrors how
        // `body_started` is set directly on the frame).
        macro_rules! sync_quotes { () => {
            if let Some(Mode::Arith { in_squote, in_dquote, .. }) = self.modes.last_mut() {
                *in_squote = squote; *in_dquote = dquote;
            }
        }; }
        let (open_char, close_char) = match delim {
            ArithDelim::Paren => ('(', ')'),
            ArithDelim::Bracket => ('[', ']'),
        };
        loop {
            match self.cursor.peek().copied() {
                None => {
                    if squote || dquote {
                        // Unterminated quote span inside the arith body. The oracle
                        // also errors here (scan_arith_body → UnterminatedArith /
                        // scan_legacy_arith_body/push_quoted_span → UnterminatedLegacyArith).
                        // Both paths error, so the input is not byte-comparable
                        // (`old_seq` panics on lex errors) — same non-diff pattern as
                        // prior iterations' unterminated cases.
                        return Err(LexError::UnterminatedArith);
                    }
                    if !text.is_empty() {
                        sync_depth!();
                        self.history.push(Token::new(TokenKind::Lit { text, quoted: true }, Span::new(off, l, c)));
                        return Ok(Step::Produced);
                    }
                    return Err(LexError::UnterminatedArith);
                }
                // Inside a single-quoted span single-quote is DELIM-AWARE, exactly
                // like double-quote: it suppresses `$`/backtick/`\`-escaping (those
                // arms are `!squote`-guarded → fall to the catch-all as literals) but
                // does NOT gate the delimiters. A Paren `(`/`)`/for-header `;` still
                // fires (the oracle `scan_arith_body` is fully quote-blind), while a
                // Bracket `[`/`]` fails the `(!squote && !dquote)` guard and falls to
                // the catch-all as a protected literal (the oracle `scan_legacy_arith_body`
                // tracks quote spans). `'` closes the span; `"` is a literal.
                Some('\'') if squote => {
                    self.cursor.next();
                    squote = false;
                    sync_quotes!();
                }
                Some('"') if squote => {
                    self.cursor.next();
                    text.push('"');
                }
                // Quote openers/closers (not in single-quote here). A `'` inside a
                // double-quote is literal; otherwise it OPENS a single-quote (drop
                // the quote char — bash quote-removal). `"` toggles the double-quote
                // span (drop the quote char). Both are DROPPED, never pushed.
                Some('\'') => {
                    self.cursor.next();
                    if dquote {
                        text.push('\'');
                    } else {
                        squote = true;
                        sync_quotes!();
                    }
                }
                Some('"') => {
                    self.cursor.next();
                    dquote = !dquote;
                    sync_quotes!();
                }
                Some(oc) if oc == open_char && (matches!(delim, ArithDelim::Paren) || (!squote && !dquote)) => {
                    self.cursor.next();
                    text.push(oc);
                    depth += 1;
                }
                Some(cc) if cc == close_char && depth > 0 && (matches!(delim, ArithDelim::Paren) || (!squote && !dquote)) => {
                    self.cursor.next();
                    text.push(cc);
                    depth -= 1;
                }
                Some(cc) if cc == close_char && (matches!(delim, ArithDelim::Paren) || (!squote && !dquote)) => {
                    // depth == 0: flush any pending literal FIRST (emit the
                    // terminator/bail on the NEXT call), else classify now.
                    if !text.is_empty() {
                        sync_depth!();
                        self.history.push(Token::new(TokenKind::Lit { text, quoted: true }, Span::new(off, l, c)));
                        return Ok(Step::Produced);
                    }
                    let poff = self.cursor.offset();
                    let pl = self.cursor.line();
                    let pc = self.cursor.column();
                    match delim {
                        ArithDelim::Paren => {
                            if self.cursor.peek_nth(1) == Some(')') {
                                self.cursor.next(); // first `)`
                                self.cursor.next(); // second `)`
                                self.history.push(Token::new(TokenKind::ArithClose, Span::new(poff, pl, pc)));
                            } else {
                                // NOT a `))` close — the `$( (…) )` wrinkle.  Do NOT
                                // consume; the parser rewinds via ArithBail.
                                self.history.push(Token::new(TokenKind::ArithBail, Span::new(poff, pl, pc)));
                            }
                        }
                        ArithDelim::Bracket => {
                            // `$[ … ]` closes on a single depth-0 `]` (no `]]` check,
                            // no bail — `$[` has no `$( (` wrinkle).
                            self.cursor.next(); // `]`
                            self.history.push(Token::new(TokenKind::ArithClose, Span::new(poff, pl, pc)));
                        }
                    }
                    return Ok(Step::Produced);
                }
                Some('`') if !squote => {
                    if !text.is_empty() {
                        sync_depth!();
                        self.history.push(Token::new(TokenKind::Lit { text, quoted: true }, Span::new(off, l, c)));
                        return Ok(Step::Produced);
                    }
                    let so = self.cursor.offset(); let sl = self.cursor.line(); let sc = self.cursor.column();
                    self.history.push(Token::new(TokenKind::BeginBacktick, Span::new(so, sl, sc)));
                    return Ok(Step::Produced);
                }
                Some('$') if !squote => {
                    // Classify what follows `$` (Task 4 adds the `$((` nested-arith branch).
                    // NOTE: arithmetic contexts always treat embedded expansions as
                    // `quoted: true` (matches the production oracle `arith_string_to_word`,
                    // which hardcodes `true` for every recursive `scan_dollar_expansion`/
                    // backtick call regardless of the outer `$((…))`'s own quoted flag) —
                    // so these arms use a literal `true`, not the mode's `in_dquote` field.
                    let mut probe = self.cursor.clone();
                    probe.next(); // skip `$`
                    match probe.peek().copied() {
                        Some('{') => {
                            if !text.is_empty() {
                                sync_depth!();
                                self.history.push(Token::new(TokenKind::Lit { text, quoted: true }, Span::new(off, l, c)));
                                return Ok(Step::Produced);
                            }
                            let so = self.cursor.offset(); let sl = self.cursor.line(); let sc = self.cursor.column();
                            self.cursor.next(); // `$`
                            self.cursor.next(); // `{`
                            self.history.push(Token::new(TokenKind::ParamOpen { quoted: true }, Span::new(so, sl, sc)));
                            return Ok(Step::Produced);
                        }
                        Some('(') => {
                            // Distinguish `$((` (nested arith) from `$(` (cmdsub) via one
                            // bounded peek. Both are zero-width signals: do NOT consume;
                            // cursor stays at `$`.
                            if !text.is_empty() {
                                sync_depth!();
                                self.history.push(Token::new(TokenKind::Lit { text, quoted: true }, Span::new(off, l, c)));
                                return Ok(Step::Produced);
                            }
                            let so = self.cursor.offset(); let sl = self.cursor.line(); let sc = self.cursor.column();
                            let mut p2 = self.cursor.clone();
                            p2.next(); // `$`
                            p2.next(); // first `(`
                            if p2.peek() == Some(&'(') {
                                // `$((` nested arith — emit a zero-width ArithOpen signal.
                                self.history.push(Token::new(TokenKind::ArithOpen, Span::new(so, sl, sc)));
                            } else {
                                // `$(` cmdsub — emit a zero-width CmdSubOpen signal.
                                self.history.push(Token::new(TokenKind::CmdSubOpen, Span::new(so, sl, sc)));
                            }
                            return Ok(Step::Produced);
                        }
                        Some('[') => {
                            // v258 T2 fix: `$[` nested legacy arith inside an arith
                            // body (e.g. `$[$[1+2]+3]`) — unlike `$((` there's no
                            // ambiguity to disambiguate (no second-char lookahead
                            // needed), so emit the zero-width LegacyArithOpen signal
                            // directly. Do NOT consume; cursor stays at `$` (mirrors
                            // the `$((`/`$(` signals above — the recursive
                            // `parse_legacy_arith_expansion` pushes a fresh
                            // `Mode::Arith{delim:Bracket}` frame whose own
                            // `!body_started` branch consumes the real `$[`).
                            if !text.is_empty() {
                                sync_depth!();
                                self.history.push(Token::new(TokenKind::Lit { text, quoted: true }, Span::new(off, l, c)));
                                return Ok(Step::Produced);
                            }
                            let so = self.cursor.offset(); let sl = self.cursor.line(); let sc = self.cursor.column();
                            self.history.push(Token::new(TokenKind::LegacyArithOpen, Span::new(so, sl, sc)));
                            return Ok(Step::Produced);
                        }
                        Some(nc) if nc.is_ascii_alphabetic() || nc == '_' => {
                            // `$name` variable — consume `$` + name run, emit DollarName.
                            if !text.is_empty() {
                                sync_depth!();
                                self.history.push(Token::new(TokenKind::Lit { text, quoted: true }, Span::new(off, l, c)));
                                return Ok(Step::Produced);
                            }
                            let so = self.cursor.offset(); let sl = self.cursor.line(); let sc = self.cursor.column();
                            self.cursor.next(); // `$`
                            let mut name = String::new();
                            while let Some(ch) = self.cursor.peek().copied() {
                                if ch.is_ascii_alphanumeric() || ch == '_' { name.push(ch); self.cursor.next(); } else { break; }
                            }
                            self.history.push(Token::new(TokenKind::DollarName { name, quoted: true }, Span::new(so, sl, sc)));
                            return Ok(Step::Produced);
                        }
                        Some(sp @ ('?' | '@' | '*' | '#' | '$' | '!' | '-')) => {
                            // Special parameter (`$?`/`$@`/`$*`/`$#`/`$$`/`$!`/`$-`) —
                            // mirrors `scan_step_param_operand`'s dquote `$`-handling
                            // (lexer.rs ~1140) and the oracle `scan_dollar_expansion`
                            // (~3444-3471), which special-cases each of these one
                            // char at a time. `parse_arith_body`'s `DollarName` match
                            // already maps `"@"`→AllArgs{joined:false}, `"*"`→
                            // AllArgs{joined:true}, `"?"`→LastStatus, else→Var — so no
                            // parser change is needed here.
                            if !text.is_empty() {
                                sync_depth!();
                                self.history.push(Token::new(TokenKind::Lit { text, quoted: true }, Span::new(off, l, c)));
                                return Ok(Step::Produced);
                            }
                            let so = self.cursor.offset(); let sl = self.cursor.line(); let sc = self.cursor.column();
                            self.cursor.next(); // `$`
                            self.cursor.next(); // special char
                            self.history.push(Token::new(TokenKind::DollarName { name: sp.to_string(), quoted: true }, Span::new(so, sl, sc)));
                            return Ok(Step::Produced);
                        }
                        Some(d) if d.is_ascii_digit() => {
                            // Positional parameter `$N` — the oracle
                            // (`scan_dollar_expansion` ~3472-3475) consumes exactly
                            // ONE digit (not a run): `$12` is `$1` followed by a
                            // literal `2`, matching bash (only `${10}` reaches the
                            // 10th positional param). `scan_step_param_operand`'s
                            // dquote digit arm (~1145-1149) does the same.
                            if !text.is_empty() {
                                sync_depth!();
                                self.history.push(Token::new(TokenKind::Lit { text, quoted: true }, Span::new(off, l, c)));
                                return Ok(Step::Produced);
                            }
                            let so = self.cursor.offset(); let sl = self.cursor.line(); let sc = self.cursor.column();
                            self.cursor.next(); // `$`
                            let digit = self.cursor.next().unwrap();
                            self.history.push(Token::new(TokenKind::DollarName { name: digit.to_string(), quoted: true }, Span::new(so, sl, sc)));
                            return Ok(Step::Produced);
                        }
                        _ => {
                            // Bare `$` (not `${`/`$(`/`$((`/`$[`/`$name`/special/digit):
                            // the oracle (arith_string_to_word) flushes the pending
                            // literal and pushes `$` as its OWN Literal part. Match that
                            // structure so `$(( 1 $ 2 ))`/`$(( $'x' ))` are byte-identical.
                            if !text.is_empty() {
                                sync_depth!();
                                self.history.push(Token::new(TokenKind::Lit { text, quoted: true }, Span::new(off, l, c)));
                                return Ok(Step::Produced);
                            }
                            let so = self.cursor.offset(); let sl = self.cursor.line(); let sc = self.cursor.column();
                            self.cursor.next(); // `$`
                            self.history.push(Token::new(TokenKind::Lit { text: "$".into(), quoted: true }, Span::new(so, sl, sc)));
                            return Ok(Step::Produced);
                        }
                    }
                }
                Some('\\') if !squote => {
                    if dquote {
                        // Double-quote `\`-escape table (matches arith_string_to_word):
                        // `\` before `" \ $ ` `` drops the backslash and keeps the
                        // metachar; otherwise the `\` is literal and the next char is
                        // reprocessed normally.
                        self.cursor.next(); // consume `\`
                        match self.cursor.peek().copied() {
                            Some(n @ ('"' | '\\' | '$' | '`')) => { self.cursor.next(); text.push(n); }
                            _ => { text.push('\\'); }
                        }
                    } else {
                        match delim {
                            // `$[`: `\` is retained VERBATIM and protects only a
                            // `]`/`[` DELIMITER (consume+push it so it can't close the
                            // bracket). For any OTHER next char, do NOT consume it — the
                            // main loop re-processes it, so `$`/backtick still expand and
                            // plain chars are pushed literally by the catch-all. This
                            // matches the oracle two-pass model: scan_legacy_arith_body
                            // protects only the delimiter in pass 1, then
                            // arith_string_to_word re-expands the retained `\c` in pass 2.
                            ArithDelim::Bracket => {
                                self.cursor.next(); // `\`
                                text.push('\\');
                                // Consume+retain the next char ONLY if it is a delimiter
                                // (`]`/`[`) or a second `\`. The oracle scan_legacy_arith_body
                                // pairs `\` with ANY next char, but for the atom single pass
                                // only these three matter: `]`/`[` must be protected from
                                // closing, and a `\` must be paired-and-consumed so a
                                // following delimiter stays LIVE (`\\]` → oracle keeps `]` a
                                // delimiter; not consuming the 2nd `\` would re-read it as a
                                // fresh escape and wrongly protect the `]`). `$`/`` ` ``/others
                                // are left for the main loop to re-expand (matches pass 2);
                                // `'`/`"` open a span (the pinned two-pass residual).
                                match self.cursor.peek().copied() {
                                    Some(nc @ (']' | '[' | '\\')) => { self.cursor.next(); text.push(nc); }
                                    _ => { /* do NOT consume — main loop re-processes */ }
                                }
                            }
                            // `$((`: `\` is a plain literal (scan_arith_body is
                            // quote/escape-blind; arith_string_to_word keeps it).
                            ArithDelim::Paren => {
                                self.cursor.next();
                                text.push('\\');
                            }
                        }
                    }
                }
                Some(';') if for_header && depth == 0 => {
                    // v256: a top-level `;` in a for-header separates init/cond/step.
                    // Flush any pending literal FIRST (emit the separator on the NEXT
                    // call), exactly like the depth-0 `)` arm above.
                    if !text.is_empty() {
                        sync_depth!();
                        self.history.push(Token::new(TokenKind::Lit { text, quoted: true }, Span::new(off, l, c)));
                        return Ok(Step::Produced);
                    }
                    let so = self.cursor.offset(); let sl = self.cursor.line(); let sc = self.cursor.column();
                    self.cursor.next(); // consume `;`
                    self.history.push(Token::new(TokenKind::ArithSemi, Span::new(so, sl, sc)));
                    return Ok(Step::Produced);
                }
                Some(ch) => {
                    self.cursor.next();
                    text.push(ch);
                }
            }
        }
    }

    /// Exactly ONE iteration of the old scan loop: advance the cursor and append
    /// 0..N tokens to `self.history`. Returns `Produced` while input remains (the
    /// old `continue` / fall-through), or routes to `finish()` at end of input
    /// (the old EOF `break`). `?` errors propagate unchanged.
    fn scan_step_command(&mut self) -> Result<Step, LexError> {
        // When `$glued` (no whitespace between the just-flushed Word and the
        // redirect operator about to be pushed), and that trailing Word is a pure
        // digit-run or `{ident}`, replace it with a `TokenKind::RedirFd` occupying the
        // same span. Must be
        // invoked AFTER the preceding word has been flushed and BEFORE the operator
        // token is pushed.
        macro_rules! take_fd_prefix {
            ($glued:expr) => {{
                if $glued {
                    if let Some(fd) = fd_prefix_of(self.history.last().map(|t| &t.kind)) {
                        // Replace the digit/`{n}` word with a RedirFd at the SAME span.
                        let span = self.history.pop().expect("fd-prefix word present").span;
                        self.history.push(Token::new(TokenKind::RedirFd(fd), span));
                    }
                }
            }};
        }
        // `=~` regex operand inside `[[ … ]]`: once `self.expect_regex` is armed and
        // the next char is the operand's first (non-whitespace) char, scan the
        // whole operand as one literal regex Word. Whitespace between `=~` and
        // the operand falls through to the normal loop (which skips it and keeps
        // `self.expect_regex` set). Branching before `self.cursor.next()` keeps the emitted
        // offset exactly at the operand's first byte.
        if self.expect_regex {
            if let Some(&ch) = self.cursor.peek() {
                if ch.is_whitespace() {
                    // skip leading whitespace via the normal path below
                } else {
                    self.expect_regex = false;
                    // The operand's first byte. Push the Word directly (NOT via
                    // emit_word_with_braces) so no brace expansion applies.
                    let operand_start = self.cursor.offset();
                    let operand_line = self.cursor.line();
                    let operand_col = self.cursor.column();
                    let operand_parts = scan_regex_operand(&mut self.cursor, self.opts)?;
                    self.history.push(Token::new(TokenKind::Word(Word(operand_parts)), Span::new(operand_start, operand_line, operand_col)));
                    self.has_token = false;
                    return Ok(Step::Produced);
                }
            } else {
                return self.finish();
            }
        }
        let c_off = self.cursor.offset();
        let c_line = self.cursor.line();
        let c_col = self.cursor.column();
        let c = match self.cursor.next() {
            Some(c) => c,
            None => return self.finish(),
        };
        if c.is_whitespace() {
            if self.has_token {
                flush_literal(&mut self.parts, &mut self.current, false);
                debug_assert!(
                    !self.parts.is_empty(),
                    "lexer invariant: has_token was true but no parts were emitted"
                );
                let kw = single_unquoted_literal(&self.parts).map(str::to_owned);
                emit_word_with_braces(&mut self.history, std::mem::take(&mut self.parts), self.brace_expand, Span::new(self.token_start, self.token_start_line, self.token_start_col))?;
                match kw.as_deref() {
                    Some("[[") => self.dbracket_depth += 1,
                    Some("]]") => self.dbracket_depth = self.dbracket_depth.saturating_sub(1),
                    Some("=~") if self.dbracket_depth > 0 => self.expect_regex = true,
                    _ => {}
                }
                self.has_token = false;
                self.in_assignment_value = false;
            }
            if c == '\n' {
                // If there are pending heredocs, collect their bodies now
                // before emitting the Newline token.
                if !self.pending_heredocs.is_empty() {
                    collect_heredoc_bodies(&mut self.cursor, &mut self.pending_heredocs, &mut self.history, self.opts, &mut self.parsed_heredoc_bodies, self.command_atoms)?;
                }
                self.history.push(Token::new(TokenKind::Newline, Span::new(c_off, c_line, c_col)));
            }
            return Ok(Step::Produced);
        }

        // Record the start byte offset of a word as soon as its first char is
        // seen. When `self.has_token` is false at the top of an iteration, this char
        // is a candidate first char; operator arms (which leave `self.has_token`
        // false) simply overwrite `self.token_start` on the next iteration, while
        // word arms read the value captured at the word's true first char.
        if !self.has_token {
            self.token_start = c_off;
            self.token_start_line = c_line;
            self.token_start_col = c_col;
        }

        // extglob (`shopt -s extglob`): one of `? * + @ !` directly followed
        // by `(` introduces a balanced parenthesised group (`+(a|b)`), lexed
        // as a single literal word part. Checked before the normal
        // `?`/`*`/`(` handling so the group is recognized first. With extglob
        // off, this branch never fires and lexing is byte-identical.
        if self.opts.extglob && matches!(c, '?' | '*' | '+' | '@' | '!') && self.cursor.peek() == Some(&'(') {
            self.has_token = true;
            flush_literal(&mut self.parts, &mut self.current, false);
            let group_parts = scan_extglob_group(c, &mut self.cursor, self.opts)?;
            self.parts.extend(group_parts);
            return Ok(Step::Produced);
        }

        match c {
            '\'' => {
                self.has_token = true;
                flush_literal(&mut self.parts, &mut self.current, false);
                let mut run: Vec<WordPart> = Vec::new();
                let mut buf = String::new();
                loop {
                    match self.cursor.next() {
                        Some('\'') => break,
                        Some(ch) => buf.push(ch),
                        None => return Err(LexError::UnterminatedQuote),
                    }
                }
                // empty '' still yields one empty quoted Literal (empty-token contract)
                run.push(WordPart::Literal { text: buf, quoted: true });
                self.parts.push(WordPart::Quoted { style: QuoteStyle::Single, parts: run });
            }
            '"' => {
                self.has_token = true;
                flush_literal(&mut self.parts, &mut self.current, false);
                let mut run: Vec<WordPart> = Vec::new();
                let mut qbuf = String::new();
                loop {
                    match self.cursor.next() {
                        Some('"') => break,
                        Some('\\') => match self.cursor.next() {
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
                            scan_dollar_expansion(&mut self.cursor, &mut run, true, self.opts)?;
                        }
                        Some('`') => {
                            // Backtick substitution inside double quotes (quoted: true).
                            flush_literal(&mut run, &mut qbuf, true);
                            let sequence = scan_backtick_substitution(&mut self.cursor, self.opts)?;
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
                self.parts.push(WordPart::Quoted { style: QuoteStyle::Double, parts: run });
            }
            '\\' => match self.cursor.next() {
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
                    self.has_token = true;
                    flush_literal(&mut self.parts, &mut self.current, false);
                    self.parts.push(WordPart::Quoted {
                        style: QuoteStyle::Backslash,
                        parts: vec![WordPart::Literal { text: ch.to_string(), quoted: true }],
                    });
                }
                None => {
                    self.has_token = true;
                    self.current.push('\\');
                }
            },
            '$' => {
                // Expansion outside any quotes (quoted: false).
                self.has_token = true;
                flush_literal(&mut self.parts, &mut self.current, false);
                scan_dollar_expansion(&mut self.cursor, &mut self.parts, false, self.opts)?;
            }
            '#' if !self.has_token => {
                // POSIX: an unquoted `#` that begins a word starts a comment to
                // end-of-line. `#` mid-word (self.has_token) falls through as literal.
                skip_line_comment(&mut self.cursor);
            }
            '~' if !self.has_token || tilde_eligible_in_assignment(self.in_assignment_value, &self.current) => {
                if let Some(spec) = try_parse_tilde(&mut self.cursor, self.in_assignment_value, false) {
                    flush_literal(&mut self.parts, &mut self.current, false);
                    self.has_token = true;
                    self.parts.push(WordPart::Tilde(spec));
                } else {
                    // Fall through: treat '~' as literal.
                    self.current.push('~');
                    self.has_token = true;
                }
            }
            '`' => {
                self.has_token = true;
                flush_literal(&mut self.parts, &mut self.current, false);
                let sequence = scan_backtick_substitution(&mut self.cursor, self.opts)?;
                self.parts.push(WordPart::CommandSub { sequence, quoted: false });
            }
            '|' => {
                if self.has_token {
                    flush_literal(&mut self.parts, &mut self.current, false);
                    emit_word_with_braces(&mut self.history, std::mem::take(&mut self.parts), self.brace_expand, Span::new(self.token_start, self.token_start_line, self.token_start_col))?;
                    self.has_token = false;
                }
                if self.cursor.peek() == Some(&'|') {
                    self.cursor.next();
                    self.history.push(Token::new(TokenKind::Op(Operator::Or), Span::new(c_off, c_line, c_col)));
                } else if self.cursor.peek() == Some(&'&') {
                    // `|&` is bash shorthand for `2>&1 |`: merge the left command's
                    // stderr into the pipe, then pipe. Desugar at the token level so
                    // the existing pipeline/redirect machinery (incl. v176
                    // compound-stage redirects) handles it unchanged.
                    self.cursor.next(); // consume the '&' of `|&`
                    self.history.push(Token::new(TokenKind::RedirFd(crate::command::RedirFd::Number(2)), Span::new(c_off, c_line, c_col)));
                    self.history.push(Token::new(TokenKind::Op(Operator::DupOut), Span::new(c_off, c_line, c_col)));
                    self.history.push(Token::new(TokenKind::Word(Word(vec![WordPart::Literal {
                        text: "1".to_string(),
                        quoted: false,
                    }])), Span::new(c_off, c_line, c_col)));
                    self.history.push(Token::new(TokenKind::Op(Operator::Pipe), Span::new(c_off, c_line, c_col)));
                } else {
                    self.history.push(Token::new(TokenKind::Op(Operator::Pipe), Span::new(c_off, c_line, c_col)));
                }
                self.in_assignment_value = false;
            }
            '&' => {
                if self.has_token {
                    flush_literal(&mut self.parts, &mut self.current, false);
                    emit_word_with_braces(&mut self.history, std::mem::take(&mut self.parts), self.brace_expand, Span::new(self.token_start, self.token_start_line, self.token_start_col))?;
                    self.has_token = false;
                }
                if self.cursor.peek() == Some(&'&') {
                    self.cursor.next();
                    self.history.push(Token::from(TokenKind::Op(Operator::And)));
                } else if self.cursor.peek() == Some(&'>') {
                    self.cursor.next();
                    if self.cursor.peek() == Some(&'>') {
                        self.cursor.next();
                        self.history.push(Token::from(TokenKind::Op(Operator::AndRedirAppend)));
                    } else {
                        self.history.push(Token::from(TokenKind::Op(Operator::AndRedirOut)));
                    }
                } else {
                    self.history.push(Token::from(TokenKind::Op(Operator::Background)));
                }
                if let Some(t) = self.history.last_mut() { t.span = Span::new(c_off, c_line, c_col); }
                self.in_assignment_value = false;
            }
            ';' => {
                if self.has_token {
                    flush_literal(&mut self.parts, &mut self.current, false);
                    emit_word_with_braces(&mut self.history, std::mem::take(&mut self.parts), self.brace_expand, Span::new(self.token_start, self.token_start_line, self.token_start_col))?;
                    self.has_token = false;
                }
                let op = if self.cursor.peek() == Some(&';') {
                    self.cursor.next();
                    if self.cursor.peek() == Some(&'&') {
                        self.cursor.next();
                        Operator::DoubleSemiAmp
                    } else {
                        Operator::DoubleSemi
                    }
                } else if self.cursor.peek() == Some(&'&') {
                    self.cursor.next();
                    Operator::SemiAmp
                } else {
                    Operator::Semi
                };
                self.history.push(Token::new(TokenKind::Op(op), Span::new(c_off, c_line, c_col)));
                self.in_assignment_value = false;
            }
            '(' => {
                if self.has_token {
                    flush_literal(&mut self.parts, &mut self.current, false);
                    emit_word_with_braces(&mut self.history, std::mem::take(&mut self.parts), self.brace_expand, Span::new(self.token_start, self.token_start_line, self.token_start_col))?;
                    self.has_token = false;
                }
                // Detect `((` (contiguous, no whitespace). The peek/next
                // sequence below consumes the second `(` only when present.
                // Whitespace between the two `(` is already consumed by the
                // outer loop's whitespace handling — so by the time we get
                // here, a second `(` means they were truly adjacent.
                if self.cursor.peek() == Some(&'(') {
                    // `((` is an arithmetic command ONLY if a matching `))` is
                    // found; otherwise bash treats it as nested subshells `( (`.
                    // Save the cursor at the second `(`, try the arith block, and
                    // on failure rewind + emit a single LParen (the first `(`); the
                    // second `(` then re-lexes as another LParen. A `((` that DOES
                    // close as `))` but isn't valid arithmetic stays an ArithBlock
                    // → arith error at parse/eval, matching bash. Mirrors the v177
                    // `$((` disambiguation.
                    let saved = self.cursor.clone();
                    self.cursor.next(); // consume the second `(`
                    match scan_arith_block(&mut self.cursor) {
                        Ok(body) => self.history.push(Token::from(TokenKind::ArithBlock(body, self.opts))),
                        Err(_) => {
                            self.cursor = saved;
                            self.history.push(Token::from(TokenKind::Op(Operator::LParen)));
                        }
                    }
                } else {
                    self.history.push(Token::from(TokenKind::Op(Operator::LParen)));
                }
                if let Some(t) = self.history.last_mut() { t.span = Span::new(c_off, c_line, c_col); }
                self.in_assignment_value = false;
            }
            ')' => {
                if self.has_token {
                    flush_literal(&mut self.parts, &mut self.current, false);
                    emit_word_with_braces(&mut self.history, std::mem::take(&mut self.parts), self.brace_expand, Span::new(self.token_start, self.token_start_line, self.token_start_col))?;
                    self.has_token = false;
                }
                self.history.push(Token::new(TokenKind::Op(Operator::RParen), Span::new(c_off, c_line, c_col)));
                self.in_assignment_value = false;
            }
            '<' => {
                // `glued` = a Word was being accumulated with no intervening
                // whitespace before this operator. Captured before the flush.
                let glued = self.has_token;
                if self.has_token {
                    flush_literal(&mut self.parts, &mut self.current, false);
                    emit_word_with_braces(&mut self.history, std::mem::take(&mut self.parts), self.brace_expand, Span::new(self.token_start, self.token_start_line, self.token_start_col))?;
                    self.has_token = false;
                }
                if self.cursor.peek() == Some(&'<') {
                    self.cursor.next(); // consume second '<'
                    if self.cursor.peek() == Some(&'<') {
                        self.cursor.next(); // consume third '<' — here-string
                        take_fd_prefix!(glued);
                        self.history.push(Token::from(TokenKind::Op(Operator::HereString)));
                    } else {
                        let strip_tabs = if self.cursor.peek() == Some(&'-') {
                            self.cursor.next(); // consume '-'
                            true
                        } else {
                            false
                        };
                        // Parse the delimiter word and detect literal vs expanding mode.
                        let (delim, expand) = parse_heredoc_delim(&mut self.cursor)?;
                        // A glued fd-prefix (`3<<EOF`) becomes a RedirFd token
                        // before the heredoc placeholder.
                        take_fd_prefix!(glued);
                        // Push a placeholder TokenKind::Heredoc with empty body.
                        // The body is back-patched after the line's \n.
                        let placeholder_idx = self.history.len();
                        self.history.push(Token::from(TokenKind::Heredoc {
                            body: Word(Vec::new()),
                            expand,
                            strip_tabs,
                        }));
                        self.pending_heredocs.push_back(PendingHeredoc {
                            delim,
                            expand,
                            strip_tabs,
                            token_idx: placeholder_idx,
                            reg_depth: self.modes.len(), // unused by the oracle queue
                        });
                    }
                    if let Some(t) = self.history.last_mut() { t.span = Span::new(c_off, c_line, c_col); }
                    self.in_assignment_value = false;
                } else if self.cursor.peek() == Some(&'(') {
                    // `<(cmd)` — process substitution. Consume the `(` and scan the
                    // inner command body exactly like `$(…)`. The result is a word
                    // part on the CURRENT word (not a standalone redirect operator).
                    self.cursor.next(); // consume '('
                    let sequence = scan_paren_substitution(&mut self.cursor, self.opts)?;
                    if !self.has_token {
                        self.token_start = c_off;
                        self.token_start_line = c_line;
                    }
                    self.has_token = true;
                    self.parts.push(WordPart::ProcessSub { sequence, dir: ProcDir::In });
                    self.in_assignment_value = false;
                } else if self.cursor.peek() == Some(&'&') {
                    self.cursor.next();
                    take_fd_prefix!(glued);
                    self.history.push(Token::new(TokenKind::Op(Operator::DupIn), Span::new(c_off, c_line, c_col)));
                    self.in_assignment_value = false;
                } else if self.cursor.peek() == Some(&'>') {
                    self.cursor.next();
                    take_fd_prefix!(glued);
                    self.history.push(Token::new(TokenKind::Op(Operator::RedirReadWrite), Span::new(c_off, c_line, c_col)));
                    self.in_assignment_value = false;
                } else {
                    take_fd_prefix!(glued);
                    self.history.push(Token::new(TokenKind::Op(Operator::RedirIn), Span::new(c_off, c_line, c_col)));
                    self.in_assignment_value = false;
                }
            }
            '>' => {
                let glued = self.has_token;
                if self.has_token {
                    flush_literal(&mut self.parts, &mut self.current, false);
                    emit_word_with_braces(&mut self.history, std::mem::take(&mut self.parts), self.brace_expand, Span::new(self.token_start, self.token_start_line, self.token_start_col))?;
                    self.has_token = false;
                }
                if self.cursor.peek() == Some(&'>') {
                    self.cursor.next();
                    take_fd_prefix!(glued);
                    self.history.push(Token::new(TokenKind::Op(Operator::RedirAppend), Span::new(c_off, c_line, c_col)));
                    self.in_assignment_value = false;
                } else if self.cursor.peek() == Some(&'&') {
                    self.cursor.next();
                    take_fd_prefix!(glued);
                    self.history.push(Token::new(TokenKind::Op(Operator::DupOut), Span::new(c_off, c_line, c_col)));
                    self.in_assignment_value = false;
                } else if self.cursor.peek() == Some(&'|') {
                    self.cursor.next();
                    take_fd_prefix!(glued);
                    self.history.push(Token::new(TokenKind::Op(Operator::RedirClobber), Span::new(c_off, c_line, c_col)));
                    self.in_assignment_value = false;
                } else if self.cursor.peek() == Some(&'(') {
                    // `>(cmd)` — process substitution. Consume the `(` and scan the
                    // inner command body exactly like `$(…)`. The result is a word
                    // part on the CURRENT word (not a standalone redirect operator).
                    self.cursor.next(); // consume '('
                    let sequence = scan_paren_substitution(&mut self.cursor, self.opts)?;
                    if !self.has_token {
                        self.token_start = c_off;
                        self.token_start_line = c_line;
                    }
                    self.has_token = true;
                    self.parts.push(WordPart::ProcessSub { sequence, dir: ProcDir::Out });
                    self.in_assignment_value = false;
                } else {
                    take_fd_prefix!(glued);
                    self.history.push(Token::new(TokenKind::Op(Operator::RedirOut), Span::new(c_off, c_line, c_col)));
                    self.in_assignment_value = false;
                }
            }
            '1' if !self.has_token && self.cursor.peek() == Some(&'>') => {
                self.cursor.next();
                if self.cursor.peek() == Some(&'>') {
                    self.cursor.next();
                    self.history.push(Token::from(TokenKind::Op(Operator::RedirAppend)));
                } else if self.cursor.peek() == Some(&'&') {
                    self.cursor.next();
                    self.history.push(Token::from(TokenKind::Op(Operator::DupOut)));
                } else if self.cursor.peek() == Some(&'|') {
                    self.cursor.next();
                    self.history.push(Token::from(TokenKind::Op(Operator::RedirClobber)));
                } else {
                    self.history.push(Token::from(TokenKind::Op(Operator::RedirOut)));
                }
                if let Some(t) = self.history.last_mut() { t.span = Span::new(c_off, c_line, c_col); }
                self.in_assignment_value = false;
            }
            '2' if !self.has_token && self.cursor.peek() == Some(&'>') => {
                self.cursor.next();
                if self.cursor.peek() == Some(&'>') {
                    self.cursor.next();
                    self.history.push(Token::from(TokenKind::Op(Operator::RedirErrAppend)));
                } else if self.cursor.peek() == Some(&'&') {
                    self.cursor.next();
                    self.history.push(Token::from(TokenKind::Op(Operator::DupErr)));
                } else if self.cursor.peek() == Some(&'|') {
                    self.cursor.next();
                    self.history.push(Token::from(TokenKind::Op(Operator::RedirErrClobber)));
                } else {
                    self.history.push(Token::from(TokenKind::Op(Operator::RedirErr)));
                }
                if let Some(t) = self.history.last_mut() { t.span = Span::new(c_off, c_line, c_col); }
                self.in_assignment_value = false;
            }
            '=' if !self.in_assignment_value && word_is_identifier_so_far(&self.current, &self.parts) => {
                self.in_assignment_value = true;
                self.has_token = true;
                self.current.push('=');
                // Compound RHS: `name=(...)`. Scan the array literal as
                // a single WordPart that becomes the value.
                // A `\<NL>` line continuation may sit between `=` and the array
                // `(` (`arr=\<NL>(…)`); bash deletes it pre-tokenization.
                skip_line_continuations(&mut self.cursor);
                if self.cursor.peek() == Some(&'(') {
                    self.cursor.next(); // consume '('
                    flush_literal(&mut self.parts, &mut self.current, false);
                    let elements = scan_array_literal(&mut self.cursor, self.opts)?;
                    self.parts.push(WordPart::ArrayLiteral(elements));
                }
            }
            // `+=`: scalar-or-array append assignment when the prefix is
            // identifier-shaped. Emits an AssignPrefix(Bare, append=true)
            // prefix Word.
            '+' if !self.in_assignment_value
                && self.cursor.peek() == Some(&'=')
                && word_is_identifier_so_far(&self.current, &self.parts) =>
            {
                self.cursor.next(); // consume '='
                self.in_assignment_value = true;
                self.has_token = true;
                // Bake the accumulated identifier text into the target.
                let name = std::mem::take(&mut self.current);
                debug_assert!(
                    self.parts.is_empty(),
                    "word_is_identifier_so_far guarantees no prior parts"
                );
                self.parts.push(WordPart::AssignPrefix {
                    target: crate::command::AssignTarget::Bare(name),
                    append: true,
                });
                // Compound RHS: `name+=(...)`.
                skip_line_continuations(&mut self.cursor);
                if self.cursor.peek() == Some(&'(') {
                    self.cursor.next();
                    let elements = scan_array_literal(&mut self.cursor, self.opts)?;
                    self.parts.push(WordPart::ArrayLiteral(elements));
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
            '[' if !self.in_assignment_value && word_is_identifier_so_far(&self.current, &self.parts) => {
                let mut raw_subscript = String::new();
                let mut depth: usize = 1;
                let mut closed_subscript = false;
                while let Some(&c) = self.cursor.peek() {
                    if c == '[' {
                        depth += 1;
                        raw_subscript.push(c);
                        self.cursor.next();
                    } else if c == ']' {
                        self.cursor.next();
                        depth -= 1;
                        if depth == 0 {
                            closed_subscript = true;
                            break;
                        }
                        raw_subscript.push(c);
                    } else {
                        raw_subscript.push(c);
                        self.cursor.next();
                    }
                }
                // Decide: is this an assignment? Peek for `=` or `+=`.
                let assign_op: Option<bool> = if closed_subscript {
                    match self.cursor.peek().copied() {
                        Some('=') => {
                            self.cursor.next();
                            Some(false)
                        }
                        Some('+') => {
                            // Need to peek two chars; clone iter for lookahead.
                            let mut peeker = self.cursor.clone();
                            peeker.next();
                            if peeker.peek() == Some(&'=') {
                                self.cursor.next(); // consume '+'
                                self.cursor.next(); // consume '='
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
                        let name = std::mem::take(&mut self.current);
                        debug_assert!(
                            self.parts.is_empty(),
                            "word_is_identifier_so_far guarantees no prior parts"
                        );
                        let subscript = parse_subscript_body(&raw_subscript, self.opts)?;
                        self.in_assignment_value = true;
                        self.has_token = true;
                        self.parts.push(WordPart::AssignPrefix {
                            target: crate::command::AssignTarget::Indexed { name, subscript },
                            append,
                        });
                        // Compound RHS: `name[i]=(...)`.
                        if self.cursor.peek() == Some(&'(') {
                            self.cursor.next();
                            let elements = scan_array_literal(&mut self.cursor, self.opts)?;
                            self.parts.push(WordPart::ArrayLiteral(elements));
                        }
                    }
                    None => {
                        // Not an indexed assignment. Fall back: append
                        // the `[`, the scanned subscript text, and the
                        // closing `]` (if any) back into the current
                        // literal so the word behaves the same as
                        // before this arm existed.
                        self.has_token = true;
                        self.current.push('[');
                        self.current.push_str(&raw_subscript);
                        if closed_subscript {
                            self.current.push(']');
                        }
                    }
                }
            }
            other => {
                self.has_token = true;
                self.current.push(other);
            }
        }
        // Fell off the bottom of the old loop body — there is more input;
        // signal progress so next_token() calls scan_step again.
        Ok(Step::Produced)
    }

    /// v247 atom-emitting Command scanner (dormant). Built up across T2–T6:
    /// word-atoms + `Blank` splitting (T2), command-position expansions (T3),
    /// assignments (T4), redirects/operators/comments (T5), compounds (T6).
    /// Atom-native: at `$(`/`${`/`` ` ``/`$((` it emits the opener SIGNAL and the
    /// parser pushes the sub-mode — it never calls the fat scanners.
    fn scan_step_command_atoms(&mut self) -> Result<Step, LexError> {
        // v250: while emitting heredoc bodies, the lexer drives body-atom output
        // from its own state (never the parser). Only in plain Command scanning;
        // a pushed sub-mode (CommandSub/Arith/etc., used by expanding bodies in
        // T4) is handled by scan_step's mode dispatch before we get here.
        if self.emitting_heredoc.is_some() {
            return self.scan_step_heredoc_body();
        }
        self.scan_step_command_atoms_core()
    }

    /// The guard-free core of `scan_step_command_atoms`: emits one Command-mode
    /// atom from the cursor. Split out (v264) so nested cmdsub/backtick body
    /// delegation (`scan_step_command_body`) can reuse the SAME atom scanning
    /// WITHOUT the heredoc-body emission short-circuit — a body embedded in an
    /// expanding heredoc must scan its own `$( … )` tokens, not re-enter the
    /// heredoc body.
    fn scan_step_command_atoms_core(&mut self) -> Result<Step, LexError> {
        // Skip a run of inter-word blanks → emit one Blank boundary token. The
        // oracle flushes the word on ANY `char::is_whitespace()` (lexer.rs:2076),
        // handling only `\n` specially (a `Newline` token); every other
        // whitespace char is a mere word boundary. So the atom `Blank` covers all
        // whitespace EXCEPT `\n`, which is its own `TokenKind::Newline` below.
        if matches!(self.cursor.peek(), Some(w) if w.is_whitespace() && *w != '\n') {
            let off = self.cursor.offset(); let l = self.cursor.line(); let c = self.cursor.column();
            while matches!(self.cursor.peek(), Some(w) if w.is_whitespace() && *w != '\n') { self.cursor.next(); }
            self.history.push(Token::new(TokenKind::Blank, Span::new(off, l, c)));
            self.boundary_reset();
            return Ok(Step::Produced);
        }
        let off = self.cursor.offset(); let l = self.cursor.line(); let c = self.cursor.column();
        match self.cursor.peek().copied() {
            None => self.finish(),

            // Newline — its own boundary token (mirrors the oracle's `c == '\n'`
            // arm). Heredoc bodies are DEFERRED (v247): no `collect_heredoc_bodies`.
            Some('\n') => {
                self.cursor.next();
                self.history.push(Token::new(TokenKind::Newline, Span::new(off, l, c)));
                self.boundary_reset();
                // v250: pending heredoc bodies are emitted as atoms after this
                // newline. Flip on the lexer-internal emission state; the next
                // scan_step calls emit the body groups (see the top-of-fn check).
                //
                // v264: do NOT (re-)trigger while a heredoc body is ALREADY being
                // emitted. This core is reached — via `scan_step_command_body` —
                // for a nested cmdsub/backtick body; when that body is embedded in
                // an EXPANDING heredoc body (`emitting_heredoc.is_some()`), the
                // current heredoc still sits at the front of
                // `atom_pending_heredocs`, so a multi-line `$( … )`'s internal
                // newline would falsely re-trigger emission of the SAME heredoc.
                // (A heredoc registered WITHIN a cmdsub body — `$(sh <<B …)` — is
                // NOT yet emitting, so its newline still triggers correctly.)
                if self.emitting_heredoc.is_none()
                    && self.atom_heredoc_idx_at_depth(self.modes.len()).is_some()
                {
                    self.emitting_heredoc = Some(HeredocEmit { began: false, at_line_start: true, trigger_depth: self.modes.len() });
                    self.heredoc_gen += 1; // v250 T6: emitting_heredoc changed (newline trigger)
                }
                Ok(Step::Produced)
            }

            // Comment: an unquoted `#` that BEGINS a word (mirrors the oracle's
            // `'#' if !self.has_token`, i.e. at a word boundary) runs to end of
            // line. `#` mid-word is literal — handled by the word-run arm, which
            // does not treat `#` as a stop char. No token is emitted.
            Some('#') if self.cmd_at_word_start => {
                skip_line_comment(&mut self.cursor);
                Ok(Step::Produced)
            }

            // Operators / separators / redirects — emit the SAME structural token
            // the oracle emits (lexer.rs ~2245-2509). fd-prefixes (`3>`, `{fd}<`)
            // are handled in the word-run arm (which emits `RedirFd` in place of
            // the digit/`{ident}` `Lit`), so these arms never look back.
            Some('|') | Some('&') | Some(';') | Some('(') | Some(')')
            | Some('<') | Some('>') => self.scan_command_operator_atom(off, l, c),

            // Quoting + literal word text — one atom per call (`Lit` for a
            // maximal unquoted run, `QuoteRun` for one complete quoted run),
            // stopping at a blank / EOF / operator / metachar without consuming it.
            Some(_) => self.scan_command_word_atom(false),
        }
    }

    /// v264: index of the FIRST pending atom heredoc registered at mode-stack
    /// depth `depth`. Shallower entries (outer-line heredocs) may sit ahead of it
    /// in the FIFO; they are skipped so a cmdsub/backtick-body heredoc emits at
    /// its own body's newline. Within a depth, FIFO order is preserved.
    fn atom_heredoc_idx_at_depth(&self, depth: usize) -> Option<usize> {
        self.atom_pending_heredocs.iter().position(|ph| ph.reg_depth == depth)
    }

    /// v250: emit atoms for the current `atom_pending_heredocs` body (the first
    /// entry matching `emitting_heredoc.trigger_depth`). One `scan_step` call:
    /// first emits `HeredocBodyBegin`, next emits the body + `HeredocBodyEnd` and
    /// removes the entry; when no same-depth entry remains, clears
    /// `emitting_heredoc`. Task 2 handles LITERAL bodies (one raw `Lit`); Task 4
    /// extends this for expanding bodies. Detects the close-delimiter line itself.
    fn scan_step_heredoc_body(&mut self) -> Result<Step, LexError> {
        let depth = match self.emitting_heredoc.as_ref() {
            Some(s) => s.trigger_depth,
            None => return self.scan_step_command_atoms(), // no-op guard
        };
        let idx = self.atom_heredoc_idx_at_depth(depth).expect("emitting implies a pending entry at this depth");
        let ph = self.atom_pending_heredocs[idx].clone();
        // Emit the Begin bracket for the current heredoc (carries `expand` so the
        // parser picks the literal vs expanding assembly).
        if !self.emitting_heredoc.as_ref().expect("emitting").began {
            self.emitting_heredoc.as_mut().expect("emitting").began = true;
            self.heredoc_gen += 1; // v250 T6 fix: emitting_heredoc.began flip is a state change
            let (off, l, c) = (self.cursor.offset(), self.cursor.line(), self.cursor.column());
            self.history.push(Token::new(TokenKind::HeredocBodyBegin { expand: ph.expand }, Span::new(off, l, c)));
            return Ok(Step::Produced);
        }
        if ph.expand {
            self.scan_step_heredoc_body_expanding(&ph)
        } else {
            self.scan_step_heredoc_body_literal(&ph)
        }
    }

    /// v250 T2/T3: LITERAL heredoc body — accumulate every content line verbatim
    /// (with per-line `\n`) into ONE `Lit{quoted:true}` atom, emitted with the
    /// closing `HeredocBodyEnd` when the close-delimiter line is reached. The
    /// PARSER (`push_heredoc_literal_lines`) splits that merged text back into the
    /// oracle's per-line `(content, "\n")` `Literal` pairs. No expansions.
    fn scan_step_heredoc_body_literal(&mut self, ph: &PendingHeredoc) -> Result<Step, LexError> {
        let mut body = String::new();
        loop {
            let mut line = String::new();
            let mut got_nl = false;
            while let Some(ch) = self.cursor.next() {
                if ch == '\n' { got_nl = true; break; }
                line.push(ch);
            }
            let check = if ph.strip_tabs { line.trim_start_matches('\t') } else { &line[..] };
            if check == ph.delim {
                // Close delimiter reached — emit the accumulated Lit + End, pop.
                let (off, l, c) = (self.cursor.offset(), self.cursor.line(), self.cursor.column());
                if !body.is_empty() {
                    self.history.push(Token::new(TokenKind::Lit { text: body, quoted: true }, Span::new(off, l, c)));
                }
                self.emit_heredoc_body_end();
                return Ok(Step::Produced);
            }
            if !got_nl {
                return Err(LexError::UnterminatedHeredoc);
            }
            let body_line = if ph.strip_tabs { line.trim_start_matches('\t').to_string() } else { line };
            body.push_str(&body_line);
            body.push('\n');
        }
    }

    /// v250 T4: EXPANDING heredoc body — emit ONE body-part atom per call,
    /// cursor-driven, so the PARSER can push a sub-mode (CommandSub/Arith/Backtick/
    /// ParamExpansion) that scans the nested structure FROM THE CURSOR (the
    /// expansion openers are zero-width signals). Mirrors `scan_expanding_body_line`
    /// (the oracle) EXACTLY: heredoc backslash rules (`\` special only before
    /// `$`/`` ` ``/`\`), `\<NL>` line continuation, `"`/`'` literal in the body,
    /// literal runs are `quoted:false` while escaped chars + the per-line `"\n"`
    /// separator are `quoted:true`. Detects the close-delimiter line at each
    /// LOGICAL line start (a bounded peek of the current physical/continued line,
    /// never a scan for a matching delimiter across the whole body).
    fn scan_step_heredoc_body_expanding(&mut self, ph: &PendingHeredoc) -> Result<Step, LexError> {
        // At a logical line start: strip `<<-` leading tabs, then the delimiter check.
        if self.emitting_heredoc.as_ref().expect("emitting").at_line_start {
            if ph.strip_tabs {
                while self.cursor.peek() == Some(&'\t') { self.cursor.next(); }
            }
            if let Some(consume) = self.heredoc_at_delim_line(ph) {
                // Consume EXACTLY the physical span `heredoc_at_delim_line` read on
                // its probe (the full continuation-joined logical line — every joined
                // physical line + the final newline). A delimiter formed across a
                // `\<NL>` continuation spans multiple physical lines, so consuming a
                // single physical line would leak the remainder as a spurious command
                // (mirrors the oracle `collect_one_heredoc_body`, which advances its
                // real cursor by the whole joined line before returning).
                for _ in 0..consume {
                    self.cursor.next();
                }
                self.emit_heredoc_body_end();
                return Ok(Step::Produced);
            }
            self.emitting_heredoc.as_mut().expect("emitting").at_line_start = false;
            self.heredoc_gen += 1; // v250 T6 fix: emitting_heredoc.at_line_start flip is a state change
            // Fall through to emit the first atom of this body line.
        }
        let off = self.cursor.offset();
        let l = self.cursor.line();
        let c = self.cursor.column();
        match self.cursor.peek().copied() {
            // EOF mid-body without a matching close-delimiter line — error (matches
            // the oracle's `!got_newline` guard).
            None => Err(LexError::UnterminatedHeredoc),
            // End of a body line: emit the `\n` separator (quoted:true) and re-arm
            // the line-start delimiter check for the next line.
            Some('\n') => {
                self.cursor.next();
                self.history.push(Token::new(TokenKind::Lit { text: "\n".into(), quoted: true }, Span::new(off, l, c)));
                self.emitting_heredoc.as_mut().expect("emitting").at_line_start = true;
                self.heredoc_gen += 1; // v250 T6 fix: emitting_heredoc.at_line_start flip is a state change
                Ok(Step::Produced)
            }
            // Backslash — special ONLY before `$`/`` ` ``/`\` (the escaped char
            // becomes a quoted:true Literal); `\<NL>` is line continuation (both
            // deleted, stay mid-line); every other `\` is a literal backslash
            // (quoted:false, coalesced into the surrounding run by the parser).
            Some('\\') => {
                self.cursor.next(); // consume `\`
                match self.cursor.peek().copied() {
                    Some(e @ ('$' | '`' | '\\')) => {
                        self.cursor.next();
                        self.history.push(Token::new(TokenKind::Lit { text: e.to_string(), quoted: true }, Span::new(off, l, c)));
                        Ok(Step::Produced)
                    }
                    Some('\n') => {
                        self.cursor.next(); // line continuation: delete `\` + NL, join
                        // Emit no atom, but a scan_step must produce one: the cursor
                        // advanced (progress), so recurse to emit the next atom of
                        // the joined logical line (at_line_start stays false).
                        self.scan_step_heredoc_body_expanding(ph)
                    }
                    _ => {
                        self.history.push(Token::new(TokenKind::Lit { text: "\\".into(), quoted: false }, Span::new(off, l, c)));
                        Ok(Step::Produced)
                    }
                }
            }
            // Backtick command substitution — zero-width BeginBacktick signal.
            Some('`') => {
                self.history.push(Token::new(TokenKind::BeginBacktick, Span::new(off, l, c)));
                Ok(Step::Produced)
            }
            // `$`-expansion — same classification/emission as a `"…"` operand
            // (`scan_expanding_body_line` reuses `scan_dollar_expansion` with
            // quoted:true, exactly like the dquote scanner).
            Some('$') => {
                self.emit_dquote_dollar_atom(off, l, c);
                Ok(Step::Produced)
            }
            // Literal run — stop at `\n`/`$`/`` ` ``/`\`. `"`/`'` are LITERAL in a
            // heredoc body (NOT quote delimiters), so they are ordinary run chars.
            // quoted:false (matches the oracle's `current`-buffer flush).
            Some(_) => {
                let mut text = String::new();
                while let Some(&ch) = self.cursor.peek() {
                    if matches!(ch, '\n' | '$' | '`' | '\\') { break; }
                    text.push(ch);
                    self.cursor.next();
                }
                self.history.push(Token::new(TokenKind::Lit { text, quoted: false }, Span::new(off, l, c)));
                Ok(Step::Produced)
            }
        }
    }

    /// v250 T4: emit `HeredocBodyEnd`, pop the front pending heredoc, and re-arm
    /// (or clear) `emitting_heredoc` for the next queued body. Shared by the
    /// literal and expanding body emitters.
    fn emit_heredoc_body_end(&mut self) {
        let (off, l, c) = (self.cursor.offset(), self.cursor.line(), self.cursor.column());
        self.history.push(Token::new(TokenKind::HeredocBodyEnd, Span::new(off, l, c)));
        // v264: remove the entry we just finished (the first at the emitting
        // depth — NOT necessarily the FIFO front, since a shallower outer-line
        // heredoc may sit ahead of a cmdsub/backtick-body one), then re-arm for
        // the NEXT same-depth entry (more heredocs on the same body line) or
        // clear when none remain at this depth.
        let depth = self.emitting_heredoc.as_ref().map(|s| s.trigger_depth).unwrap_or_else(|| self.modes.len());
        if let Some(idx) = self.atom_heredoc_idx_at_depth(depth) {
            self.atom_pending_heredocs.remove(idx);
        }
        self.heredoc_gen += 1; // v250 T6: atom_pending_heredocs/emitting_heredoc changed
        self.emitting_heredoc = if self.atom_heredoc_idx_at_depth(depth).is_some() {
            Some(HeredocEmit { began: false, at_line_start: true, trigger_depth: depth })
        } else {
            None
        };
    }

    /// v250 T4: does the logical body line the cursor sits at (leading `<<-` tabs
    /// already stripped) equal the close delimiter? Bounded, non-consuming: clones
    /// the cursor and reads ONE logical line (applying `\<NL>` continuation joins,
    /// mirroring `collect_one_heredoc_body`) to compare against `ph.delim`. This is
    /// line-oriented delimiter matching, NOT a forward scan for a matching
    /// delimiter across the body. Returns `Some(n)` on a match, where `n` is the
    /// exact number of chars the probe consumed to form the logical line (every
    /// joined physical line + its terminating newline) — the caller advances the
    /// REAL cursor by `n` so a continuation-formed delimiter is fully consumed and
    /// nothing leaks; `None` on no match.
    fn heredoc_at_delim_line(&self, ph: &PendingHeredoc) -> Option<usize> {
        let mut probe = self.cursor.clone();
        let mut line = String::new();
        let mut got_nl = false;
        let mut consumed = 0usize;
        loop {
            match probe.next() {
                Some('\n') => { consumed += 1; got_nl = true; break; }
                Some(ch) => { consumed += 1; line.push(ch); }
                None => break,
            }
        }
        // Expanding-body line continuation: a physical line ending in an odd run of
        // backslashes joins the next line (both `\` and NL deleted) BEFORE the
        // delimiter comparison — so a would-be delimiter with a trailing `\` never
        // matches. Every char read here still counts toward `consumed` (the joined
        // `\` and NL are physically consumed on the real cursor).
        while got_nl && ends_with_continuation_backslash(&line) && probe.peek().is_some() {
            line.pop();
            got_nl = false;
            loop {
                match probe.next() {
                    Some('\n') => { consumed += 1; got_nl = true; break; }
                    Some(ch) => { consumed += 1; line.push(ch); }
                    None => break,
                }
            }
        }
        // Leading `<<-` tabs are already stripped on the real cursor; `trim` here is
        // a harmless no-op that also matches the oracle's whole-line strip.
        let check = if ph.strip_tabs { line.trim_start_matches('\t') } else { &line[..] };
        if check == ph.delim { Some(consumed) } else { None }
    }

    /// v250 T4: emit ONE atom for a `$`-expansion in a QUOTED operand context —
    /// shared by the dquote body (`scan_step_dquote`) and the expanding-heredoc
    /// body (`scan_step_heredoc_body_expanding`), which classify `$` identically
    /// (both mirror the oracle `scan_dollar_expansion(.., quoted=true)`). Cursor is
    /// at `$`. `${…}` consumes `${` and emits `ParamOpen`; `$(`/`$((`/`` ` `` are
    /// zero-width signals (cursor left at `$`) so the parser scans the nested
    /// structure from the cursor; `$name`/specials/`$N` consume and emit
    /// `DollarName`; `$[` is the still-deferred legacy-arith signal; a lone `$`
    /// emits `DollarLit`.
    fn emit_dquote_dollar_atom(&mut self, off: usize, l: u32, c: u32) {
        let mut probe = self.cursor.clone();
        probe.next(); // skip `$`
        match probe.peek().copied() {
            Some('{') => {
                self.cursor.next(); // `$`
                self.cursor.next(); // `{`
                self.history.push(Token::new(TokenKind::ParamOpen { quoted: true }, Span::new(off, l, c)));
            }
            Some('(') => {
                let mut probe2 = probe.clone();
                probe2.next(); // skip first `(`
                if probe2.peek() == Some(&'(') {
                    self.history.push(Token::new(TokenKind::ArithOpen, Span::new(off, l, c)));
                } else {
                    self.history.push(Token::new(TokenKind::CmdSubOpen, Span::new(off, l, c)));
                }
            }
            Some(sp @ ('?' | '@' | '*' | '#' | '$' | '!' | '-')) => {
                self.cursor.next(); // `$`
                self.cursor.next(); // special char
                self.history.push(Token::new(TokenKind::DollarName { name: sp.to_string(), quoted: true }, Span::new(off, l, c)));
            }
            Some(d) if d.is_ascii_digit() => {
                self.cursor.next(); // `$`
                let digit = self.cursor.next().unwrap();
                self.history.push(Token::new(TokenKind::DollarName { name: digit.to_string(), quoted: true }, Span::new(off, l, c)));
            }
            Some(nc) if is_name_start(nc) => {
                self.cursor.next(); // `$`
                let name = scan_var_name(&mut self.cursor);
                self.history.push(Token::new(TokenKind::DollarName { name, quoted: true }, Span::new(off, l, c)));
            }
            // `$[expr]` legacy arith (v258) — zero-width `LegacyArithOpen` signal
            // (cursor stays on `$`); the parser pushes Mode::Arith{delim:Bracket},
            // whose first scan consumes `$[` and emits the real LegacyArithOpen.
            Some('[') => {
                self.history.push(Token::new(TokenKind::LegacyArithOpen, Span::new(off, l, c)));
            }
            _ => {
                self.cursor.next(); // lone `$`
                self.history.push(Token::new(TokenKind::DollarLit { quoted: true }, Span::new(off, l, c)));
            }
        }
    }

    /// v247 T5: reset the word/assignment boundary flags after emitting a Blank,
    /// Newline, or operator — the next word-content atom begins a fresh word and
    /// is no longer in assignment-value context (mirrors the oracle's whitespace
    /// and operator arms clearing `has_token` / `in_assignment_value`).
    fn boundary_reset(&mut self) {
        self.cmd_at_word_start = true;
        self.in_assignment_value = false;
        self.assign_val_tilde_ok = false;
    }

    /// v247 T5: emit ONE structural operator/redirect token per call. The cursor
    /// sits on the operator's first char (`| & ; ( ) < >`). Mirrors
    /// `scan_step_command`'s operator arms char-for-char (multi-char recognition
    /// order matters). Heredoc / here-string openers emit the opener token but do
    /// NOT scan a body (deferred — the parser returns `UnsupportedCommand`).
    fn scan_command_operator_atom(&mut self, off: usize, l: u32, c: u32) -> Result<Step, LexError> {
        let first = self.cursor.next().expect("caller peeked an operator char");
        macro_rules! push { ($k:expr) => {{ self.history.push(Token::new($k, Span::new(off, l, c))) }} }
        match first {
            '|' => {
                match self.cursor.peek().copied() {
                    Some('|') => { self.cursor.next(); push!(TokenKind::Op(Operator::Or)); }
                    Some('&') => {
                        // `|&` desugars to `2>&1 |` (mirrors the oracle ~2254-2266).
                        // The `1` is emitted as a `Lit` atom (not a `Word`) so the
                        // atom redirect-target assembler consumes it uniformly.
                        self.cursor.next();
                        push!(TokenKind::RedirFd(crate::command::RedirFd::Number(2)));
                        push!(TokenKind::Op(Operator::DupOut));
                        push!(TokenKind::Lit { text: "1".to_string(), quoted: false });
                        push!(TokenKind::Op(Operator::Pipe));
                    }
                    _ => push!(TokenKind::Op(Operator::Pipe)),
                }
            }
            '&' => match self.cursor.peek().copied() {
                Some('&') => { self.cursor.next(); push!(TokenKind::Op(Operator::And)); }
                Some('>') => {
                    self.cursor.next();
                    if self.cursor.peek() == Some(&'>') {
                        self.cursor.next();
                        push!(TokenKind::Op(Operator::AndRedirAppend));
                    } else {
                        push!(TokenKind::Op(Operator::AndRedirOut));
                    }
                }
                _ => push!(TokenKind::Op(Operator::Background)),
            },
            ';' => {
                let op = if self.cursor.peek() == Some(&';') {
                    self.cursor.next();
                    if self.cursor.peek() == Some(&'&') { self.cursor.next(); Operator::DoubleSemiAmp }
                    else { Operator::DoubleSemi }
                } else if self.cursor.peek() == Some(&'&') {
                    self.cursor.next(); Operator::SemiAmp
                } else {
                    Operator::Semi
                };
                push!(TokenKind::Op(op));
            }
            '(' => push!(TokenKind::Op(Operator::LParen)),
            ')' => push!(TokenKind::Op(Operator::RParen)),
            '<' => match self.cursor.peek().copied() {
                Some('(') => {
                    // v251: `<(` process substitution. Zero-width word-part
                    // signal; DON'T consume `(` (Mode::CommandSub consumes it).
                    // Word continuation, so no boundary_reset: mark that a word
                    // has started (mirrors scan_command_word_atom emitting a Lit).
                    self.history.push(Token::new(TokenKind::ProcSubOpen { dir: ProcDir::In }, Span::new(off, l, c)));
                    self.cmd_at_word_start = false;
                    return Ok(Step::Produced);
                }
                Some('<') => {
                    self.cursor.next(); // second `<`
                    if self.cursor.peek() == Some(&'<') {
                        self.cursor.next(); // third `<` — here-string
                        push!(TokenKind::Op(Operator::HereString));
                    } else {
                        // v250: heredoc opener. Parse the delimiter (so `expand`
                        // is correct) and record a pending record in the ATOM
                        // queue. The body is emitted as atoms after the line's
                        // `\n` (a later task). Reuses TokenKind::Heredoc as the
                        // opener (empty placeholder body; the parser fills the
                        // AST from the body atoms once that's wired up) so
                        // next_is_redirect recognizes it unchanged. The parser
                        // still defers on any `Heredoc` token, so this is dormant.
                        let strip_tabs = if self.cursor.peek() == Some(&'-') {
                            self.cursor.next(); true
                        } else { false };
                        let (delim, expand) = parse_heredoc_delim(&mut self.cursor)?;
                        push!(TokenKind::Heredoc { body: Word(Vec::new()), expand, strip_tabs });
                        self.atom_pending_heredocs.push_back(PendingHeredoc {
                            delim, expand, strip_tabs, token_idx: 0, // token_idx unused on the atom path
                            reg_depth: self.modes.len(),
                        });
                        self.heredoc_gen += 1; // v250 T6: atom_pending_heredocs changed
                    }
                }
                Some('&') => { self.cursor.next(); push!(TokenKind::Op(Operator::DupIn)); }
                Some('>') => { self.cursor.next(); push!(TokenKind::Op(Operator::RedirReadWrite)); }
                _ => push!(TokenKind::Op(Operator::RedirIn)),
            },
            '>' => match self.cursor.peek().copied() {
                Some('(') => {
                    // v251: `>(` process substitution — see the `<(` arm above.
                    self.history.push(Token::new(TokenKind::ProcSubOpen { dir: ProcDir::Out }, Span::new(off, l, c)));
                    self.cmd_at_word_start = false;
                    return Ok(Step::Produced);
                }
                Some('>') => { self.cursor.next(); push!(TokenKind::Op(Operator::RedirAppend)); }
                Some('&') => { self.cursor.next(); push!(TokenKind::Op(Operator::DupOut)); }
                Some('|') => { self.cursor.next(); push!(TokenKind::Op(Operator::RedirClobber)); }
                _ => push!(TokenKind::Op(Operator::RedirOut)),
            },
            _ => unreachable!("scan_command_operator_atom called on non-operator char"),
        }
        self.boundary_reset();
        Ok(Step::Produced)
    }

    /// v247 T2: emit ONE atom's worth of command-word text per call — either a
    /// maximal unquoted-literal run (`Lit { quoted: false }`) or one complete
    /// quote run (`QuoteRun`: `'…'` / `"…"` / a single `\c` escape / `$'…'`
    /// ANSI-C). Mirrors the oracle's `scan_step_command` quoting byte-for-byte
    /// (that fat char-based scanner wraps every quote run in `WordPart::Quoted
    /// { style, .. }` — see its `'`/`"`/`\\` arms — so `QuoteRun` carries the
    /// `QuoteStyle` the parser needs to reproduce that wrapper; a flat `Lit`
    /// atom cannot, since `Lit` only carries a `quoted: bool`).
    ///
    /// T2 scope: no operators, no `$name`/`${…}`/`$(…)`/backtick expansions
    /// (`$'…'` is ANSI-C QUOTING, not an expansion, so it IS handled here —
    /// decoded via the shared `scan_ansi_c_quoted` leaf helper, never the fat
    /// `scan_dollar_expansion` dispatcher). A bare `$` not followed by `'`, or
    /// a backtick, is swallowed into the surrounding literal run — wrong in
    /// general, but the T2 corpus never exercises it; T3 breaks these out into
    /// their own atoms.
    /// `in_array_value`: v252 T2 — when true, this is scanning a positional
    /// value INSIDE `Mode::ArrayLiteral` rather than a top-level command word.
    /// Every arm is shared verbatim (assignment-prefix, quotes, `$`/backtick
    /// openers, tilde) — the ONLY difference is the plain-literal-run stop-set:
    /// command position stops at the metacharacters `;|&<>()` (the top-level
    /// scanner emits their own `Op` atom next call), but an array value has no
    /// such operators — `|;&<>` and a bare `(` all stay literal, so the run
    /// stops only at whitespace / a quote-or-`$`-or-backtick opener / the
    /// array-closing `)` (mirrors the oracle's `scan_array_element_word`,
    /// whose raw-buffer collector only special-cases those same chars before
    /// re-tokenizing the whole element as a standalone word). The redirect-fd
    /// look-back (`3>`, `{fd}<`) is command-position-only and skipped here —
    /// with `<`/`>` no longer stop chars, the run never stops with the cursor
    /// sitting on one, so that block would be dead code anyway.
    fn scan_command_word_atom(&mut self, in_array_value: bool) -> Result<Step, LexError> {
        let off = self.cursor.offset();
        let l   = self.cursor.line();
        let c   = self.cursor.column();
        // A `~` is tilde-special only at word start (mirrors the oracle's
        // `!has_token` guard); capture the flag before it's cleared below.
        let at_word_start = self.cmd_at_word_start;
        // v247 T4: at word start, try to peel a structured assignment prefix
        // (`name+=`, `name[sub]=`, `name[sub]+=`) or a plain scalar `name=`.
        if at_word_start {
            if let Some(step) = self.try_scan_assign_prefix(off, l, c)? {
                return Ok(step);
            }
        }
        // v247 T4: value-position tilde eligibility. `assign_val_tilde_ok` is true
        // when the previous unquoted literal char was `=`/`:` inside an assignment
        // value (mirrors the oracle's `tilde_eligible_in_assignment`). Capture it,
        // then DEFAULT-CLEAR: every atom kind resets it EXCEPT the literal-run arm,
        // which re-sets it based on its final char (a non-literal part flushes the
        // oracle's buffer, so a following `~` is no longer value-eligible).
        let tilde_ok = self.assign_val_tilde_ok;
        self.assign_val_tilde_ok = false;
        match self.cursor.peek().copied() {
            None => self.finish(),

            // `'…'` — single-quoted run: fully literal, no escapes recognized.
            Some('\'') => {
                self.cmd_at_word_start = false;
                self.cursor.next(); // consume opening `'`
                let mut text = String::new();
                loop {
                    match self.cursor.next() {
                        None => return Err(LexError::UnterminatedQuote),
                        Some('\'') => break,
                        Some(ch) => text.push(ch),
                    }
                }
                self.history.push(Token::new(
                    TokenKind::QuoteRun { style: QuoteStyle::Single, text },
                    Span::new(off, l, c),
                ));
                Ok(Step::Produced)
            }

            // `"…"` — double-quoted span. v247 T3: emit a ZERO-WIDTH BeginDquote
            // signal (cursor stays on `"`); the parser (`parse_dquote`) pushes
            // `Mode::DoubleQuote`, whose first scan consumes the opening `"` and
            // thereafter emits inner atoms (literals + expansion openers). This is
            // atom-native — the parser owns recursion into nested `$(…)`/`` `…`
            // ``/`$((…))` — instead of the T2 flat single-shot body scan.
            Some('"') => {
                self.cmd_at_word_start = false;
                self.history.push(Token::new(TokenKind::BeginDquote, Span::new(off, l, c)));
                Ok(Step::Produced)
            }

            // `\c` — backslash escape outside quotes: the next char is always
            // literal (one-char QuoteRun). `\<NL>` is line continuation — both
            // chars deleted, no atom emitted (cursor still advanced: safe, and
            // `cmd_at_word_start` is preserved so `\<NL>~` stays word-start tilde).
            Some('\\') => {
                self.cursor.next(); // consume `\`
                match self.cursor.next() {
                    None => {
                        self.cmd_at_word_start = false;
                        self.history.push(Token::new(
                            TokenKind::Lit { text: "\\".to_string(), quoted: false },
                            Span::new(off, l, c),
                        ));
                        Ok(Step::Produced)
                    }
                    Some('\n') => Ok(Step::Produced), // deleted — no atom, word-start preserved
                    Some(ch) => {
                        self.cmd_at_word_start = false;
                        self.history.push(Token::new(
                            TokenKind::QuoteRun { style: QuoteStyle::Backslash, text: ch.to_string() },
                            Span::new(off, l, c),
                        ));
                        Ok(Step::Produced)
                    }
                }
            }

            // `$'…'` — ANSI-C quoting (must precede the general `$` arm below).
            Some('$') if self.cursor.peek_nth(1) == Some('\'') => {
                self.cmd_at_word_start = false;
                self.cursor.next(); // `$`
                self.cursor.next(); // `'`
                let text = scan_ansi_c_quoted(&mut self.cursor)?;
                self.history.push(Token::new(
                    TokenKind::QuoteRun { style: QuoteStyle::AnsiC, text },
                    Span::new(off, l, c),
                ));
                Ok(Step::Produced)
            }

            // `$` — command-position expansion. Mirrors `scan_step_param_operand`'s
            // unquoted `$`-classification (v247 T3), quoted:false: `${`→ParamOpen,
            // `$((`→ArithOpen (zero-width), `$(`→CmdSubOpen (zero-width), specials/
            // digit/name→DollarName, lone `$`→Lit. Reuses the v246 `$((`-vs-`$(`
            // bounded peek.
            Some('$') => {
                self.cmd_at_word_start = false;
                self.emit_unquoted_dollar_atom(off, l, c);
                Ok(Step::Produced)
            }

            // Backtick command substitution — zero-width BeginBacktick signal
            // (cursor stays on `` ` ``; the parser's `parse_backtick_sub` pushes
            // `Mode::Backtick`, whose depth-0 scan consumes the opening `` ` ``).
            Some('`') => {
                self.cmd_at_word_start = false;
                self.history.push(Token::new(TokenKind::BeginBacktick, Span::new(off, l, c)));
                Ok(Step::Produced)
            }

            // v264 extglob (`shopt -s extglob`): one of `? * + @ !` directly
            // followed by `(` introduces an extglob group. Mirrors the oracle's
            // trigger (lexer.rs:2467, `scan_step_command`'s pre-dispatch check)
            // but atom-natively: emit a ZERO-WIDTH `ExtglobOpen{prefix}` signal
            // WITHOUT consuming the prefix/`(` (left for `Mode::Extglob`'s first
            // scan, pushed by the parser's `parse_extglob_group`). Checked before
            // the literal-run catch-all so the group is recognized first; with
            // extglob off this arm never matches and lexing is unchanged.
            Some(pc) if self.opts.extglob
                && matches!(pc, '?' | '*' | '+' | '@' | '!')
                && self.cursor.peek_nth(1) == Some('(') =>
            {
                self.cmd_at_word_start = false;
                self.history.push(Token::new(TokenKind::ExtglobOpen { prefix: pc }, Span::new(off, l, c)));
                Ok(Step::Produced)
            }

            // `~…` — tilde construct at WORD START (mirrors the oracle's
            // `!has_token` guard) OR in ASSIGNMENT-VALUE position right after an
            // unquoted `=`/`:` (v247 T4; mirrors `tilde_eligible_in_assignment`).
            // Mid-word `~` elsewhere (`a~b`, `$x~`) is swallowed into the literal
            // run below. In value context `:` is a tilde TERMINATOR, so
            // `try_parse_tilde` is told the assignment-value flag. In an ARRAY
            // VALUE (v252 T2), the closing `)` must ALSO count as a terminator
            // (equivalently: end-of-word): the oracle collects each element into
            // a BOUNDED raw buffer that never includes the closing `)`, then
            // re-tokenizes that buffer standalone, so a trailing `~` there sees
            // EOF, not `)` — our live-cursor scan sees the real `)` unless told
            // to treat it as a boundary too.
            Some('~') if at_word_start || (self.in_assignment_value && tilde_ok) => {
                self.cmd_at_word_start = false;
                self.cursor.next(); // consume `~`
                match try_parse_tilde(&mut self.cursor, self.in_assignment_value, in_array_value) {
                    Some(spec) => {
                        self.history.push(Token::new(TokenKind::Tilde(spec), Span::new(off, l, c)));
                    }
                    None => {
                        // Not a tilde construct — `~` is a literal (coalesced with
                        // any following literal run by the parser).
                        self.history.push(Token::new(
                            TokenKind::Lit { text: "~".into(), quoted: false },
                            Span::new(off, l, c),
                        ));
                    }
                }
                Ok(Step::Produced)
            }

            // Unquoted literal run: accumulate until the next quote opener,
            // backslash, expansion opener (`$`/`` ` ``), blank, or EOF. `~` is
            // NOT a stop char — mid-word it is literal — EXCEPT in assignment-value
            // position right after a `=`/`:`, where it opens a tilde construct
            // (v247 T4): break so the `~` arm fires on the next call.
            Some(_) => {
                self.cmd_at_word_start = false;
                let mut text = String::new();
                // Track the oracle's tilde eligibility per char: true iff we're in
                // an assignment value and the last char accumulated is `=`/`:`.
                let mut boundary = self.in_assignment_value && tilde_ok;
                while let Some(&ch) = self.cursor.peek() {
                    // Stop at whitespace, quote openers, and expansion openers
                    // always. At COMMAND position (v247 T5) also stop at the
                    // metacharacters `; | & < > ( )` — the top-level scanner
                    // emits their structural token on the next call. Inside an
                    // ARRAY VALUE (v252 T2) there are no such operators — those
                    // chars stay literal, so the run stops only at the closing
                    // `)` (the array literal's own boundary), not at `(` (kept
                    // literal — mirrors the oracle, which never nests `(`/`)`
                    // tracking for a plain unquoted `(` in a value). `#` is NOT
                    // a stop char either way: mid-word it is literal (`a#b`); a
                    // word-start `#` is a comment, handled before this arm runs.
                    if ch.is_whitespace() { break; }
                    // v264 extglob: a mid-run `?*+@!` immediately followed by
                    // `(` must break WITHOUT consuming it, so the top-level
                    // match's dedicated trigger arm fires on the next call
                    // (mirrors `zzz+(q)` glued-prefix — the oracle's own
                    // per-char loop checks this same condition every iteration,
                    // not only at word start).
                    if self.opts.extglob
                        && matches!(ch, '?' | '*' | '+' | '@' | '!')
                        && self.cursor.peek_nth(1) == Some('(')
                    { break; }
                    if in_array_value {
                        if ch == ')' || matches!(ch, '\'' | '"' | '\\' | '$' | '`') { break; }
                    } else if matches!(ch, '\'' | '"' | '\\' | '$' | '`'
                                    | ';' | '|' | '&' | '<' | '>' | '(' | ')') { break; }
                    if boundary && ch == '~' { break; }
                    text.push(ch);
                    self.cursor.next();
                    boundary = self.in_assignment_value && matches!(ch, '=' | ':');
                }
                // v247 T5 fd-prefix: a WHOLE-word pure digit-run or `{ident}` (this
                // run began at word start) glued directly to a redirect operator
                // (`3>out`, `{fd}<in`) is an fd-prefix — emit `RedirFd` in place of
                // the `Lit`, mirroring the oracle's `take_fd_prefix` look-back. The
                // `at_word_start` guard is the atom analogue of the oracle
                // classifying the ENTIRE flushed word: a run after earlier glued
                // content (`x=2>`, `a3>`) is never a whole-word digit-run.
                // COMMAND-POSITION ONLY (v252 T2): with `<`/`>` no longer stop
                // chars in an array value, the run never stops with the cursor
                // sitting on one, so this block would never fire there anyway —
                // `in_array_value` just documents that explicitly.
                if !in_array_value && at_word_start && matches!(self.cursor.peek(), Some('<') | Some('>')) {
                    // Oracle special-case: a bare `1` glued to `>` (`1>`, `1>>`,
                    // `1>&2`, `1>|`) is a plain STDOUT redirect with the DEFAULT
                    // fd — NOT `RedirFd(1)` (`scan_step_command`'s `'1' if peek=='>'`
                    // arm, lexer.rs ~2476, emits the op with no fd prefix →
                    // `plain_fd()` = `Default`). Drop the `1` (emit no token); the
                    // top-level scanner emits the `>`-family op next with its
                    // default fd. (`2>` needs NO such case: `RedirFd(2)` + the
                    // stdout op builds the SAME AST as the oracle's stderr op, whose
                    // `err_fd()` also defaults to `Number(2)`. `1<` is a genuine
                    // `RedirFd(1)` — the special arm is `>`-only — so it falls
                    // through.)
                    if text == "1" && self.cursor.peek() == Some(&'>') {
                        return Ok(Step::Produced);
                    }
                    if let Some(fd) = fd_prefix_of_text(&text) {
                        self.assign_val_tilde_ok = false;
                        self.history.push(Token::new(TokenKind::RedirFd(fd), Span::new(off, l, c)));
                        return Ok(Step::Produced);
                    }
                }
                // Carry the eligibility to the next atom: true when the run ended on
                // an unquoted `=`/`:` (or broke right before a value-tilde `~`).
                self.assign_val_tilde_ok = boundary;
                // `text` is non-empty: none of the break conditions can fire on
                // the FIRST char of this arm (the outer match already routed
                // `'`/`"`/`\\`/`$`/`` ` `` away, and metacharacters are handled by
                // the top-level scanner; a value-tilde `~` at the very start is
                // routed to the `~` arm above), so the loop consumes ≥1 char.
                self.history.push(Token::new(
                    TokenKind::Lit { text, quoted: false },
                    Span::new(off, l, c),
                ));
                Ok(Step::Produced)
            }
        }
    }

    /// v247 T4: at word start, try to recognize and consume an assignment prefix.
    /// Mirrors the oracle's `=`/`+=`/`[…]` arms, which fire only when the word so
    /// far is identifier-shaped and the value has not yet started:
    ///
    ///   - `name=`         → PLAIN scalar assignment. Emits a single `Lit`
    ///                        `"name="` (NO `AssignPrefix`); the value flows into
    ///                        the literal run and `try_split_assignment` splits on
    ///                        the first unquoted `=` — byte-identical to the oracle.
    ///   - `name+=`        → `AssignPrefix { Bare(name), append: true }`.
    ///   - `name[sub]=` / `name[sub]+=`
    ///                     → `AssignPrefix { Indexed { name, subscript }, append }`,
    ///                        `subscript` parsed by the `parse_subscript_body` leaf.
    ///
    /// The bracket form is speculative: if the `[…]` is not closed, or is not
    /// followed by `=`/`+=` (e.g. glob `a[abc]*`), NOTHING is consumed and `None`
    /// is returned so the caller falls back to ordinary word scanning (the oracle's
    /// fallback treats the `[` and its body as literal). Sets `in_assignment_value`
    /// and seeds `assign_val_tilde_ok` (true only after the bare `name=`, whose
    /// buffer ends in `=`; false after `+=`/`[sub]=`, whose buffer is empty).
    ///
    /// ARRAY LITERALS (`name=(…)`, `name+=(…)`, `name[i]=(…)`) are DEFERRED: the
    /// compound `(` RHS is left unconsumed for a later task (do NOT scan it here).
    fn try_scan_assign_prefix(&mut self, off: usize, l: u32, c: u32) -> Result<Option<Step>, LexError> {
        // The prefix must begin with an identifier: [A-Za-z_][A-Za-z0-9_]*.
        let Some(first) = self.cursor.peek().copied() else { return Ok(None) };
        if !is_name_start(first) { return Ok(None); }
        // Scan the maximal identifier on a probe (nothing is consumed yet).
        let mut probe = self.cursor.clone();
        let mut name = String::new();
        while let Some(&ch) = probe.peek() {
            if is_name_cont(ch) { name.push(ch); probe.next(); } else { break; }
        }
        // Identifier chars are ASCII, so `name.len()` (bytes) == the char count.
        match probe.peek().copied() {
            // `name=` — plain scalar: emit `Lit { "name=" }`, no AssignPrefix.
            Some('=') => {
                for _ in 0..name.len() { self.cursor.next(); }
                self.cursor.next(); // `=`
                name.push('=');
                self.cmd_at_word_start = false;
                self.in_assignment_value = true;
                self.assign_val_tilde_ok = true; // buffer now ends in `=`
                self.history.push(Token::new(
                    TokenKind::Lit { text: name, quoted: false },
                    Span::new(off, l, c),
                ));
                // v252: compound array RHS `name=(...)`. A `\<NL>` may sit between
                // the prefix and `(` (bash deletes it). Mirror the production `=`
                // arm's inline `(` probe: emit a zero-width ArrayOpen so the parser
                // pushes Mode::ArrayLiteral. Cursor is LEFT on `(`.
                skip_line_continuations(&mut self.cursor);
                if self.cursor.peek() == Some(&'(') {
                    let (ao, al, ac) = (self.cursor.offset(), self.cursor.line(), self.cursor.column());
                    self.history.push(Token::new(TokenKind::ArrayOpen, Span::new(ao, al, ac)));
                }
                Ok(Some(Step::Produced))
            }
            // `name+=` — scalar/array append. `name+x` (no `=`) is NOT an
            // assignment: leave everything for ordinary word scanning.
            Some('+') => {
                let mut p2 = probe.clone();
                p2.next(); // `+`
                if p2.peek() != Some(&'=') { return Ok(None); }
                for _ in 0..name.len() { self.cursor.next(); }
                self.cursor.next(); // `+`
                self.cursor.next(); // `=`
                self.cmd_at_word_start = false;
                self.in_assignment_value = true;
                self.assign_val_tilde_ok = false; // buffer empty after the prefix
                self.history.push(Token::new(
                    TokenKind::AssignPrefix {
                        target: crate::command::AssignTarget::Bare(name),
                        append: true,
                    },
                    Span::new(off, l, c),
                ));
                // v252: compound array RHS `name+=(...)`. A `\<NL>` may sit between
                // the prefix and `(` (bash deletes it). Mirror the production `=`
                // arm's inline `(` probe: emit a zero-width ArrayOpen so the parser
                // pushes Mode::ArrayLiteral. Cursor is LEFT on `(`.
                skip_line_continuations(&mut self.cursor);
                if self.cursor.peek() == Some(&'(') {
                    let (ao, al, ac) = (self.cursor.offset(), self.cursor.line(), self.cursor.column());
                    self.history.push(Token::new(TokenKind::ArrayOpen, Span::new(ao, al, ac)));
                }
                Ok(Some(Step::Produced))
            }
            // `name[sub]=` / `name[sub]+=` — subscripted lvalue. Speculatively scan
            // the bounded `[…]`; only an assignment if `=`/`+=` follows.
            Some('[') => {
                let mut bracket = probe.clone();
                bracket.next(); // consume `[`
                let mut raw = String::new();
                let mut depth: usize = 1;
                let mut closed = false;
                while let Some(&ch) = bracket.peek() {
                    if ch == '[' { depth += 1; raw.push(ch); bracket.next(); }
                    else if ch == ']' {
                        bracket.next();
                        depth -= 1;
                        if depth == 0 { closed = true; break; }
                        raw.push(ch);
                    } else { raw.push(ch); bracket.next(); }
                }
                // Decide: an indexed assignment only if the bracket CLOSED and
                // `=`/`+=` follows the `]`. Otherwise (unclosed bracket, or no
                // `=`/`+=`) this is NOT an assignment.
                let append: Option<bool> = if closed {
                    match bracket.peek().copied() {
                        Some('=') => { bracket.next(); Some(false) }
                        Some('+') => {
                            let mut p2 = bracket.clone();
                            p2.next();
                            if p2.peek() == Some(&'=') { bracket.next(); bracket.next(); Some(true) }
                            else { None }
                        }
                        _ => None,
                    }
                } else {
                    None
                };
                match append {
                    Some(append) => {
                        // Confirmed indexed assignment. Parse the subscript (leaf
                        // helper), then advance the real cursor to where `bracket` sits.
                        let subscript = parse_subscript_body(&raw, self.opts)?;
                        self.cursor.seek(bracket.offset(), bracket.line(), bracket.column());
                        self.cmd_at_word_start = false;
                        self.in_assignment_value = true;
                        self.assign_val_tilde_ok = false; // buffer empty after the prefix
                        self.history.push(Token::new(
                            TokenKind::AssignPrefix {
                                target: crate::command::AssignTarget::Indexed { name, subscript },
                                append,
                            },
                            Span::new(off, l, c),
                        ));
                        // v252: compound array RHS `name[sub]=(...)` / `name[sub]+=(...)`.
                        // A `\<NL>` may sit between the prefix and `(` (bash deletes it).
                        // Mirror the scalar `=`/`+=` arms' inline `(` probe: emit a
                        // zero-width ArrayOpen so the parser pushes Mode::ArrayLiteral.
                        // Cursor is LEFT on `(`.
                        skip_line_continuations(&mut self.cursor);
                        if self.cursor.peek() == Some(&'(') {
                            let (ao, al, ac) = (self.cursor.offset(), self.cursor.line(), self.cursor.column());
                            self.history.push(Token::new(TokenKind::ArrayOpen, Span::new(ao, al, ac)));
                        }
                        Ok(Some(Step::Produced))
                    }
                    None => {
                        // NOT an indexed assignment (unclosed bracket, or no
                        // `=`/`+=` after `]`). Mirror the oracle's literal-swallow
                        // fallback (`scan_step_command`'s `None =>` arm): the WHOLE
                        // `name[…]` region becomes ONE raw literal atom — `$`,
                        // backtick, and quotes inside stay LITERAL (never re-lexed).
                        // Advance the real cursor past `name` + `[` + raw + `]?` so
                        // scanning resumes just after the region; downstream literal
                        // coalescing merges any following literal run.
                        let mut text = name;
                        text.push('[');
                        text.push_str(&raw);
                        if closed { text.push(']'); }
                        self.cursor.seek(bracket.offset(), bracket.line(), bracket.column());
                        self.cmd_at_word_start = false;
                        self.history.push(Token::new(
                            TokenKind::Lit { text, quoted: false },
                            Span::new(off, l, c),
                        ));
                        Ok(Some(Step::Produced))
                    }
                }
            }
            _ => Ok(None),
        }
    }

    /// v247 T3: `Mode::DoubleQuote { body_started }` scanner — emits the inner
    /// atoms of a `"…"` command-word span. On entry (`body_started == false`) the
    /// cursor sits just before the opening `"` (the parser consumed the zero-width
    /// `BeginDquote` signal but not the `"` itself); consume the `"`, flip the
    /// frame to `body_started`, and scan the first inner atom in the same call.
    ///
    /// Inner atoms (all `quoted: true`): literal chunks with POSIX double-quote
    /// backslash rules (`\$ \` \" \\` unescape; `\<NL>` line continuation; other
    /// `\c` keeps the `\`), and the SAME expansion openers as command position
    /// (`ParamOpen`/`ArithOpen`/`CmdSubOpen`/`BeginBacktick`/`DollarName`). On the
    /// closing `"`, emit `EndDquote` (the parser pops the mode). Mirrors
    /// `scan_step_param_operand`'s `in_dquote` branch but wires `$(`/`$((` (which
    /// the operand path still defers) since the parser owns the recursion.
    fn scan_step_dquote(&mut self, body_started: bool) -> Result<Step, LexError> {
        if !body_started {
            // Consume the opening `"` and flip the frame to the body phase.
            debug_assert_eq!(self.cursor.peek(), Some(&'"'), "scan_step_dquote entry: expected opening \"");
            self.cursor.next(); // consume opening `"`
            if let Some(Mode::DoubleQuote { body_started }) = self.modes.last_mut() {
                *body_started = true;
            }
            // Fall through to scan the first inner atom.
        }
        let off = self.cursor.offset();
        let l   = self.cursor.line();
        let c   = self.cursor.column();
        match self.cursor.peek().copied() {
            None => Err(LexError::UnterminatedQuote),

            // Closing `"` — emit EndDquote; the parser pops the DoubleQuote frame.
            Some('"') => {
                self.cursor.next(); // consume closing `"`
                self.history.push(Token::new(TokenKind::EndDquote, Span::new(off, l, c)));
                Ok(Step::Produced)
            }

            // Backslash: special only before `$`, `` ` ``, `"`, `\` inside `"…"`;
            // `\<NL>` is line continuation (both deleted); other `\c` keeps `\`.
            Some('\\') => {
                self.cursor.next(); // consume `\`
                match self.cursor.peek().copied() {
                    Some(e @ ('$' | '`' | '"' | '\\')) => {
                        self.cursor.next();
                        self.history.push(Token::new(
                            TokenKind::Lit { text: e.to_string(), quoted: true },
                            Span::new(off, l, c),
                        ));
                    }
                    Some('\n') => {
                        self.cursor.next(); // line continuation — both chars deleted, no atom
                    }
                    _ => {
                        let mut s = String::from("\\");
                        if let Some(ch) = self.cursor.next() { s.push(ch); }
                        self.history.push(Token::new(
                            TokenKind::Lit { text: s, quoted: true },
                            Span::new(off, l, c),
                        ));
                    }
                }
                Ok(Step::Produced)
            }

            // Backtick command substitution — zero-width BeginBacktick signal.
            Some('`') => {
                self.history.push(Token::new(TokenKind::BeginBacktick, Span::new(off, l, c)));
                Ok(Step::Produced)
            }

            // `$` expansion inside `"…"` — mirrors the command-position `$` arm
            // but with quoted:true. Shared with the expanding-heredoc body via
            // `emit_dquote_dollar_atom` (identical classification).
            Some('$') => {
                self.emit_dquote_dollar_atom(off, l, c);
                Ok(Step::Produced)
            }

            // Literal run inside `"…"`: stop at `"`, `$`, `` ` ``, or `\`.
            Some(_) => {
                let mut text = String::new();
                while let Some(&ch) = self.cursor.peek() {
                    if matches!(ch, '"' | '$' | '`' | '\\') { break; }
                    text.push(ch);
                    self.cursor.next();
                }
                self.history.push(Token::new(
                    TokenKind::Lit { text, quoted: true },
                    Span::new(off, l, c),
                ));
                Ok(Step::Produced)
            }
        }
    }

    /// v254: emit the atom for an unquoted `$…` at the cursor (cursor on `$`).
    /// `${`→ParamOpen{false}, `$((`→ArithOpen, `$(`→CmdSubOpen, specials/digit/
    /// name→DollarName{false}, `$[`→LegacyArithOpen, lone `$`→DollarLit{false}.
    /// Extracted verbatim from scan_command_word_atom's `$` arm — called from
    /// BOTH the command scanner and `scan_step_regex` (the whole command-position
    /// `$`-test suite is the regression gate).
    fn emit_unquoted_dollar_atom(&mut self, off: usize, l: u32, c: u32) {
        let mut probe = self.cursor.clone();
        probe.next(); // skip `$`
        match probe.peek().copied() {
            Some('{') => {
                self.cursor.next(); // `$`
                self.cursor.next(); // `{`
                self.history.push(Token::new(TokenKind::ParamOpen { quoted: false }, Span::new(off, l, c)));
            }
            Some('(') => {
                let mut probe2 = probe.clone();
                probe2.next(); // skip first `(`
                if probe2.peek() == Some(&'(') {
                    // `$((` — zero-width ArithOpen signal (cursor stays on `$`).
                    self.history.push(Token::new(TokenKind::ArithOpen, Span::new(off, l, c)));
                } else {
                    // `$(cmd)` — zero-width CmdSubOpen signal (cursor stays on `$`).
                    self.history.push(Token::new(TokenKind::CmdSubOpen, Span::new(off, l, c)));
                }
            }
            Some(sp @ ('?' | '@' | '*' | '#' | '$' | '!' | '-')) => {
                self.cursor.next(); // `$`
                self.cursor.next(); // special char
                self.history.push(Token::new(TokenKind::DollarName { name: sp.to_string(), quoted: false }, Span::new(off, l, c)));
            }
            Some(d) if d.is_ascii_digit() => {
                self.cursor.next(); // `$`
                let digit = self.cursor.next().unwrap();
                self.history.push(Token::new(TokenKind::DollarName { name: digit.to_string(), quoted: false }, Span::new(off, l, c)));
            }
            Some(nc) if is_name_start(nc) => {
                self.cursor.next(); // `$`
                let name = scan_var_name(&mut self.cursor);
                self.history.push(Token::new(TokenKind::DollarName { name, quoted: false }, Span::new(off, l, c)));
            }
            // `$"…"` — bash locale quoting; huck's translation is the identity,
            // so `$"…" ≡ "…"`. Drop the `$` and emit the zero-width BeginDquote
            // (cursor left on `"`), exactly mirroring a bare `"`; the parser's
            // Mode::DoubleQuote then consumes the `"` and scans the body. (Oracle:
            // scan_dollar_expansion's `Some('"') if !quoted => {}`.)
            Some('"') => {
                self.cursor.next(); // consume `$` only, leave `"`
                self.history.push(Token::new(TokenKind::BeginDquote, Span::new(off, l, c)));
            }
            // `$[expr]` legacy arith (v258) — zero-width `LegacyArithOpen` signal
            // (cursor stays on `$`); the parser pushes Mode::Arith{delim:Bracket},
            // whose first scan consumes `$[` and emits the real LegacyArithOpen.
            Some('[') => {
                self.history.push(Token::new(TokenKind::LegacyArithOpen, Span::new(off, l, c)));
            }
            _ => {
                self.cursor.next(); // lone `$`
                self.history.push(Token::new(
                    TokenKind::DollarLit { quoted: false },
                    Span::new(off, l, c),
                ));
            }
        }
    }

    /// v254: `Mode::Regex { paren_depth, body_started }` scanner — emits the atoms
    /// of the `=~` pattern operand inside `[[ … ]]`. Mirrors `scan_regex_operand`
    /// (see the production fn) atom-natively:
    ///  - literal runs (incl. the regex metacharacters `| < > ; &` and depth-tracked
    ///    `( )`) → `Lit { quoted: false }`;
    ///  - `$`/`` ` ``/`"`/`'`/`$'` → the SAME expansion-opener signals the command
    ///    scanner emits (`ParamOpen`/`CmdSubOpen`/`ArithOpen`/`DollarName`/`DollarLit`/
    ///    `BeginBacktick`/`BeginDquote`/`QuoteRun`), so the parser recurses via the
    ///    existing sub-modes;
    ///  - `\<NL>` → line-continuation (deleted); `\x` → literal `\x` (backslash KEPT,
    ///    UNQUOTED — unlike the command word's `QuoteRun{Backslash}`); `\`-EOF → `\`;
    ///  - depth-0 whitespace or EOF → pop the mode + emit zero-width `RegexEnd`
    ///    (leading whitespace while `!body_started` is skipped, not a terminator).
    fn scan_step_regex(&mut self, paren_depth: u32, body_started: bool) -> Result<Step, LexError> {
        let off = self.cursor.offset();
        let l   = self.cursor.line();
        let c   = self.cursor.column();

        // Leading-whitespace / continuation skip while the operand is still empty.
        if !body_started {
            loop {
                match self.cursor.peek().copied() {
                    Some(ch) if ch.is_whitespace() => { self.cursor.next(); }
                    // `\<NL>` continuation before any operand content.
                    Some('\\') if self.cursor.peek_nth(1) == Some('\n') => { self.cursor.next(); self.cursor.next(); }
                    _ => break,
                }
            }
        }

        // Terminator: EOF or depth-0 whitespace ends the operand.
        match self.cursor.peek().copied() {
            None => { self.pop_mode(); self.history.push(Token::new(TokenKind::RegexEnd, Span::new(off, l, c))); return Ok(Step::Produced); }
            Some(ch) if ch.is_whitespace() && paren_depth == 0 => {
                self.pop_mode();
                self.history.push(Token::new(TokenKind::RegexEnd, Span::new(off, l, c)));
                return Ok(Step::Produced);
            }
            _ => {}
        }

        // `body_started` is PARSER-MANAGED (v254 T1 fix): the lexer only READS it
        // (for the leading-ws/`\<NL>` skip above) and never self-sets it. The
        // parser calls `set_regex_body_started(!(parts.is_empty()&&acc.is_none()))`
        // after each atom, so `body_started` reflects the oracle's
        // `!(lit.is_empty() && parts.is_empty())` — an EMPTY `""` produces NO part,
        // leaving the operand "unstarted" so the following space is still treated as
        // leading (skipped); the pattern then becomes the literal `]]`, which the
        // oracle's `next_test_word` rejects as `Err(TestExprMissingOperand)` (see the
        // `=~` arm guard). This scan still writes back `paren_depth` in the literal arm.

        match self.cursor.peek().copied() {
            // Unreachable: the terminator match above already returned on EOF.
            None => unreachable!("regex EOF handled by the terminator match"),
            // Single-quoted run → QuoteRun{Single} (reuse the command-scanner shape).
            Some('\'') => {
                self.cursor.next();
                let mut text = String::new();
                loop {
                    match self.cursor.next() {
                        None => return Err(LexError::UnterminatedQuote),
                        Some('\'') => break,
                        Some(ch) => text.push(ch),
                    }
                }
                self.history.push(Token::new(TokenKind::QuoteRun { style: QuoteStyle::Single, text }, Span::new(off, l, c)));
                Ok(Step::Produced)
            }
            // Double-quoted span → zero-width BeginDquote (parser pushes Mode::DoubleQuote).
            Some('"') => { self.history.push(Token::new(TokenKind::BeginDquote, Span::new(off, l, c))); Ok(Step::Produced) }
            // `$'…'` ANSI-C (must precede the general `$` arm).
            Some('$') if self.cursor.peek_nth(1) == Some('\'') => {
                self.cursor.next(); self.cursor.next();
                let text = scan_ansi_c_quoted(&mut self.cursor)?;
                self.history.push(Token::new(TokenKind::QuoteRun { style: QuoteStyle::AnsiC, text }, Span::new(off, l, c)));
                Ok(Step::Produced)
            }
            // `$…` — same unquoted classification the command scanner uses.
            Some('$') => { self.emit_unquoted_dollar_atom(off, l, c); Ok(Step::Produced) }
            // Backtick command-sub → zero-width BeginBacktick.
            Some('`') => { self.history.push(Token::new(TokenKind::BeginBacktick, Span::new(off, l, c))); Ok(Step::Produced) }
            // Literal run (incl. `| < > ; &`, depth-tracked `( )`, `\`-escapes,
            // and depth>0 whitespace). Stops at an expansion/quote opener, at
            // depth-0 whitespace, or EOF (the outer match re-enters).
            Some(_) => {
                let mut text = String::new();
                let mut depth = paren_depth;
                while let Some(&ch) = self.cursor.peek() {
                    match ch {
                        '\'' | '"' | '`' => break,           // quote/backtick openers
                        '$' => break,                        // expansion opener
                        c2 if c2.is_whitespace() && depth == 0 => break, // terminator
                        '\\' => {
                            self.cursor.next();              // consume `\`
                            match self.cursor.next() {
                                Some('\n') => {}             // line continuation: delete both
                                Some(next) => { text.push('\\'); text.push(next); }
                                None => text.push('\\'),
                            }
                        }
                        '(' => { text.push('('); depth += 1; self.cursor.next(); }
                        ')' => { text.push(')'); depth = depth.saturating_sub(1); self.cursor.next(); }
                        _   => { text.push(ch); self.cursor.next(); } // incl. | < > ; & and depth>0 ws
                    }
                }
                // Persist the running paren depth on the mode for the next step.
                if let Some(Mode::Regex { paren_depth: p, .. }) = self.modes.last_mut() { *p = depth; }
                if text.is_empty() {
                    // Only reachable if the first char was `\<NL>` at operand start
                    // (already handled) — re-enter to hit the terminator/opener.
                    return self.scan_step_regex(depth, true);
                }
                self.history.push(Token::new(TokenKind::Lit { text, quoted: false }, Span::new(off, l, c)));
                Ok(Step::Produced)
            }
        }
    }

    /// v264: `Mode::Extglob { paren_depth }` scanner — emits the atoms of an
    /// extglob group `<prefix>( … )` (`?(...)`/`*(...)`/`+(...)`/`@(...)`/
    /// `!(...)`, gated by `LexerOptions::extglob`). Mirrors `scan_step_regex`'s
    /// shape (literal runs + the SAME expansion-opener atoms for inner `$…`/
    /// `` `…` ``/`"…"`/`'…'`/`$'…'`), adapted for extglob's two differences:
    ///  1. the boundary is the matching `)` (`paren_depth` returning to 0), not
    ///     depth-0 whitespace/EOF;
    ///  2. the prefix char and every structural `(`/`)` are literal TEXT that
    ///     must stay byte-identical to the oracle's `scan_extglob_group`
    ///     (lexer.rs `fn scan_extglob_group`), including its quirks: `\`
    ///     escapes keep BOTH chars verbatim (no `\<NL>` line-continuation
    ///     deletion, unlike the main word/regex scanners), and a nested
    ///     extglob prefix (`@(a*(b)c)`) is NOT recognized specially — an inner
    ///     `(` (even preceded by a bare `*`/`+`/etc., themselves just literal
    ///     chars here) simply increments `paren_depth`; only the OUTER
    ///     `scan_command_word_atom` trigger recognizes a fresh `<prefix>(` at
    ///     word-content position.
    ///
    /// On first entry (`paren_depth == 0`) consumes `<prefix>(` and emits it as
    /// one `Lit` atom (mirrors the oracle's `let mut lit = format!("{prefix}(")`),
    /// setting `paren_depth` to 1. The literal-run arm can emit the closing `)`
    /// `Lit` AND the zero-width `ExtglobEnd` terminator (plus popping the mode)
    /// in the SAME call — no separate "already closed" mode state is needed,
    /// since `scan_step` is never invoked again for a popped mode frame.
    fn scan_step_extglob(&mut self, paren_depth: u32) -> Result<Step, LexError> {
        let off = self.cursor.offset();
        let l   = self.cursor.line();
        let c   = self.cursor.column();

        if paren_depth == 0 {
            // Fresh entry: the trigger guaranteed `<prefix>(` sits at the cursor.
            let prefix = self.cursor.next().expect("extglob entry: prefix char present (trigger guaranteed it)");
            debug_assert!(matches!(prefix, '?' | '*' | '+' | '@' | '!'), "extglob entry: unexpected prefix {prefix:?}");
            debug_assert_eq!(self.cursor.peek(), Some(&'('), "extglob entry: expected '(' after prefix");
            self.cursor.next(); // consume '('
            if let Some(Mode::Extglob { paren_depth: p }) = self.modes.last_mut() { *p = 1; }
            self.history.push(Token::new(
                TokenKind::Lit { text: format!("{prefix}("), quoted: false },
                Span::new(off, l, c),
            ));
            return Ok(Step::Produced);
        }

        match self.cursor.peek().copied() {
            // EOF mid-group — mirrors the oracle falling through its
            // `while let Some(c) = chars.next()` loop without hitting depth 0.
            None => Err(LexError::UnterminatedExtglob),
            // Single-quoted run → flat Literal{quoted:true} (mirrors the
            // oracle's `scan_squote_content` + flat push — NOT a `Quoted`
            // wrapper — same shape `scan_step_regex` uses).
            Some('\'') => {
                self.cursor.next();
                let mut text = String::new();
                loop {
                    match self.cursor.next() {
                        None => return Err(LexError::UnterminatedQuote),
                        Some('\'') => break,
                        Some(ch) => text.push(ch),
                    }
                }
                self.history.push(Token::new(TokenKind::QuoteRun { style: QuoteStyle::Single, text }, Span::new(off, l, c)));
                Ok(Step::Produced)
            }
            // Double-quoted span → zero-width BeginDquote (parser pushes Mode::DoubleQuote).
            Some('"') => { self.history.push(Token::new(TokenKind::BeginDquote, Span::new(off, l, c))); Ok(Step::Produced) }
            // `$'…'` ANSI-C (must precede the general `$` arm).
            Some('$') if self.cursor.peek_nth(1) == Some('\'') => {
                self.cursor.next(); self.cursor.next();
                let text = scan_ansi_c_quoted(&mut self.cursor)?;
                self.history.push(Token::new(TokenKind::QuoteRun { style: QuoteStyle::AnsiC, text }, Span::new(off, l, c)));
                Ok(Step::Produced)
            }
            // `$…` — same unquoted classification the command scanner uses.
            Some('$') => { self.emit_unquoted_dollar_atom(off, l, c); Ok(Step::Produced) }
            // Backtick command-sub → zero-width BeginBacktick.
            Some('`') => { self.history.push(Token::new(TokenKind::BeginBacktick, Span::new(off, l, c))); Ok(Step::Produced) }
            // Literal run: accumulate chars, tracking paren depth, until a
            // quote/`$`/backtick opener interrupts it OR the group's own `)`
            // brings depth back to 0 (closing the group). No whitespace-stop —
            // unlike regex, whitespace at ANY depth is ordinary literal content
            // here (mirrors the oracle: `other => lit.push(other)` has no
            // whitespace special-case).
            Some(_) => {
                let mut text = String::new();
                let mut depth = paren_depth;
                let mut closed = false;
                while let Some(&ch) = self.cursor.peek() {
                    match ch {
                        '\'' | '"' | '`' | '$' => break, // quote/expansion openers
                        '\\' => {
                            // Oracle's extglob `\` arm keeps BOTH chars verbatim —
                            // NO `\<NL>` line-continuation deletion (unlike the
                            // main word/regex scanners' backslash handling).
                            self.cursor.next(); // consume `\`
                            text.push('\\');
                            if let Some(next) = self.cursor.next() { text.push(next); }
                        }
                        '(' => { text.push('('); depth += 1; self.cursor.next(); }
                        ')' => {
                            text.push(')'); self.cursor.next();
                            depth -= 1;
                            if depth == 0 { closed = true; break; }
                        }
                        _ => { text.push(ch); self.cursor.next(); } // incl. | and any whitespace
                    }
                }
                if !closed && self.cursor.peek().is_none() {
                    return Err(LexError::UnterminatedExtglob);
                }
                // Persist the running paren depth on the mode for the next step
                // (only reachable when NOT closed — the mode is popped below on
                // closure, so writing back afterward would resurrect a stale
                // frame; harmless either way since nothing reads it once popped).
                if let Some(Mode::Extglob { paren_depth: p }) = self.modes.last_mut() { *p = depth; }
                self.history.push(Token::new(TokenKind::Lit { text, quoted: false }, Span::new(off, l, c)));
                if closed {
                    self.pop_mode();
                    self.history.push(Token::new(TokenKind::ExtglobEnd, Span::new(off, l, c)));
                }
                Ok(Step::Produced)
            }
        }
    }

    /// v252 T1/T2: `Mode::ArrayLiteral { body_started }` scanner — emits the
    /// inner atoms of a `name=(...)`/`name+=(...)` compound array RHS. On entry
    /// (`body_started == false`) the cursor sits on the opening `(` (the parser
    /// consumed the zero-width `ArrayOpen` signal but not the `(` itself);
    /// consume it, flip the frame, and scan the first inner atom in the same
    /// call. T1 scope was POSITIONAL values as a bare literal run stopping at
    /// whitespace/`)`. T2 widens the value content itself: each value is
    /// scanned exactly like a fresh command word (quote/`$`/backtick openers,
    /// tilde, assignment-prefix recognition — ALL shared with
    /// `scan_command_word_atom`) via its `in_array_value` stop-set, mirroring
    /// the oracle's `scan_array_element_word`, which collects the element's raw
    /// text (preserving nested quote/expansion bodies verbatim) and then
    /// RE-TOKENIZES it as a standalone word — so e.g. `a=(a=~)`'s element gets
    /// the SAME assignment-prefix + value-tilde treatment a fresh word would.
    /// Bracketed subscripts (`[i]=value`) are NOT wired here (Task 3 widens
    /// this). Mirrors `scan_array_literal`/`scan_array_element_word`'s
    /// separator/value grammar.
    fn scan_step_array_literal(&mut self, body_started: bool, expect_subscript_eq: bool, at_element_start: bool) -> Result<Step, LexError> {
        if !body_started {
            debug_assert_eq!(self.cursor.peek(), Some(&'('), "array-literal entry: expected '('");
            self.cursor.next(); // consume opening '('
            if let Some(Mode::ArrayLiteral { body_started, .. }) = self.modes.last_mut() {
                *body_started = true;
            }
            // Each value is scanned as a FRESH word (mirrors the oracle
            // re-tokenizing the collected element text from scratch): reset the
            // word-start/assignment-value state so `try_scan_assign_prefix` and
            // value-tilde eligibility see a clean slate for the first value.
            self.cmd_at_word_start = true;
            self.in_assignment_value = false;
            self.assign_val_tilde_ok = false;
            // fall through to scan the first atom (at_element_start is already
            // true from the push, so a leading `[` opens a subscript).
        }
        // v252 T3: control has returned from the parser's `[expr]` subscript scan
        // (the cursor sits just past `]`). The oracle (`scan_array_literal`)
        // requires a `=` here — consume it, clear the flag, and scan the value's
        // first atom in this same call. `at_element_start` is already false (it was
        // cleared when the `LBracket` was emitted), so the value's leading char —
        // even a `[` — is treated as literal, not another subscript. If `=` is
        // absent → ArrayLiteralMissingEquals.
        if expect_subscript_eq {
            if let Some(Mode::ArrayLiteral { expect_subscript_eq: e, .. }) = self.modes.last_mut() {
                *e = false;
            }
            if self.cursor.peek() == Some(&'=') {
                self.cursor.next(); // consume the required '='
            } else {
                return Err(LexError::ArrayLiteralMissingEquals);
            }
        }
        let (off, l, c) = (self.cursor.offset(), self.cursor.line(), self.cursor.column());
        match self.cursor.peek().copied() {
            None => Err(LexError::UnterminatedArrayLiteral),
            Some(')') => {
                self.cursor.next();
                self.history.push(Token::new(TokenKind::ArrayClose, Span::new(off, l, c)));
                Ok(Step::Produced)
            }
            // Inter-element separators: whitespace / newline / `#`-comment.
            // Coalesce a maximal run into ONE Blank atom (a comment consumes to EOL,
            // its body — incl. any `)` — never read as elements; matches
            // skip_array_literal_separators). Never emit content for a separator.
            //
            // NOTE: a bare `\<NL>` line continuation is NOT a separator — it is
            // GLUE (bash deletes it), so it must fall through to the value
            // scanner below when it abuts element text with no surrounding
            // whitespace (`1\<NL>2` -> one element `12`). It is still handled
            // INSIDE the coalescing loop below so a continuation that follows
            // real whitespace (`1 \<NL>2`) is absorbed into that separator run.
            Some(ch) if ch.is_whitespace() || ch == '#' => {
                loop {
                    match self.cursor.peek().copied() {
                        Some(w) if w.is_whitespace() => { self.cursor.next(); }
                        Some('#') => { while let Some(&x) = self.cursor.peek() { if x == '\n' { break; } self.cursor.next(); } }
                        Some('\\') => {
                            let mut p = self.cursor.clone(); p.next();
                            if p.peek() == Some(&'\n') { self.cursor.next(); self.cursor.next(); } else { break; }
                        }
                        _ => break,
                    }
                }
                self.history.push(Token::new(TokenKind::Blank, Span::new(off, l, c)));
                // The NEXT value (if any) starts a fresh word — same reset as
                // the entry bootstrap above. A separator opens a NEW element, so
                // the persistent `at_element_start` flips back to true (a `[`
                // right after this separator is a subscript, not a literal).
                self.cmd_at_word_start = true;
                self.in_assignment_value = false;
                self.assign_val_tilde_ok = false;
                if let Some(Mode::ArrayLiteral { at_element_start, .. }) = self.modes.last_mut() {
                    *at_element_start = true;
                }
                Ok(Step::Produced)
            }
            // v252 T3: a `[` AT ELEMENT START begins an explicit `[expr]=value`
            // subscript (mirrors the oracle's leading-`[` sniff in
            // `scan_array_literal`). Emit a zero-width `LBracket`; the parser
            // pushes `Mode::ParamSubscriptOperand`, assembles the subscript Word,
            // consumes `RBracket`, pops — then the required `=` is consumed by the
            // `expect_subscript_eq` block above on the next `scan_step`. The guard
            // uses the PERSISTENT `at_element_start` (true only right after `(` or a
            // separator, before ANY value atom of this element): a `[` mid-value —
            // e.g. after a `$x`/quote atom that ended a prior scan_step, or in the
            // same run — stays literal (BUG-1 fix). Clear it here: emitting the
            // subscript ends the element's start.
            Some('[') if at_element_start => {
                self.cursor.next(); // consume '['
                if let Some(Mode::ArrayLiteral { expect_subscript_eq, at_element_start, .. }) = self.modes.last_mut() {
                    *expect_subscript_eq = true;
                    *at_element_start = false;
                }
                self.history.push(Token::new(TokenKind::LBracket, Span::new(off, l, c)));
                Ok(Step::Produced)
            }
            // Value content — shares `scan_command_word_atom`'s full atom
            // classification (quotes/`$`/backtick/tilde/assignment-prefix),
            // just with the array-value stop-set (`in_array_value = true`):
            // the plain-literal run stops ONLY at whitespace / `)` / a quote
            // or `$`/backtick opener — `|;&<>` and a bare `(` all stay literal.
            // Emitting a value atom ends the element's start, so subsequent
            // scan_step re-entries (for the next atom of THIS same value — a
            // quote/`$`/backtick/tilde that ended its own call) see
            // `at_element_start == false` and keep a mid-value `[` literal.
            _ => {
                if let Some(Mode::ArrayLiteral { at_element_start, .. }) = self.modes.last_mut() {
                    *at_element_start = false;
                }
                self.scan_command_word_atom(true)
            }
        }
    }

    /// End-of-input epilogue, run incrementally: flush a pending final word
    /// (once), then report any unterminated heredoc, else EOF. Flushing the
    /// final word returns Produced so next_token() drains it before EOF.
    fn finish(&mut self) -> Result<Step, LexError> {
        if self.has_token {
            flush_literal(&mut self.parts, &mut self.current, false);
            emit_word_with_braces(&mut self.history, std::mem::take(&mut self.parts), self.brace_expand, Span::new(self.token_start, self.token_start_line, self.token_start_col))?;
            self.has_token = false;
            return Ok(Step::Produced);
        }
        if !self.pending_heredocs.is_empty() {
            return Err(LexError::UnterminatedHeredoc);
        }
        Ok(Step::Eof)
    }

    /// True iff a pending heredoc still targets the token at `idx` (its body
    /// is not yet backfilled). Such a token must not be handed out yet.
    fn backfill_pending_at(&self, idx: usize) -> bool {
        self.pending_heredocs.iter().any(|ph| ph.token_idx == idx)
    }

    /// Pull one token, scanning lazily. Hands out the next buffered token
    /// unless it is a heredoc still awaiting its body, in which case it scans
    /// further (collecting the body) first. Returns None at end of input.
    fn next_token(&mut self) -> Result<Option<Token>, LexError> {
        loop {
            if self.pos < self.history.len() && !self.backfill_pending_at(self.pos) {
                let t = self.history[self.pos].clone();
                self.pos += 1;
                return Ok(Some(t));
            }
            match self.scan_step()? {
                Step::Eof => return Ok(None),
                Step::Produced => {}
            }
        }
    }

    /// Test-only: current byte offset of the char cursor. Lets tests assert that
    /// `next_token` consumes input lazily (the cursor stays near the tokens
    /// handed out, never jumping to EOF up front).
    #[cfg(test)]
    fn cursor_offset(&self) -> usize {
        self.cursor.offset()
    }

    /// Test-only: number of tokens scanned into history so far. Used by
    /// incrementality tests to assert that `parse_one_unit` does not eagerly
    /// scan the entire input.
    #[cfg(test)]
    pub fn scanned_token_count(&self) -> usize { self.history.len() }

    /// Build a replay lexer over already-tokenized input (Task 2 bridge). history is
    /// pre-filled; scanning is a no-op so the pull never errors.
    pub fn from_tokens(tokens: Vec<Token>) -> Lexer<'static> {
        Lexer {
            cursor: CharCursor::new(""),
            opts: LexerOptions::default(),
            brace_expand: true,
            history: tokens,
            pos: 0,
            replay: true,
            parts: Vec::new(),
            current: String::new(),
            has_token: false,
            token_start: 0,
            token_start_line: 1,
            token_start_col: 1,
            in_assignment_value: false,
            dbracket_depth: 0,
            expect_regex: false,
            pending_heredocs: std::collections::VecDeque::new(),
            atom_pending_heredocs: std::collections::VecDeque::new(),
            emitting_heredoc: None,
            parsed_heredoc_bodies: Vec::new(),
            aliases: std::collections::HashMap::new(),
            active: std::collections::HashSet::new(),
            alias_trailing_eligible: false,
            modes: vec![Mode::Command],
            retokenize_arith_as_cmdsub: false,
            command_atoms: false,
            cmd_at_word_start: true,
            assign_val_tilde_ok: false,
            heredoc_gen: 0,
        }
    }

    /// Ensure history[idx] exists AND is backfill-ready (heredoc body present),
    /// pulling lazily via scan_step. Mirrors next_token's readiness rule so a
    /// Heredoc token is never exposed before its body is collected (v238). On a lex
    /// error, RETURN it (no stash). scan_step appends to history without advancing pos.
    fn fill_to(&mut self, idx: usize) -> Result<(), LexError> {
        if self.replay {
            return Ok(());
        }
        loop {
            if self.history.len() > idx && !self.backfill_pending_at(idx) {
                return Ok(());
            }
            match self.scan_step()? {
                Step::Produced => {}
                Step::Eof => return Ok(()),
            }
        }
    }

    pub fn peek(&mut self) -> Result<Option<&Token>, LexError> {
        self.fill_to(self.pos)?;
        Ok(self.history.get(self.pos))
    }
    pub fn next(&mut self) -> Result<Option<Token>, LexError> {
        self.fill_to(self.pos)?;
        let t = self.history.get(self.pos).cloned();
        if t.is_some() { self.pos += 1; }
        Ok(t)
    }
    pub fn peek_kind(&mut self) -> Result<Option<&TokenKind>, LexError> {
        self.fill_to(self.pos)?;
        Ok(self.history.get(self.pos).map(|t| &t.kind))
    }
    pub fn peek2_kind(&mut self) -> Result<Option<&TokenKind>, LexError> {
        self.fill_to(self.pos + 1)?;
        Ok(self.history.get(self.pos + 1).map(|t| &t.kind))
    }
    /// Peek the token `n` positions ahead of the cursor WITHOUT consuming
    /// (n=0 == `peek_kind`, n=1 == `peek2_kind`).  Fills the lookahead buffer
    /// as needed; reads already-buffered tokens only — no forward scan for a
    /// delimiter.
    pub fn peek_nth_kind(&mut self, n: usize) -> Result<Option<&TokenKind>, LexError> {
        self.fill_to(self.pos + n)?;
        Ok(self.history.get(self.pos + n).map(|t| &t.kind))
    }
    pub fn next_kind(&mut self) -> Result<Option<TokenKind>, LexError> {
        Ok(self.next()?.map(|t| t.kind))
    }
    pub fn peek_span(&mut self) -> Result<Option<Span>, LexError> {
        self.fill_to(self.pos)?;
        Ok(self.history.get(self.pos).map(|t| t.span))
    }
    pub fn current_line(&mut self) -> Result<u32, LexError> {
        Ok(self.peek_span()?.map(|s| s.line).unwrap_or(0))
    }
    pub fn remaining(&self) -> usize {
        self.history.len().saturating_sub(self.pos)
    }

    pub fn set_aliases(&mut self, aliases: std::collections::HashMap<String, String>) {
        self.aliases = aliases;
    }

    /// Returns the current byte offset of the scanner cursor within the input
    /// slice. After a lex error from `parse_one_unit`, this is the position
    /// where the scanner gave up — used by the source loop to compute the
    /// restart line (`next_line_start(start + iter.cursor_pos())`).
    pub fn cursor_pos(&self) -> usize {
        self.cursor.offset()
    }

    /// Set the starting line number for span generation. Call after `new_live`
    /// when the input slice starts mid-file (`start > 0`) so that token spans
    /// carry file-absolute line numbers and `$LINENO` reflects the true file
    /// line rather than a chunk-relative one.
    pub fn set_base_line(&mut self, base_line: u32) {
        self.cursor.line = 1 + base_line;
        self.token_start_line = 1 + base_line;
    }

    /// Expand a registered alias at command position by splicing its body tokens into
    /// `history` ahead of `pos`. Implements bash's read-time alias rules (recursion
    /// guard, trailing-blank, span inheritance). Body tokens take the alias-name span.
    fn maybe_expand_command_alias(&mut self) -> Result<(), LexError> {
        self.fill_to(self.pos)?;
        self.alias_trailing_eligible = false;   // default: a non-expanding word leaves it false
        let Some(tok) = self.history.get(self.pos) else { return Ok(()) };
        let name_span = tok.span;
        // `is_bare_atom_lit`: whether `name` came from a bare atom `Lit` (needs
        // the boundary re-check below) vs. a legacy `Word` token (already the
        // whole word by construction — no further check needed).
        let (name, is_bare_atom_lit) = match &tok.kind {
            TokenKind::Word(w) => {
                let Some(name) = word_literal_text(w) else { return Ok(()) };
                (name, false)
            }
            // v264: the atom-command stream (`command_atoms`) has no `Word`
            // token at command position — the command word is a bare `Lit`
            // atom. Extract the name here; the boundary check (that this
            // `Lit` is the ENTIRE word, nothing glued on) happens below, once
            // the borrow of `self.history` this match holds has ended (the
            // check itself needs `&mut self` via `fill_to`).
            TokenKind::Lit { text, quoted: false } if self.command_atoms => {
                (text.clone(), true)
            }
            _ => return Ok(()),
        };
        if is_bare_atom_lit {
            // Only a WORD BOUNDARY immediately after (`Blank`/`Newline`/
            // `ArrayClose`/`Op(_)`/EOF) means this `Lit` is the entire command
            // word. Any other follower atom (another `Lit`, `DollarName`,
            // `ParamOpen`, `CmdSubOpen`, `BeginBacktick`, `BeginDquote`,
            // `QuoteRun`, `ArithOpen`, `LegacyArithOpen`, `ProcSubOpen`,
            // `ArrayOpen`, `Tilde`, `AssignPrefix`, `DeferredExpansion`, …)
            // means the word has more parts and is NOT a bare literal name —
            // mirrors `word_literal_text` returning `Some` only for a
            // single-literal word.
            self.fill_to(self.pos + 1)?;
            let boundary = matches!(
                self.history.get(self.pos + 1).map(|t| &t.kind),
                None | Some(TokenKind::Blank)
                    | Some(TokenKind::Newline)
                    | Some(TokenKind::ArrayClose)
                    | Some(TokenKind::Op(_))
            );
            if !boundary { return Ok(()); }
        }
        if self.active.contains(&name) { return Ok(()); }
        let Some(body) = self.aliases.get(&name).cloned() else { return Ok(()) };
        let body_tokens = tokenize(&body)?; // bad body → Err, propagated by callers
        self.history.remove(self.pos);
        let mut insert_at = self.pos;
        for bt in body_tokens {
            self.history.insert(insert_at, Token::new(bt.kind, name_span));
            insert_at += 1;
        }
        // Recursion guard: re-enter with `name` active so the body's own first word
        // expands if it is a *different* alias, but `name` cannot re-expand itself.
        self.active.insert(name.clone());
        self.maybe_expand_command_alias()?;
        self.active.remove(&name);
        self.alias_trailing_eligible = body.chars().last().is_some_and(|c| c.is_whitespace());
        Ok(())
    }

    /// Expand the alias at command position. Call this at the top of any
    /// command-position parse entry point; the parser then reads via the plain
    /// `peek_kind`/`next_kind` API which sees the already-expanded history.
    pub fn expand_command_alias(&mut self) -> Result<(), LexError> {
        self.maybe_expand_command_alias()
    }

    /// Return and reset the trailing-blank eligibility flag. Returns `true`
    /// (and clears to `false`) when the most recent alias expansion ended with
    /// a blank, making the next argument word eligible for alias expansion.
    pub fn take_trailing_eligible(&mut self) -> bool {
        let e = self.alias_trailing_eligible;
        self.alias_trailing_eligible = false;
        e
    }

    /// Build a live lexer for the REPL: the lexer scans `input` incrementally
    /// and expands registered aliases at command position.
    pub fn new_live(input: &'a str, aliases: &std::collections::HashMap<String, String>, opts: LexerOptions) -> Lexer<'a> {
        let mut lx = Lexer::new(input, opts, true);
        lx.aliases = aliases.clone();
        lx
    }

    /// v247: a live lexer whose `Mode::Command` emits atoms (dormant atom path).
    pub fn new_live_atoms(
        input: &'a str,
        aliases: &std::collections::HashMap<String, String>,
        opts: LexerOptions,
    ) -> Lexer<'a> {
        let mut lx = Lexer::new_live(input, aliases, opts);
        lx.command_atoms = true;
        lx
    }
}

/// Returns the concatenated literal text of a Word iff every part is an
/// unquoted Literal. Returns None for any quoted, Var, Arith, CommandSub, or
/// Tilde part — aliases only expand from plain unquoted identifiers.
fn word_literal_text(w: &Word) -> Option<String> {
    let mut s = String::new();
    for part in &w.0 {
        match part {
            WordPart::Literal { text, quoted: false } => s.push_str(text),
            _ => return None,
        }
    }
    if s.is_empty() { None } else { Some(s) }
}

fn tokenize_partial_inner(
    input: &str,
    opts: LexerOptions,
    brace_expand: bool,
) -> PartialTokens {
    // Build the token vec purely by draining the incremental next_token().
    let mut lx = Lexer::new(input, opts, brace_expand);
    let mut out = Vec::new();
    loop {
        match lx.next_token() {
            Ok(Some(t)) => out.push(t),
            Ok(None) => return (out, None),
            Err(e) => {
                let off = lx.cursor.offset();
                // Error path is terminal: include any tokens scanned but not yet
                // handed out — e.g. an unterminated heredoc's placeholder, which
                // the readiness rule kept buffered (its body will never arrive).
                // This makes the partial set byte-identical to the batch lexer,
                // while the readiness rule still governs normal incremental reads.
                out.extend(lx.history[lx.pos..].iter().cloned());
                return (out, Some((e, off)));
            }
        }
    }
}

/// Tokenizes `input`, returning `(tokens, error)`. On a lex error the tokens
/// produced BEFORE the error are returned alongside `Some((error, byte_offset))`;
/// on success the second element is `None`. Each token carries its own span, so
/// there is no separate offsets sidecar — the byte offset of the truncation is
/// the `byte_offset` in the error tuple. This lets the source reader execute the
/// complete units that lexed before the failure and re-lex the truncated unit.
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
        (tokens, None) => Ok(tokens),
        (_, Some((e, _))) => Err(e),
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

/// Collects a single-quoted `'…'` span body (opening `'` already consumed).
/// Returns the content as a `String`, NOT including the closing `'`.
/// Errors with `UnterminatedQuote` if the input ends before the closing `'`.
fn scan_squote_content(chars: &mut CharCursor<'_>) -> Result<String, LexError> {
    let mut out = String::new();
    loop {
        match chars.next() {
            Some('\'') => return Ok(out),
            Some(ch) => out.push(ch),
            None => return Err(LexError::UnterminatedQuote),
        }
    }
}

/// Scans a `"…"` span body (opening `"` already consumed): expands `$`/`` ` ``/`\`,
/// pushes resulting `WordPart`s into `parts`. Consumes through the closing `"`.
fn scan_dquote_expansion_body(
    chars: &mut CharCursor<'_>,
    parts: &mut Vec<WordPart>,
    opts: LexerOptions,
) -> Result<(), LexError> {
    let mut q = String::new();
    loop {
        match chars.next() {
            Some('"') => break,
            Some('\\') => match chars.next() {
                Some(esc @ ('"' | '\\' | '$' | '`')) => q.push(esc),
                Some('\n') => {}
                Some(other) => { q.push('\\'); q.push(other); }
                None => return Err(LexError::UnterminatedQuote),
            },
            Some('$') => {
                flush_literal(parts, &mut q, true);
                scan_dollar_expansion(chars, parts, true, opts)?;
            }
            Some('`') => {
                flush_literal(parts, &mut q, true);
                let sequence = scan_backtick_substitution(chars, opts)?;
                parts.push(WordPart::CommandSub { sequence, quoted: true });
            }
            Some(ch) => q.push(ch),
            None => return Err(LexError::UnterminatedQuote),
        }
    }
    flush_literal(parts, &mut q, true);
    Ok(())
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
                let inner = scan_squote_content(chars)?;
                parts.push(WordPart::Literal { text: inner, quoted: true });
            }
            '"' => {
                flush(&mut lit, &mut parts);
                scan_dquote_expansion_body(chars, &mut parts, opts)?;
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
                let inner = scan_squote_content(chars)?;
                group_parts.push(WordPart::Literal { text: inner, quoted: true });
            }
            '"' => {
                // Double quote: mirror the main loop's `"` arm.
                flush(&mut lit, &mut group_parts);
                scan_dquote_expansion_body(chars, &mut group_parts, opts)?;
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

/// Emits the word for `parts` into `tokens`, expanding any unquoted braces.
/// Every emitted Word (1 normally, or one per brace-expansion product) is built
/// with `span` — the source span of the word's first character — so each token
/// carries its own location. Returns the number of tokens pushed.
fn emit_word_with_braces(
    tokens: &mut Vec<Token>,
    parts: Vec<WordPart>,
    brace_expand: bool,
    span: Span,
) -> Result<usize, LexError> {
    if !brace_expand {
        tokens.push(Token::new(TokenKind::Word(Word(parts)), span));
        return Ok(1);
    }
    let products = brace_expand_parts(parts)?;
    let count = products.len();
    for p in products {
        // Every brace-expansion product shares the source word's start span.
        tokens.push(Token::new(TokenKind::Word(Word(p)), span));
    }
    Ok(count)
}

/// Brace-expands a word's `parts` into one-or-more parts-lists. With no
/// unquoted brace, returns the single input list unchanged. Non-literal
/// parts (expansions, quoted runs) are sentinel-protected so only literal
/// source braces expand. Shared by `emit_word_with_braces` (command words)
/// and `scan_array_literal` (bare array elements).
pub(crate) fn brace_expand_parts(parts: Vec<WordPart>) -> Result<Vec<Vec<WordPart>>, LexError> {
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
/// placeholder `TokenKind::Heredoc` at `token_idx`.
///
/// v260 CF1 bridge: under `command_atoms` ONLY, ALSO pushes each resolved body
/// onto `parsed_heredoc_bodies` (the same Lexer-owned FIFO the
/// atom-command-parser's `parsed_heredoc_bodies` queue uses — see
/// `push_heredoc_body`/`take_heredoc_bodies`) instead of patching the token
/// body directly. This function (via `scan_step_command`) is the ONLY
/// heredoc-resolution path reachable from a nested-command context
/// (`Mode::CommandSub`/`Mode::Backtick` body scanning always delegates to
/// `scan_step_command`, regardless of the top-level `command_atoms` flag —
/// v244's dormant CommandSub-body reuse). The atom-command-parser's
/// `parse_one_redirect` always builds a placeholder `Word(vec![])` for a
/// `TokenKind::Heredoc` and discards whatever `body` the consumed token
/// carries (correct for a TRUE atom-scanned heredoc, whose body arrives later
/// as `HeredocBodyBegin`…`End` atoms) — but a nested heredoc resolved HERE
/// never emits those atoms, so without this bridge its body is never queued
/// and the placeholder is left empty forever. The batch-tokenizing oracle
/// (`command.rs`'s `parse`) never drains this queue and reads the token body
/// directly, so under `command_atoms == false` the body is moved straight
/// into the token slot with no bridge push (no wasted clone).
fn collect_heredoc_bodies(
    chars: &mut CharCursor<'_>,
    pending: &mut std::collections::VecDeque<PendingHeredoc>,
    tokens: &mut [Token],
    opts: LexerOptions,
    parsed_heredoc_bodies: &mut Vec<Word>,
    command_atoms: bool,
) -> Result<(), LexError> {
    while let Some(ph) = pending.pop_front() {
        let body = collect_one_heredoc_body(chars, &ph, opts)?;
        if let Some(TokenKind::Heredoc { body: slot, expand, strip_tabs }) =
            tokens.get_mut(ph.token_idx).map(|t| &mut t.kind)
        {
            *expand = ph.expand;
            *strip_tabs = ph.strip_tabs;
            if command_atoms {
                // Atom path: parse_one_redirect discards the token body and
                // builds an empty placeholder, filling it later from this FIFO.
                *slot = Word(vec![]);
                parsed_heredoc_bodies.push(body); // move, no clone
            } else {
                // Oracle/production: command.rs reads the token body directly and
                // never drains parsed_heredoc_bodies. Move the body in; no push.
                *slot = body;
            }
        } else {
            unreachable!("placeholder token at index was not TokenKind::Heredoc");
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
                        flush_literal(parts, &mut current, false);
                        parts.push(WordPart::Literal { text: next.to_string(), quoted: true });
                    }
                    _ => current.push('\\'),
                }
            }
            '$' => {
                flush_literal(parts, &mut current, false);
                // Heredoc bodies are quoted-context (no word-splitting).
                scan_dollar_expansion(&mut chars, parts, true, opts)?;
            }
            '`' => {
                flush_literal(parts, &mut current, false);
                let sequence = scan_backtick_substitution(&mut chars, opts)?;
                parts.push(WordPart::CommandSub { sequence, quoted: true });
            }
            other => current.push(other),
        }
    }
    flush_literal(parts, &mut current, false);
    Ok(())
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
                push_quoted_span(chars, '\'', out, unterminated.clone())?;
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

/// Collects a raw ANSI-C `$'…'` body (both `$` and opening `'` already consumed).
/// Appends chars to `out` with `\`-escape pairs verbatim; does NOT push the
/// closing `'`. Returns `Ok(())` on the first unescaped `'`; `Err(err)` on EOF.
fn scan_raw_ansi_c_body(
    chars: &mut CharCursor<'_>,
    out: &mut String,
    err: LexError,
) -> Result<(), LexError> {
    loop {
        match chars.next() {
            None => return Err(err),
            Some('\\') => {
                out.push('\\');
                if let Some(c) = chars.next() { out.push(c); }
            }
            Some('\'') => return Ok(()),
            Some(c) => out.push(c),
        }
    }
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
                push_quoted_span(chars, '\'', &mut body, LexError::UnterminatedBrace)?;
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
                        scan_raw_ansi_c_body(chars, &mut body, LexError::UnterminatedBrace)?;
                        body.push('\'');
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
                    flush_literal(&mut parts, &mut cur, true);
                    parts.push(WordPart::Literal { text: e.to_string(), quoted: true });
                }
                _ => cur.push('\\'),
            },
            '\\' => {
                // Backslash escapes the next char: emit it as a quoted literal
                // (glob-safe, consistent with the main tokenizer). `\` at end of
                // body silently vanishes.
                if let Some(n) = chars.next() {
                    flush_literal(&mut parts, &mut cur, false);
                    parts.push(WordPart::Literal { text: n.to_string(), quoted: true });
                }
            }
            '$' => {
                flush_literal(&mut parts, &mut cur, q);
                scan_dollar_expansion(&mut chars, &mut parts, q, opts)?;
            }
            '`' => {
                flush_literal(&mut parts, &mut cur, q);
                let sequence = scan_backtick_substitution(&mut chars, opts)?;
                parts.push(WordPart::CommandSub { sequence, quoted: q });
            }
            // In double-quote context a single quote is a LITERAL character.
            '\'' if enclosing_dquote => cur.push('\''),
            '\'' => {
                // Single-quoted span: everything literal until the next `'`.
                flush_literal(&mut parts, &mut cur, false);
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
                flush_literal(&mut parts, &mut cur, q);
                loop {
                    match chars.next() {
                        None => return Err(LexError::UnterminatedQuote),
                        Some('"') => break,
                        Some('\\') => match chars.peek().copied() {
                            Some(e @ ('$' | '`' | '"' | '\\')) => {
                                chars.next();
                                flush_literal(&mut parts, &mut cur, true);
                                parts.push(WordPart::Literal { text: e.to_string(), quoted: true });
                            }
                            _ => cur.push('\\'),
                        },
                        Some('$') => {
                            flush_literal(&mut parts, &mut cur, true);
                            scan_dollar_expansion(&mut chars, &mut parts, true, opts)?;
                        }
                        Some('`') => {
                            flush_literal(&mut parts, &mut cur, true);
                            let sequence = scan_backtick_substitution(&mut chars, opts)?;
                            parts.push(WordPart::CommandSub { sequence, quoted: true });
                        }
                        Some(ch) => cur.push(ch),
                    }
                }
                flush_literal(&mut parts, &mut cur, true);
            }
            other => cur.push(other),
        }
    }
    flush_literal(&mut parts, &mut cur, q);
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
    let mut tokens = tokenize_with_opts(body, opts).map_err(|e| LexError::Substitution(Box::new(e)))?;
    // A command-substitution body is parsed in isolation; its token lines are
    // body-relative, not script-relative. Keep inner commands' `line` at 0
    // ("unknown"), matching pre-span behavior (script-relative $LINENO inside
    // `$( )` would need offset propagation, out of scope here).
    for t in &mut tokens { t.span.line = 0; }
    let mut lx = Lexer::from_tokens(tokens);
    let parsed = crate::command::parse(&mut lx).map_err(LexError::SubstitutionParseError)?;
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
            // `${$'…'}` (extquote name) / `${$"…"}` (bad-subst) must NOT be
            // parsed as the `$` shell-pid special param. If `$` is followed by
            // a quote, fall through to the extquote-aware regular-name path.
            let mut look = chars.clone();
            look.next();
            if matches!(look.peek().copied(), Some('\'') | Some('"')) {
                // fall through (do not consume, do not return)
            } else {
                chars.next();
                return dispatch_braced_modifier("$".to_string(), quoted, None, chars, parts, false, opts, dollar_start);
            }
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

    let (name, name_decoded) = match scan_braced_name_ext(chars)? {
        NameScan::BadSubst => return recover_bad_subst(chars, parts, quoted, dollar_start),
        NameScan::Name { name, decoded } => {
            // extquote: a `$'…'`-decoded name is only valid in double-quote
            // context (bash). `quoted` covers top-level + default operands;
            // `opts.in_dquote` covers pattern operands. Unquoted -> bad subst.
            if decoded && !(quoted || opts.in_dquote) {
                return recover_bad_subst(chars, parts, quoted, dollar_start);
            }
            // A decoded name must be a valid identifier (e.g. `${$'x\ty'}` is
            // invalid -> bad subst). A non-decoded name keeps prior behavior.
            if decoded && !is_valid_param_name(&name) {
                return recover_bad_subst(chars, parts, quoted, dollar_start);
            }
            (name, decoded)
        }
    };
    if name.is_empty() {
        // `${}` (truly empty) or `${+foo}` etc. — bad substitution at runtime.
        return recover_bad_subst(chars, parts, quoted, dollar_start);
    }
    // Optional subscript: `${a[…]}`, `${a[@]}`, `${a[*]}`.
    let subscript = scan_param_subscript(chars, opts)?;
    let pre_len = parts.len();
    dispatch_braced_modifier(name, quoted, subscript, chars, parts, false, opts, dollar_start)?;
    // When the name was decoded from `$'…'`, the dispatcher emits a bare `Var`
    // for `${$'x1'}` — which `declare -f` would reconstruct as `$x1`.  Promote
    // it to a `ParamExpansion` with `ParamModifier::None` so reconstruction
    // yields the normalised `${x1}` form (matches bash `declare -f` output).
    if name_decoded && parts.len() > pre_len {
        if let Some(WordPart::Var { name: vn, quoted: vq }) = parts.last().cloned() {
            *parts.last_mut().unwrap() = WordPart::ParamExpansion {
                name: vn,
                modifier: ParamModifier::None,
                quoted: vq,
                subscript: None,
                indirect: false,
            };
        }
    }
    Ok(())
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
        if let TokenKind::Word(w) = t.kind {
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
                push_quoted_span(chars, '\'', &mut buf, LexError::UnterminatedQuote)?;
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
        if let TokenKind::Word(w) = t.kind {
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

/// A valid POSIX parameter name: `[A-Za-z_][A-Za-z0-9_]*`, non-empty.
fn is_valid_param_name(s: &str) -> bool {
    let mut cs = s.chars();
    match cs.next() {
        Some(c) if c == '_' || c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    cs.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// Result of scanning a braced parameter NAME with `extquote` support.
enum NameScan {
    /// The assembled name. `decoded` is true if any `$'…'` run contributed
    /// (so the caller validates it as an identifier).
    Name { name: String, decoded: bool },
    /// A `$"…"` in name position — bash bad-substs it; the caller recovers.
    BadSubst,
}

/// Scans a braced parameter name, decoding any `$'…'` (ANSI-C) runs into the
/// name (bash `extquote`). `${a$'b'}` -> "ab". Stops at the first non-name,
/// non-`$'…'` char (leaving the cursor there for subscript/modifier scanning).
/// A `$"…"` (locale) run in name position returns `NameScan::BadSubst`.
fn scan_braced_name_ext(chars: &mut CharCursor<'_>) -> Result<NameScan, LexError> {
    let mut name = String::new();
    let mut decoded = false;
    loop {
        match chars.peek().copied() {
            Some(c) if c == '_' || c.is_ascii_alphanumeric() => {
                name.push(c);
                chars.next();
            }
            Some('$') => {
                // Look past `$` for `'` (ANSI-C, decode) / `"` (locale, bad-subst).
                let mut look = chars.clone();
                look.next();
                match look.peek().copied() {
                    Some('\'') => {
                        chars.next(); // `$`
                        chars.next(); // `'`
                        // ANSI-C span: `\` escapes the next char; closes on the
                        // first UNescaped `'`. Reuses the M4 span shape.
                        let mut body = String::new();
                        scan_raw_ansi_c_body(chars, &mut body, LexError::UnterminatedBrace)?;
                        name.push_str(&decode_ansi_c_escapes(&body));
                        decoded = true;
                    }
                    Some('"') => return Ok(NameScan::BadSubst),
                    _ => break, // a `$` not starting a quote ends the name run
                }
            }
            _ => break,
        }
    }
    Ok(NameScan::Name { name, decoded })
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
            let modifier = modifier_with_operand(chars, false, opts.with_in_dquote(quoted || opts.in_dquote), |w| ParamModifier::RemovePrefix { pattern: w, longest })?;
            parts.push(WordPart::ParamExpansion { name, modifier, quoted, subscript, indirect });
            Ok(())
        }
        Some('%') => {
            let longest = chars.peek() == Some(&'%');
            if longest { chars.next(); }
            let modifier = modifier_with_operand(chars, false, opts.with_in_dquote(quoted || opts.in_dquote), |w| ParamModifier::RemoveSuffix { pattern: w, longest })?;
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
            let (pattern, replacement) = scan_substitution_operand(chars, opts.with_in_dquote(quoted || opts.in_dquote))?;
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
            let pattern = scan_optional_braced_operand(chars, opts.with_in_dquote(quoted || opts.in_dquote))?;
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
            let pattern = scan_optional_braced_operand(chars, opts.with_in_dquote(quoted || opts.in_dquote))?;
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
///
/// `in_array_value` (v252 T2): true when scanning an ATOM-path array-literal
/// value. The oracle (`scan_array_element_word`) collects each element into a
/// bounded raw buffer that never includes the element's closing `)`, then
/// re-tokenizes that buffer standalone — so a trailing `~` there sees EOF, not
/// `)`. Our atom scanner runs directly against the live source cursor (no
/// bounded buffer), so it must be told to treat the array's closing `)` as an
/// end-of-word terminator too, or a value-final `~` (`a=(a=~)`) would see the
/// real `)` and wrongly fail to recognize the tilde.
fn try_parse_tilde(
    chars: &mut CharCursor<'_>,
    in_assignment_value: bool,
    in_array_value: bool,
) -> Option<TildeSpec> {
    let term = |c: char| is_tilde_terminator(c) || (in_assignment_value && c == ':') || (in_array_value && c == ')');
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
    fn char_cursor_tracks_offset_line_column() {
        let mut c = CharCursor::new("ab\ncé\td");
        // before consuming: at 'a'
        assert_eq!((c.offset(), c.line(), c.column()), (0, 1, 1));
        c.next();                       // consume 'a'
        assert_eq!((c.offset(), c.line(), c.column()), (1, 1, 2)); // at 'b'
        c.next();                       // consume 'b'
        assert_eq!((c.offset(), c.line(), c.column()), (2, 1, 3)); // at '\n'
        c.next();                       // consume '\n' -> next line, col resets
        assert_eq!((c.offset(), c.line(), c.column()), (3, 2, 1)); // at 'c'
        c.next();                       // consume 'c'
        assert_eq!((c.offset(), c.line(), c.column()), (4, 2, 2)); // at 'é' (2 bytes)
        c.next();                       // consume 'é' -> offset +2, column +1
        assert_eq!((c.offset(), c.line(), c.column()), (6, 2, 3)); // at '\t'
        c.next();                       // consume tab -> one column
        assert_eq!((c.offset(), c.line(), c.column()), (7, 2, 4)); // at 'd'
    }

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

    /// True iff `tokens` contains at least one `TokenKind::RedirFd`.
    fn has_redir_fd(tokens: &[Token]) -> bool {
        tokens.iter().any(|t| matches!(t.kind, TokenKind::RedirFd(_)))
    }

    #[test]
    fn lexer_fd_prefix_numeric() {
        // `echo 2>&1`: the `2` is whitespace-separated from `echo`, glued to the
        // operator — but the dedicated `2>` scanner fires (DupErr) and encodes
        // fd 2 in the operator itself, so NO RedirFd token is emitted here.
        // Use an fd with no dedicated scanner (`3>`) to exercise take_fd_prefix.
        let toks = tokenize("echo 3>file").unwrap();
        assert!(
            toks.iter().any(|t| matches!(&t.kind, TokenKind::RedirFd(RedirFd::Number(3)))),
            "expected RedirFd(Number(3)) in {toks:?}"
        );
        // And `echo 12>file` → fd 12.
        let toks = tokenize("echo 12>file").unwrap();
        assert!(
            toks.iter().any(|t| matches!(&t.kind, TokenKind::RedirFd(RedirFd::Number(12)))),
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
            toks.iter().any(|t| matches!(&t.kind, TokenKind::Word(w) if crate::command::word_literal_text(w) == Some("3"))),
            "expected Word(\"3\") arg in {toks:?}"
        );
    }

    #[test]
    fn lexer_fd_prefix_glued_word_is_not_prefix() {
        // `file2>x`: `file2` is not all-digits, so no RedirFd; `file2` stays a Word.
        let toks = tokenize("file2>x").unwrap();
        assert!(!has_redir_fd(&toks), "unexpected RedirFd in {toks:?}");
        assert!(
            toks.iter().any(|t| matches!(&t.kind, TokenKind::Word(w) if crate::command::word_literal_text(w) == Some("file2"))),
            "expected Word(\"file2\") in {toks:?}"
        );
    }

    #[test]
    fn lexer_named_fd_prefix() {
        // `exec {fd}>log`: `{fd}` glued to `>` → RedirFd::Var("fd").
        let toks = tokenize("exec {fd}>log").unwrap();
        assert!(
            toks.iter().any(|t| matches!(&t.kind, TokenKind::RedirFd(RedirFd::Var(name)) if name == "fd")),
            "expected RedirFd(Var(\"fd\")) in {toks:?}"
        );
    }

    #[test]
    fn lexer_readwrite_and_dupin_operators() {
        let toks = tokenize("cmd <>f").unwrap();
        assert!(toks.iter().any(|t| matches!(&t.kind, TokenKind::Op(Operator::RedirReadWrite))));
        let toks = tokenize("cmd 3<&0").unwrap();
        assert!(toks.iter().any(|t| matches!(&t.kind, TokenKind::RedirFd(RedirFd::Number(3)))));
        assert!(toks.iter().any(|t| matches!(&t.kind, TokenKind::Op(Operator::DupIn))));
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
        TokenKind::Word(Word(vec![WordPart::Literal { text: s.to_string(), quoted: false }])).into()
    }

    fn word_text(t: &Token) -> Option<String> {
        if let TokenKind::Word(Word(parts)) = &t.kind
            && parts.len() == 1
            && let WordPart::Literal { text, quoted: false } = &parts[0]
        {
            return Some(text.clone());
        }
        None
    }

    #[test]
    fn extglob_inside_command_sub_lexes() {
        let opts = LexerOptions { extglob: true, ..Default::default() };
        let toks = tokenize_with_opts("echo $(echo !(x))", opts).unwrap();
        assert!(toks.iter().any(|t| matches!(
            &t.kind, TokenKind::Word(Word(parts)) if parts.iter().any(|p| matches!(p, WordPart::CommandSub { .. }))
        )));
    }

    #[test]
    fn extglob_inside_backtick_sub_lexes() {
        let opts = LexerOptions { extglob: true, ..Default::default() };
        tokenize_with_opts("echo `echo !(x)`", opts).unwrap();
    }

    #[test]
    fn extglob_inside_array_literal_command_sub_lexes() {
        let opts = LexerOptions { extglob: true, ..Default::default() };
        tokenize_with_opts("a=($(printf '%s\\n' /tmp/!(x)))", opts).unwrap();
    }

    #[test]
    fn command_sub_without_extglob_still_errors_on_bare_extglob() {
        let opts = LexerOptions { extglob: false, ..Default::default() };
        assert!(tokenize_with_opts("echo $(echo !(x))", opts).is_err());
    }

    #[test]
    fn plain_command_sub_unchanged() {
        for eg in [false, true] {
            let opts = LexerOptions { extglob: eg, ..Default::default() };
            tokenize_with_opts("echo $(echo hi) $((1+1))", opts).unwrap();
        }
    }

    #[test]
    fn dbracket_regex_paren_operand_is_one_word() {
        let toks = tokenize("[[ x =~ (a) ]]").unwrap();
        let texts: Vec<_> = toks.iter().filter_map(word_text).collect();
        assert_eq!(texts, vec!["[[", "x", "=~", "(a)", "]]"]);
        assert!(!toks.iter().any(|t| matches!(&t.kind, TokenKind::Op(Operator::LParen) | TokenKind::ArithBlock(..))));
    }

    #[test]
    fn dbracket_regex_double_paren_not_arithblock() {
        let toks = tokenize("[[ ab =~ ((a)) ]]").unwrap();
        let texts: Vec<_> = toks.iter().filter_map(word_text).collect();
        assert_eq!(texts, vec!["[[", "ab", "=~", "((a))", "]]"]);
        assert!(!toks.iter().any(|t| matches!(&t.kind, TokenKind::ArithBlock(..))));
    }

    #[test]
    fn dbracket_regex_line847_shape() {
        let toks = tokenize(r"[[ $option =~ (\[((no|dont)-?)\]). ]]").unwrap();
        let texts: Vec<_> = toks.iter().filter_map(word_text).collect();
        assert!(texts.iter().any(|t| t.starts_with("(\\[")));
        assert!(texts.contains(&"]]".to_string()));
        assert!(!toks.iter().any(|t| matches!(&t.kind, TokenKind::ArithBlock(..))));
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
        assert!(!toks.iter().any(|t| matches!(&t.kind, TokenKind::ArithBlock(..) | TokenKind::Op(Operator::LParen))));
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
        assert!(toks.iter().any(|t| matches!(&t.kind, TokenKind::Op(Operator::LParen))));
        assert!(toks.iter().any(|t| matches!(&t.kind, TokenKind::Op(Operator::RParen))));
    }

    #[test]
    fn arith_block_outside_dbracket_unchanged() {
        let toks = tokenize("(( 1 + 1 ))").unwrap();
        assert!(toks.iter().any(|t| matches!(&t.kind, TokenKind::ArithBlock(..))));
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
    fn span_offsets_align_with_token_starts() {
        // "echo hi\nls" -> Word(echo)@0 Word(hi)@5 Newline@7 Word(ls)@8
        let toks = tokenize("echo hi\nls").unwrap();
        assert_eq!(toks[0].span.offset, 0);
        let nl = toks.iter().position(|t| matches!(&t.kind, TokenKind::Newline)).unwrap();
        assert_eq!(toks[nl].span.offset, 7);
        assert_eq!(toks[nl + 1].span.offset, 8);
    }

    /// Each token's recorded span offset must point at the byte where that
    /// token's source actually begins (verified by re-deriving from a
    /// distinctive first character).
    #[test]
    fn span_offsets_point_at_real_token_starts() {
        // Leading whitespace before the first word; operators; a quoted word.
        //          0123456789012345678901
        let src = "  echo a && ls 'x y'\n";
        let toks = tokenize_with_opts(src, LexerOptions::default()).unwrap();
        // First word "echo" starts at byte 2 (after two spaces).
        assert_eq!(toks[0].span.offset, 2);
        // The `&&` operator starts at byte 9.
        let and = toks.iter().position(|t| matches!(&t.kind, TokenKind::Op(Operator::And))).unwrap();
        assert_eq!(toks[and].span.offset, 9);
        assert_eq!(&src[toks[and].span.offset..toks[and].span.offset + 2], "&&");
        // The quoted word 'x y' is the second-to-last token (before trailing Newline).
        let q = toks.len() - 2;
        assert_eq!(&src[toks[q].span.offset..toks[q].span.offset + 1], "'");
        // Trailing Newline at byte 20.
        assert_eq!(toks.last().unwrap().span.offset, 20);
        // Offsets are non-decreasing and in range.
        for w in toks.windows(2) {
            assert!(w[0].span.offset <= w[1].span.offset);
        }
        assert!(toks.iter().all(|t| t.span.offset <= src.len()));
    }

    /// A token's span carries its 1-based source line directly (the line-lookup
    /// the reader used to compute by counting newlines up to an offset).
    #[test]
    fn span_carries_source_line() {
        let src = "echo a\necho b\nbad)\n";
        let toks = tokenize(src).unwrap();
        // The `)` operator is on line 3.
        let rp = toks.iter().position(|t| matches!(&t.kind, TokenKind::Op(Operator::RParen))).unwrap();
        assert_eq!(toks[rp].span.line, 3);
    }

    #[test]
    fn partial_error_returns_failure_position() {
        let (_toks, err) = tokenize_partial("echo 'oops", LexerOptions::default());
        let (_e, off) = err.expect("unterminated quote should error");
        assert!(off >= 5, "failure offset {off} should be at/after the open quote");
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
        let toks = tokenize_with_opts("+(a|b)", LexerOptions { extglob: true, ..Default::default() }).unwrap();
        assert_eq!(toks.len(), 1, "expected one Word token, got {toks:?}");
        assert!(matches!(&toks[0].kind, TokenKind::Word(_)));
    }

    #[test]
    fn span_columns_are_1based_char_columns_reset_per_line() {
        // 1-based CHARACTER columns (Unicode scalars; tab counts as 1) captured at
        // each token's first char, reset to 1 after a newline. Built at lex time
        // from the CharCursor — no offsets/lines sidecar, no zip pass.
        //          col: 1234567 8 12345
        let src = "echo  hi\nαβ x";
        let toks = tokenize(src).unwrap();
        // "echo" starts at column 1.
        assert_eq!(toks[0].span.column, 1);
        // "hi" follows two spaces after "echo " -> column 7.
        assert_eq!(toks[1].span.column, 7);
        // Newline itself sits at column 9 (after "echo  hi").
        let nl = toks.iter().position(|t| matches!(&t.kind, TokenKind::Newline)).unwrap();
        assert_eq!(toks[nl].span.column, 9);
        // After the newline, "αβ" starts at column 1 (two scalars, not bytes).
        assert_eq!(toks[nl + 1].span.column, 1);
        // "x" is one scalar ('α','β') + one space past column 1 -> column 4.
        assert_eq!(toks[nl + 2].span.column, 4);
        assert_eq!(toks[nl + 2].span.line, 2);
    }

    // ---- v238: direct next_token (incremental pull) API ----

    fn drain(input: &str) -> Vec<Token> {
        let mut lx = Lexer::new(input, LexerOptions::default(), true);
        let mut v = Vec::new();
        while let Some(t) = lx.next_token().expect("lex") {
            v.push(t);
        }
        v
    }

    #[test]
    fn next_token_yields_each_token_in_order() {
        // Repeated single-token reads return the exact ordered stream.
        let mut lx = Lexer::new("echo foo | grep bar", LexerOptions::default(), true);
        assert_eq!(lx.next_token().unwrap().unwrap(), w("echo"));
        assert_eq!(lx.next_token().unwrap().unwrap(), w("foo"));
        assert_eq!(lx.next_token().unwrap().unwrap().kind, TokenKind::Op(Operator::Pipe));
        assert_eq!(lx.next_token().unwrap().unwrap(), w("grep"));
        assert_eq!(lx.next_token().unwrap().unwrap(), w("bar"));
        assert!(lx.next_token().unwrap().is_none());
    }

    #[test]
    fn mode_stack_push_pop_current() {
        let mut lx = Lexer::new("echo hi", LexerOptions::default(), true);
        assert_eq!(lx.current_mode(), Mode::Command);
        lx.push_mode(Mode::Arith { paren_depth: 0, in_squote: false, in_dquote: false, body_started: false, for_header: false, delim: ArithDelim::Paren });
        assert_eq!(lx.current_mode(), Mode::Arith { paren_depth: 0, in_squote: false, in_dquote: false, body_started: false, for_header: false, delim: ArithDelim::Paren });
        lx.push_mode(Mode::CommandSub { body_started: false });
        assert_eq!(lx.current_mode(), Mode::CommandSub { body_started: false });
        assert_eq!(lx.pop_mode(), Mode::CommandSub { body_started: false });
        assert_eq!(lx.pop_mode(), Mode::Arith { paren_depth: 0, in_squote: false, in_dquote: false, body_started: false, for_header: false, delim: ArithDelim::Paren });
        assert_eq!(lx.current_mode(), Mode::Command);
    }

    #[test]
    fn arith_for_header_emits_arith_semi() {
        // Drive Mode::Arith with for_header=true over a for-header body (no `((`
        // prefix — body_started=true starts at the body loop). Top-level `;`
        // must emit ArithSemi; a `;` nested in `()` stays literal.
        let mut lx = Lexer::new_live_atoms("i=0;i<3;i++))", &Default::default(), LexerOptions::default());
        lx.push_mode(Mode::Arith { paren_depth: 0, in_squote: false, in_dquote: false, body_started: true, for_header: true, delim: ArithDelim::Paren });
        let mut kinds = Vec::new();
        loop {
            match lx.next_kind().expect("lex") {
                Some(TokenKind::ArithClose) => { kinds.push(TokenKind::ArithClose); break; }
                Some(k) => kinds.push(k),
                None => break,
            }
        }
        assert_eq!(kinds, vec![
            TokenKind::Lit { text: "i=0".into(), quoted: true },
            TokenKind::ArithSemi,
            TokenKind::Lit { text: "i<3".into(), quoted: true },
            TokenKind::ArithSemi,
            TokenKind::Lit { text: "i++".into(), quoted: true },
            TokenKind::ArithClose,
        ]);

        // Nested `;` (depth>0) stays literal — one Lit, no ArithSemi.
        let mut lx2 = Lexer::new_live_atoms("(a;b)))", &Default::default(), LexerOptions::default());
        lx2.push_mode(Mode::Arith { paren_depth: 0, in_squote: false, in_dquote: false, body_started: true, for_header: true, delim: ArithDelim::Paren });
        let mut kinds2 = Vec::new();
        loop {
            match lx2.next_kind().expect("lex") {
                Some(TokenKind::ArithClose) => { kinds2.push(TokenKind::ArithClose); break; }
                Some(k) => kinds2.push(k),
                None => break,
            }
        }
        assert_eq!(kinds2, vec![
            TokenKind::Lit { text: "(a;b)".into(), quoted: true },
            TokenKind::ArithClose,
        ]);
    }

    #[test]
    fn arith_bracket_mode_scans_legacy_arith() {
        // A Mode::Arith{delim:Bracket} body: `$[a[0]+1]` → LegacyArithOpen, the
        // body Lit "a[0]+1" (inner [0] bracket-nested), then ArithClose.
        let mut lx = Lexer::new_live_atoms("$[a[0]+1]", &Default::default(), LexerOptions::default());
        lx.push_mode(Mode::Arith { paren_depth: 0, in_squote: false, in_dquote: false, body_started: false, for_header: false, delim: ArithDelim::Bracket });
        let mut kinds = Vec::new();
        loop {
            match lx.next_kind().expect("lex") {
                Some(TokenKind::ArithClose) => { kinds.push(TokenKind::ArithClose); break; }
                Some(k) => kinds.push(k),
                None => break,
            }
        }
        assert_eq!(kinds.first(), Some(&TokenKind::LegacyArithOpen));
        assert!(kinds.iter().any(|k| matches!(k, TokenKind::Lit { text, .. } if text == "a[0]+1")),
            "expected body Lit \"a[0]+1\", got {kinds:?}");
        assert_eq!(kinds.last(), Some(&TokenKind::ArithClose));
    }

    #[test]
    fn char_cursor_peek_nth_does_not_advance() {
        // peek_nth must not consume any characters — the cursor position must be
        // identical before and after the call.  Uses the CharCursor directly.
        let mut cur = crate::lexer::CharCursor::new("abc");
        // Pre-fill the peeked slot (mirrors normal lexer usage where peek() was called).
        let p0 = cur.peek().copied();
        assert_eq!(p0, Some('a'), "peek() should return 'a'");

        // peek_nth(0) == first char ('a'), same as peek() — no advancement.
        assert_eq!(cur.peek_nth(0), Some('a'), "peek_nth(0) should be 'a'");
        // peek_nth(1) looks one further ahead.
        assert_eq!(cur.peek_nth(1), Some('b'), "peek_nth(1) should be 'b'");
        assert_eq!(cur.peek_nth(2), Some('c'), "peek_nth(2) should be 'c'");
        // Past-end returns None.
        assert_eq!(cur.peek_nth(3), None, "peek_nth(3) past end should be None");

        // The cursor has not advanced — next() must still yield 'a'.
        assert_eq!(cur.next(), Some('a'), "cursor should not have advanced");
        // Now first unconsumed char is 'b'; peek_nth(0) returns 'b'.
        assert_eq!(cur.peek_nth(0), Some('b'), "after consuming 'a', peek_nth(0) should be 'b'");
    }

    #[test]
    fn next_token_drain_equals_tokenize() {
        // Draining the pull API must equal the batch tokenize() byte-for-byte.
        for src in [
            "echo hi",
            "a 'b c' d",
            "x=\"a${y}b\"",
            "echo ${x:-def}",
            "v=$(cmd arg)",
            "n=$((1 + 2))",
            "echo `date`",
            "[[ $x =~ ^a.*z$ ]]",
            "a{1,2,3}b",
            "cat 2>&1",
            "one\ntwo\nthree",
            "cat <<EOF\nline1\nline2\nEOF\n",
        ] {
            assert_eq!(drain(src), tokenize(src).unwrap(), "stream != batch for {src:?}");
        }
    }

    #[test]
    fn next_token_is_lazy_not_greedy() {
        // A long multi-token input: after the FIRST token the cursor must be near
        // the start, NOT at EOF. A greedy implementation that tokenized everything
        // up front would leave the cursor at EOF here and invalidate the whole
        // incremental design — this test fails loudly in that case.
        let words: Vec<String> = (0..100).map(|i| format!("w{i}")).collect();
        let src = words.join(" ");
        let mut lx = Lexer::new(&src, LexerOptions::default(), true);
        let first = lx.next_token().unwrap().unwrap();
        assert_eq!(word_text(&first).as_deref(), Some("w0"));
        let off = lx.cursor_offset();
        assert!(off <= 3, "cursor advanced too far after first token: {off} (input len {})", src.len());
        assert!(off < src.len(), "cursor jumped to EOF after first token (greedy consumption!)");
    }

    #[test]
    fn next_token_cursor_tracks_consumed_input() {
        // After handing out token i (space-separated words), the char cursor sits
        // at EXACTLY the next token's start — it consumed token i and its single
        // delimiter and nothing more. Pins down "the char stream is at the correct
        // location" with no greedy over-consumption.
        let src = "alpha beta gamma delta";
        let starts: Vec<usize> = tokenize(src).unwrap().iter().map(|t| t.span.offset).collect();
        let mut lx = Lexer::new(src, LexerOptions::default(), true);
        for i in 0..starts.len() {
            let _ = lx.next_token().unwrap().unwrap();
            let expected = if i + 1 < starts.len() { starts[i + 1] } else { src.len() };
            assert_eq!(lx.cursor_offset(), expected, "after token {i}, cursor should be at {expected}");
        }
        assert!(lx.next_token().unwrap().is_none());
        assert_eq!(lx.cursor_offset(), src.len());
    }

    #[test]
    fn next_token_brace_expansion_drains_one_at_a_time() {
        // One source unit -> N tokens, drained across successive next_token calls.
        let mut lx = Lexer::new("a{1,2,3}b", LexerOptions::default(), true);
        let a = lx.next_token().unwrap().unwrap();
        let b = lx.next_token().unwrap().unwrap();
        let c = lx.next_token().unwrap().unwrap();
        assert!(lx.next_token().unwrap().is_none());
        assert_eq!(
            (word_text(&a), word_text(&b), word_text(&c)),
            (Some("a1b".into()), Some("a2b".into()), Some("a3b".into()))
        );
    }

    #[test]
    fn next_token_heredoc_body_complete_when_emitted() {
        // The Heredoc token handed out by next_token must already carry its full
        // body (readiness/stall rule): an early hand-out would yield an empty body.
        let toks = drain("cat <<EOF; echo hi\nbody1\nbody2\nEOF\n");
        let body = heredoc_body(&toks);
        assert!(!body.0.is_empty(), "heredoc body was empty — token handed out before backfill");
    }

    #[test]
    fn next_token_partial_error_matches_tokenize_partial() {
        // A mid-stream lex error drained via next_token returns the same
        // tokens-so-far and the same error byte offset as tokenize_partial.
        let src = "echo ok \"unterminated";
        let (batch_tokens, batch_err) = tokenize_partial(src, LexerOptions::default());
        let mut lx = Lexer::new(src, LexerOptions::default(), true);
        let mut stream = Vec::new();
        let mut stream_err = None;
        loop {
            match lx.next_token() {
                Ok(Some(t)) => stream.push(t),
                Ok(None) => break,
                Err(e) => {
                    stream_err = Some((e, lx.cursor_offset()));
                    break;
                }
            }
        }
        assert_eq!(stream, batch_tokens);
        assert_eq!(stream_err.map(|(_, o)| o), batch_err.map(|(_, o)| o));
    }

    #[test]
    fn next_token_partial_unterminated_heredoc_keeps_buffered_tokens() {
        // Unterminated heredoc: the readiness rule buffers the placeholder Heredoc
        // (and trailing same-line tokens) during normal reads, but on the terminal
        // error path tokenize_partial must still surface them — byte-identical to
        // the batch lexer's partial set. Locks the error-path flush in
        // tokenize_partial_inner.
        let src = "cat <<EOF; echo hi";
        let (toks, err) = tokenize_partial(src, LexerOptions::default());
        assert!(matches!(err, Some((LexError::UnterminatedHeredoc, _))), "err: {err:?}");
        // cat, Heredoc(placeholder), ;, echo, hi
        assert_eq!(toks.len(), 5, "partial set should keep the buffered heredoc + trailing: {toks:?}");
        assert_eq!(word_text(&toks[0]).as_deref(), Some("cat"));
        assert!(matches!(&toks[1].kind, TokenKind::Heredoc { .. }), "toks[1] should be the Heredoc placeholder: {:?}", toks[1]);
        assert_eq!(word_text(&toks[4]).as_deref(), Some("hi"));
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
            let toks = tokenize_with_opts(p, LexerOptions { extglob: true, ..Default::default() }).unwrap();
            assert_eq!(toks.len(), 1, "{p} should be one word, got {toks:?}");
        }
    }

    #[test]
    fn extglob_group_preserves_inner_expansion() {
        // `+($x)` must NOT collapse to a single flat literal — the `$x`
        // inside the group has to survive as a Param part so it expands.
        let toks = tokenize_with_opts("+($x)", LexerOptions { extglob: true, ..Default::default() }).unwrap();
        assert_eq!(toks.len(), 1, "expected one Word token, got {toks:?}");
        let TokenKind::Word(Word(parts)) = &toks[0].kind else {
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
        TokenKind::Word(Word(vec![qsingle(s)])).into()
    }
    /// A double-quoted word with a single literal body: `"s"`.
    fn wqd(s: &str) -> Token {
        TokenKind::Word(Word(vec![qdouble(vec![WordPart::Literal {
            text: s.to_string(),
            quoted: true,
        }])])).into()
    }
    /// A backslash-escaped single char as a word: `\s`.
    fn wqb(s: &str) -> Token {
        TokenKind::Word(Word(vec![qbackslash(s)])).into()
    }
    /// An ANSI-C quoted word: `$'s'` (s already decoded).
    fn wqa(s: &str) -> Token {
        TokenKind::Word(Word(vec![WordPart::Quoted {
            style: QuoteStyle::AnsiC,
            parts: vec![WordPart::Literal { text: s.to_string(), quoted: true }],
        }])).into()
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
        let word = match tokens.remove(0).kind {
            TokenKind::Word(w) => w,
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
            vec![TokenKind::Newline.into(), w("echo"), w("hi")]
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
            vec![w("echo"), w("a"), TokenKind::Newline.into(), w("echo"), w("b")]
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
            vec![w("echo"), w("a"), TokenKind::Op(Operator::Semi).into()]
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
            vec![w("echo"), TokenKind::Word(Word(vec![
                qbackslash("#"),
                WordPart::Literal { text: "hash".to_string(), quoted: false },
            ])).into()]
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
                TokenKind::Word(Word(vec![
                    WordPart::Literal { text: "a".to_string(), quoted: false },
                    qbackslash(" "),
                    WordPart::Literal { text: "b".to_string(), quoted: false },
                ])).into(),
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
        let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!() };
        assert_eq!(parts, &[qbackslash("*")]);
    }

    #[test]
    fn backslash_in_middle_of_word_flushes_and_quotes() {
        // `foo\*bar` → unquoted "foo", quoted "*", unquoted "bar"
        let tokens = tokenize("foo\\*bar").unwrap();
        let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!() };
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
            vec![TokenKind::Word(Word(vec![
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
            vec![w("a"), TokenKind::Op(Operator::Pipe).into(), w("b")]
        );
    }

    #[test]
    fn tokenize_pipe_without_spaces() {
        assert_eq!(
            tokenize("a|b").unwrap(),
            vec![w("a"), TokenKind::Op(Operator::Pipe).into(), w("b")]
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
                TokenKind::RedirFd(crate::command::RedirFd::Number(2)).into(),
                TokenKind::Op(Operator::DupOut).into(),
                w("1"),
                TokenKind::Op(Operator::Pipe).into(),
                w("b"),
            ]
        );
    }

    #[test]
    fn tokenize_redirect_out() {
        assert_eq!(
            tokenize("ls > f").unwrap(),
            vec![w("ls"), TokenKind::Op(Operator::RedirOut).into(), w("f")]
        );
    }

    #[test]
    fn tokenize_redirect_out_without_spaces() {
        assert_eq!(
            tokenize("ls>f").unwrap(),
            vec![w("ls"), TokenKind::Op(Operator::RedirOut).into(), w("f")]
        );
    }

    #[test]
    fn tokenize_redirect_append() {
        assert_eq!(
            tokenize("ls >> f").unwrap(),
            vec![w("ls"), TokenKind::Op(Operator::RedirAppend).into(), w("f")]
        );
    }

    #[test]
    fn tokenize_redirect_in() {
        assert_eq!(
            tokenize("cat < f").unwrap(),
            vec![w("cat"), TokenKind::Op(Operator::RedirIn).into(), w("f")]
        );
    }

    #[test]
    fn tokenize_redirect_stderr() {
        assert_eq!(
            tokenize("cmd 2> f").unwrap(),
            vec![w("cmd"), TokenKind::Op(Operator::RedirErr).into(), w("f")]
        );
    }

    #[test]
    fn tokenize_redirect_stderr_append() {
        assert_eq!(
            tokenize("cmd 2>> f").unwrap(),
            vec![w("cmd"), TokenKind::Op(Operator::RedirErrAppend).into(), w("f")]
        );
    }

    #[test]
    fn tokenize_two_in_word_is_not_stderr_operator() {
        assert_eq!(
            tokenize("x2>f").unwrap(),
            vec![w("x2"), TokenKind::Op(Operator::RedirOut).into(), w("f")]
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
                TokenKind::Op(Operator::RedirIn).into(),
                w("in"),
                TokenKind::Op(Operator::Pipe).into(),
                w("b"),
                TokenKind::Op(Operator::RedirOut).into(),
                w("out"),
            ]
        );
    }

    #[test]
    fn tokenize_or_with_spaces() {
        assert_eq!(
            tokenize("a || b").unwrap(),
            vec![w("a"), TokenKind::Op(Operator::Or).into(), w("b")]
        );
    }

    #[test]
    fn tokenize_or_without_spaces() {
        assert_eq!(
            tokenize("a||b").unwrap(),
            vec![w("a"), TokenKind::Op(Operator::Or).into(), w("b")]
        );
    }

    #[test]
    fn tokenize_and_with_spaces() {
        assert_eq!(
            tokenize("a && b").unwrap(),
            vec![w("a"), TokenKind::Op(Operator::And).into(), w("b")]
        );
    }

    #[test]
    fn tokenize_and_without_spaces() {
        assert_eq!(
            tokenize("a&&b").unwrap(),
            vec![w("a"), TokenKind::Op(Operator::And).into(), w("b")]
        );
    }

    #[test]
    fn tokenize_bare_ampersand_is_background_op() {
        assert_eq!(
            tokenize("a & b").unwrap(),
            vec![w("a"), TokenKind::Op(Operator::Background).into(), w("b")]
        );
    }

    #[test]
    fn tokenize_bare_ampersand_at_end_is_background_op() {
        assert_eq!(
            tokenize("a &").unwrap(),
            vec![w("a"), TokenKind::Op(Operator::Background).into()]
        );
    }

    #[test]
    fn tokenize_double_ampersand_still_and_op() {
        assert_eq!(
            tokenize("a && b").unwrap(),
            vec![w("a"), TokenKind::Op(Operator::And).into(), w("b")]
        );
    }

    #[test]
    fn tokenize_two_separate_backgrounds() {
        assert_eq!(
            tokenize("a & &").unwrap(),
            vec![
                w("a"),
                TokenKind::Op(Operator::Background).into(),
                TokenKind::Op(Operator::Background).into(),
            ]
        );
    }

    #[test]
    fn tokenize_semicolon_with_spaces() {
        assert_eq!(
            tokenize("a ; b").unwrap(),
            vec![w("a"), TokenKind::Op(Operator::Semi).into(), w("b")]
        );
    }

    #[test]
    fn tokenize_semicolon_without_spaces() {
        assert_eq!(
            tokenize("a;b").unwrap(),
            vec![w("a"), TokenKind::Op(Operator::Semi).into(), w("b")]
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
            TokenKind::Word(Word(vec![qbackslash(a), qbackslash(b)])).into()
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
                TokenKind::Op(Operator::And).into(),
                w("b"),
                TokenKind::Op(Operator::Or).into(),
                w("c"),
                TokenKind::Op(Operator::Semi).into(),
                w("d"),
            ]
        );
    }

    fn vword_unquoted(name: &str) -> Token {
        TokenKind::Word(Word(vec![WordPart::Var {
            name: name.to_string(),
            quoted: false,
        }])).into()
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
            vec![TokenKind::Word(Word(vec![qdouble(vec![WordPart::Var {
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
        let TokenKind::Word(Word(parts)) = &toks[0].kind else { panic!("not a word: {toks:?}") };
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
            vec![TokenKind::Word(Word(vec![WordPart::LastStatus {
                quoted: false
            }]))]
        );
    }

    #[test]
    fn tokenize_dollar_then_digit_is_positional_param() {
        // Since v22 Task 4: $<digit> is a positional parameter, not a literal $.
        assert_eq!(
            tokenize("$5").unwrap(),
            vec![TokenKind::Word(Word(vec![
                WordPart::Var { name: "5".to_string(), quoted: false },
            ]))]
        );
    }

    #[test]
    fn tokenize_double_dollar_is_var_name_dollar() {
        // v26: $$ is the shell PID special parameter, not two literal dollars.
        assert_eq!(
            tokenize("$$").unwrap(),
            vec![TokenKind::Word(Word(vec![
                WordPart::Var { name: "$".to_string(), quoted: false },
            ]))]
        );
    }

    #[test]
    fn tokenize_tilde_alone() {
        assert_eq!(
            tokenize("~").unwrap(),
            vec![TokenKind::Word(Word(vec![WordPart::Tilde(TildeSpec::Home)]))]
        );
    }

    #[test]
    fn tokenize_tilde_slash_path() {
        assert_eq!(
            tokenize("~/foo").unwrap(),
            vec![TokenKind::Word(Word(vec![
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
            vec![TokenKind::Word(Word(vec![
                WordPart::Tilde(TildeSpec::User("foo".to_string())),
            ]))]
        );
    }

    #[test]
    fn tokenize_tilde_user_alone() {
        assert_eq!(
            tokenize("~alice").unwrap(),
            vec![TokenKind::Word(Word(vec![
                WordPart::Tilde(TildeSpec::User("alice".to_string())),
            ]))]
        );
    }

    #[test]
    fn tokenize_tilde_user_slash_path() {
        assert_eq!(
            tokenize("~alice/bin").unwrap(),
            vec![TokenKind::Word(Word(vec![
                WordPart::Tilde(TildeSpec::User("alice".to_string())),
                WordPart::Literal { text: "/bin".to_string(), quoted: false },
            ]))]
        );
    }

    #[test]
    fn tokenize_tilde_user_with_underscore_and_digits() {
        assert_eq!(
            tokenize("~alice_123").unwrap(),
            vec![TokenKind::Word(Word(vec![
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
        let TokenKind::Word(Word(parts)) = &toks[0].kind else { panic!() };
        assert!(matches!(&parts[0],
            WordPart::ParamExpansion { modifier: ParamModifier::BadSubst { raw }, .. }
            if raw == "${1foo}"
        ), "expected BadSubst, got {:?}", parts[0]);
    }

    #[test]
    fn tokenize_braced_var_empty_name() {
        // v233: `${}` is lexable-but-invalid → BadSubst at runtime, not a parse error.
        let toks = tokenize("${}").unwrap();
        let TokenKind::Word(Word(parts)) = &toks[0].kind else { panic!() };
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
            vec![TokenKind::Word(Word(vec![
                WordPart::Literal { text: "a".to_string(), quoted: false },
                WordPart::Var { name: "FOOb".to_string(), quoted: false },
            ]))]
        );
    }

    #[test]
    fn tokenize_braced_var_separates_from_following_word() {
        assert_eq!(
            tokenize("${FOO}bar").unwrap(),
            vec![TokenKind::Word(Word(vec![
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
            vec![TokenKind::Word(Word(vec![
                WordPart::Var { name: "FOO".to_string(), quoted: false },
                WordPart::Var { name: "BAR".to_string(), quoted: false },
            ]))]
        );
    }

    fn sub_word(parts: Vec<WordPart>) -> Token {
        TokenKind::Word(Word(parts)).into()
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
        match &tokens[0].kind {
            TokenKind::Word(Word(parts)) => {
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
        assert!(matches!(&toks[0].kind, TokenKind::Word(Word(p)) if matches!(&p[0], WordPart::CommandSub { .. })));
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
        match &tokens[0].kind {
            TokenKind::Word(Word(parts)) => {
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
        match &tokens[0].kind {
            TokenKind::Word(Word(parts)) => {
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
        assert_eq!(tokens[1], TokenKind::Op(Operator::RedirOut));
        match &tokens[2].kind {
            TokenKind::Word(Word(parts)) => {
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
        match &tokens[0].kind {
            TokenKind::Word(Word(parts)) => {
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
        match &tokens[0].kind {
            TokenKind::Word(Word(parts)) => match &parts[0] {
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
        match &tokens[0].kind {
            TokenKind::Word(Word(parts)) => match &parts[0] {
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
            vec![TokenKind::Word(Word(vec![WordPart::Tilde(TildeSpec::Pwd)]))]
        );
    }

    #[test]
    fn tokenize_tilde_minus_alone() {
        assert_eq!(
            tokenize("~-").unwrap(),
            vec![TokenKind::Word(Word(vec![WordPart::Tilde(TildeSpec::OldPwd)]))]
        );
    }

    #[test]
    fn tokenize_tilde_plus_slash_path() {
        assert_eq!(
            tokenize("~+/x").unwrap(),
            vec![TokenKind::Word(Word(vec![
                WordPart::Tilde(TildeSpec::Pwd),
                WordPart::Literal { text: "/x".to_string(), quoted: false },
            ]))]
        );
    }

    #[test]
    fn tokenize_tilde_minus_slash_path() {
        assert_eq!(
            tokenize("~-/x").unwrap(),
            vec![TokenKind::Word(Word(vec![
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
            vec![TokenKind::Word(Word(vec![
                WordPart::Literal { text: "X=".to_string(), quoted: false },
                WordPart::Tilde(TildeSpec::Home),
            ]))]
        );
    }

    #[test]
    fn tokenize_assignment_value_expands_first_tilde_after_equals() {
        assert_eq!(
            tokenize("PATH=~/bin").unwrap(),
            vec![TokenKind::Word(Word(vec![
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
            vec![TokenKind::Word(Word(vec![
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
        let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!() };
        // Expect quoted "F", unquoted "OO=bar" — no assignment split.
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0], qdouble(vec![WordPart::Literal { text: "F".to_string(), quoted: true }]));
        assert_eq!(parts[1], WordPart::Literal { text: "OO=bar".to_string(), quoted: false });
    }

    #[test]
    fn tokenize_assignment_value_tilde_user() {
        assert_eq!(
            tokenize("HOMES=~alice:~bob").unwrap(),
            vec![TokenKind::Word(Word(vec![
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
        let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!() };
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
        let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!() };
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
        let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!() };
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
        let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!() };
        assert_eq!(parts.len(), 1);
        assert_eq!(arith_body_lit(&parts[0]), "a[1]+1");
    }

    #[test]
    fn tokenize_legacy_arith_aware_close() {
        for src in ["$[ $(echo ']')+1 ]", "$[ \"x]\" + 1 ]"] {
            let tokens = tokenize(src).unwrap_or_else(|e| panic!("{src}: {e:?}"));
            assert_eq!(tokens.len(), 1, "{src} closed early: {tokens:?}");
            let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!("{src}") };
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
        let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!() };
        assert_eq!(parts.len(), 1);
        assert!(matches!(parts[0], WordPart::Arith { .. }), "got {:?}", parts[0]);
    }

    #[test]
    fn tokenize_legacy_arith_inside_dquote() {
        // `"$[1+2]"` — the $[ arm must carry quoted: true through to WordPart::Arith.
        let tokens = tokenize("\"$[1+2]\"").unwrap();
        assert_eq!(tokens.len(), 1);
        let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!() };
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
        assert!(arith_string_to_word(body, LexerOptions { extglob: true, ..Default::default() }).is_ok());
        assert!(arith_string_to_word(body, LexerOptions { extglob: false, ..Default::default() }).is_err());
    }

    #[test]
    fn tokenize_arith_with_nested_parens() {
        let tokens = tokenize("$(( (1+2) * 3 ))").unwrap();
        let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!() };
        assert_eq!(arith_body_lit(&parts[0]), " (1+2) * 3 ");
    }

    #[test]
    fn tokenize_arith_inside_double_quotes_is_quoted() {
        let tokens = tokenize("\"$((1+2))\"").unwrap();
        let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!() };
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
        let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!() };
        assert!(matches!(parts[0], WordPart::Arith { .. }));
        assert_eq!(arith_body_lit(&parts[0]), "1+");
    }

    #[test]
    fn tokenize_arith_and_command_sub_both_recognized() {
        let tokens = tokenize("$((1)) $(echo x)").unwrap();
        let TokenKind::Word(Word(parts1)) = &tokens[0].kind else { panic!() };
        assert!(matches!(parts1[0], WordPart::Arith { .. }));
        let TokenKind::Word(Word(parts2)) = &tokens[1].kind else { panic!() };
        assert!(matches!(parts2[0], WordPart::CommandSub { .. }));
    }

    #[test]
    fn tokenize_arith_var_with_dollar_prefix_inside() {
        // Post-v93: `$x` inside `$(())` is now an expandable Var body part
        // (expanded before arith parse), not a pre-parsed ArithExpr::Var.
        let tokens = tokenize("$(($x))").unwrap();
        let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!() };
        let WordPart::Arith { body: Word(bparts), .. } = &parts[0] else { panic!() };
        assert_eq!(bparts.len(), 1);
        assert_eq!(bparts[0], WordPart::Var { name: "x".to_string(), quoted: true });
    }

    #[test]
    fn tokenize_arith_back_to_back_in_same_word() {
        let tokens = tokenize("$((1+2))$((3+4))").unwrap();
        assert_eq!(tokens.len(), 1);
        let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!() };
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
        let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!() };
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
        let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!() };
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0], WordPart::Var { name: "foo".to_string(), quoted: false });
    }

    #[test]
    fn tokenize_length_modifier() {
        let tokens = tokenize("${#foo}").unwrap();
        let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!() };
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
        let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!() };
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
        let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!() };
        let WordPart::ParamExpansion { modifier, .. } = &parts[0] else { panic!() };
        assert!(matches!(modifier, ParamModifier::UseDefault { colon: false, .. }));
    }

    #[test]
    fn tokenize_indirect_bare() {
        // `${!ref}` — v95 indirect scalar expansion, no modifier.
        let tokens = tokenize("${!ref}").unwrap();
        let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!() };
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
        let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!() };
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
        let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!() };
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
        let TokenKind::Word(Word(parts)) = &toks[0].kind else { panic!() };
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
        let TokenKind::Word(Word(parts)) = &toks[0].kind else { panic!() };
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
        let TokenKind::Word(Word(parts)) = &toks[0].kind else { panic!() };
        let WordPart::ParamExpansion { modifier, .. } = &parts[0] else { panic!() };
        assert!(matches!(modifier, ParamModifier::IndirectKeys));
    }

    #[test]
    fn tokenize_prefix_names_star() {
        // `${!pfx*}` — prefix-name expansion, `*` form (at=false).
        let tokens = tokenize("${!_Q*}").unwrap();
        let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!() };
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
        let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!() };
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
        let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!() };
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
        let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!() };
        let WordPart::Var { name, .. } = &parts[0]
        else { panic!("expected Var, got {:?}", parts[0]) };
        assert_eq!(name, "-");
    }

    #[test]
    fn tokenize_braced_dash_remove_prefix() {
        // v102: `${-#*e}` — nvm's errexit driver, RemovePrefix modifier.
        let tokens = tokenize("${-#*e}").unwrap();
        let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!() };
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
        let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!() };
        let WordPart::Var { name, .. } = &parts[0]
        else { panic!("expected Var, got {:?}", parts[0]) };
        assert_eq!(name, "?");
    }

    #[test]
    fn tokenize_braced_pid_bare() {
        // v102: `${$}` — shell-pid special param, bare form is a Var.
        let tokens = tokenize("${$}").unwrap();
        let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!() };
        let WordPart::Var { name, .. } = &parts[0]
        else { panic!("expected Var, got {:?}", parts[0]) };
        assert_eq!(name, "$");
    }

    #[test]
    fn tokenize_braced_bgpid_bare() {
        // v102: bare `${!}` is the `$!` last-bg-pid special param,
        // emitted as a plain Var, NOT the v95 indirect path.
        let tokens = tokenize("${!}").unwrap();
        let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!() };
        let WordPart::Var { name, .. } = &parts[0]
        else { panic!("expected Var, got {:?}", parts[0]) };
        assert_eq!(name, "!");
    }

    #[test]
    fn tokenize_braced_indirect_still_indirect() {
        // Regression: `${!var}` (non-`}` after `!`) stays the v95 indirect path.
        let tokens = tokenize("${!var}").unwrap();
        let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!() };
        let WordPart::ParamExpansion { name, indirect, .. } = &parts[0]
        else { panic!("expected ParamExpansion, got {:?}", parts[0]) };
        assert_eq!(name, "var");
        assert!(*indirect);
    }

    #[test]
    fn tokenize_assign_default_colon_equals() {
        let tokens = tokenize("${X:=w}").unwrap();
        let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!() };
        let WordPart::ParamExpansion { modifier, .. } = &parts[0] else { panic!() };
        assert!(matches!(modifier, ParamModifier::AssignDefault { colon: true, .. }));
    }

    #[test]
    fn tokenize_error_if_unset_colon_question() {
        let tokens = tokenize("${X:?msg}").unwrap();
        let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!() };
        let WordPart::ParamExpansion { modifier, .. } = &parts[0] else { panic!() };
        assert!(matches!(modifier, ParamModifier::ErrorIfUnset { colon: true, .. }));
    }

    #[test]
    fn tokenize_use_alternate_colon_plus() {
        let tokens = tokenize("${X:+w}").unwrap();
        let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!() };
        let WordPart::ParamExpansion { modifier, .. } = &parts[0] else { panic!() };
        assert!(matches!(modifier, ParamModifier::UseAlternate { colon: true, .. }));
    }

    #[test]
    fn tokenize_remove_prefix_short() {
        let tokens = tokenize("${X#pat}").unwrap();
        let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!() };
        let WordPart::ParamExpansion { modifier, .. } = &parts[0] else { panic!() };
        assert!(matches!(modifier, ParamModifier::RemovePrefix { longest: false, .. }));
    }

    #[test]
    fn tokenize_remove_prefix_long() {
        let tokens = tokenize("${X##pat}").unwrap();
        let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!() };
        let WordPart::ParamExpansion { modifier, .. } = &parts[0] else { panic!() };
        assert!(matches!(modifier, ParamModifier::RemovePrefix { longest: true, .. }));
    }

    #[test]
    fn tokenize_remove_suffix_short() {
        let tokens = tokenize("${X%pat}").unwrap();
        let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!() };
        let WordPart::ParamExpansion { modifier, .. } = &parts[0] else { panic!() };
        assert!(matches!(modifier, ParamModifier::RemoveSuffix { longest: false, .. }));
    }

    #[test]
    fn tokenize_remove_suffix_long() {
        let tokens = tokenize("${X%%pat}").unwrap();
        let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!() };
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
        let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!() };
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
        let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!() };
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
        let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!() };
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
        let TokenKind::Word(Word(parts)) = &toks[0].kind else { panic!() };
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
        let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!() };
        // Should be a ParamExpansion with UseDefault modifier.
        assert!(matches!(parts[0], WordPart::ParamExpansion { .. }));
    }

    #[test]
    fn newline_outside_quotes_emits_newline_token() {
        let tokens = tokenize("a\nb").unwrap();
        assert_eq!(
            tokens,
            vec![
                TokenKind::Word(Word(vec![WordPart::Literal { text: "a".to_string(), quoted: false }])),
                TokenKind::Newline.into(),
                TokenKind::Word(Word(vec![WordPart::Literal { text: "b".to_string(), quoted: false }])),
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
                TokenKind::Word(Word(vec![WordPart::Literal { text: "a".to_string(), quoted: false }])),
                TokenKind::Newline.into(),
                TokenKind::Newline.into(),
                TokenKind::Word(Word(vec![WordPart::Literal { text: "b".to_string(), quoted: false }])),
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
                TokenKind::Word(Word(vec![WordPart::Literal { text: "a".to_string(), quoted: false }])),
                TokenKind::Word(Word(vec![WordPart::Literal { text: "b".to_string(), quoted: false }])),
            ]
        );
    }

    #[test]
    fn tokenize_open_paren() {
        assert_eq!(tokenize("(").unwrap(), vec![TokenKind::Op(Operator::LParen)]);
    }

    #[test]
    fn tokenize_close_paren() {
        assert_eq!(tokenize(")").unwrap(), vec![TokenKind::Op(Operator::RParen)]);
    }

    #[test]
    fn tokenize_double_semi() {
        assert_eq!(tokenize(";;").unwrap(), vec![TokenKind::Op(Operator::DoubleSemi)]);
    }

    #[test]
    fn tokenize_semi_amp() {
        assert_eq!(tokenize(";&").unwrap(), vec![TokenKind::Op(Operator::SemiAmp)]);
    }

    #[test]
    fn tokenize_double_semi_amp() {
        assert_eq!(tokenize(";;&").unwrap(), vec![TokenKind::Op(Operator::DoubleSemiAmp)]);
    }

    #[test]
    fn tokenize_double_semi_space_amp_is_two_tokens() {
        assert_eq!(
            tokenize(";; &").unwrap(),
            vec![TokenKind::Op(Operator::DoubleSemi), TokenKind::Op(Operator::Background)]
        );
    }

    #[test]
    fn tokenize_lone_semi_still_semi() {
        assert_eq!(
            tokenize("a;b").unwrap(),
            vec![w("a"), TokenKind::Op(Operator::Semi).into(), w("b")]
        );
    }

    #[test]
    fn tokenize_paren_splits_adjacent_word() {
        assert_eq!(
            tokenize("a)").unwrap(),
            vec![w("a"), TokenKind::Op(Operator::RParen).into()]
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
            vec![TokenKind::Word(Word(vec![WordPart::Var {
                name: "1".to_string(), quoted: false
            }]))]
        );
    }

    #[test]
    fn tokenize_dollar_hash() {
        let tokens = tokenize("$#").unwrap();
        assert_eq!(
            tokens,
            vec![TokenKind::Word(Word(vec![WordPart::Var {
                name: "#".to_string(), quoted: false
            }]))]
        );
    }

    #[test]
    fn tokenize_dollar_at() {
        let tokens = tokenize("$@").unwrap();
        assert_eq!(
            tokens,
            vec![TokenKind::Word(Word(vec![WordPart::AllArgs {
                joined: false, quoted: false
            }]))]
        );
    }

    #[test]
    fn tokenize_dollar_star() {
        let tokens = tokenize("$*").unwrap();
        assert_eq!(
            tokens,
            vec![TokenKind::Word(Word(vec![WordPart::AllArgs {
                joined: true, quoted: false
            }]))]
        );
    }

    #[test]
    fn tokenize_quoted_dollar_at() {
        let tokens = tokenize("\"$@\"").unwrap();
        assert_eq!(
            tokens,
            vec![TokenKind::Word(Word(vec![qdouble(vec![WordPart::AllArgs {
                joined: false, quoted: true
            }])]))]
        );
    }

    #[test]
    fn tokenize_braced_positional() {
        let tokens = tokenize("${10}").unwrap();
        assert_eq!(
            tokens,
            vec![TokenKind::Word(Word(vec![WordPart::Var {
                name: "10".to_string(), quoted: false
            }]))]
        );
    }

    #[test]
    fn tokenize_braced_special_at() {
        let tokens = tokenize("${@}").unwrap();
        assert_eq!(
            tokens,
            vec![TokenKind::Word(Word(vec![WordPart::AllArgs {
                joined: false, quoted: false
            }]))]
        );
    }

    // --- Here-document tests (v24) ---

    /// Helper: extract the body Word from the first TokenKind::Heredoc in tokens.
    fn heredoc_body(tokens: &[Token]) -> &Word {
        for tok in tokens {
            if let TokenKind::Heredoc { body, .. } = &tok.kind {
                return body;
            }
        }
        panic!("no TokenKind::Heredoc found in tokens: {tokens:?}");
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
        // Verify <<EOF lexes and produces a TokenKind::Heredoc with body.
        let result = tokenize("cat <<EOF\nhello\nEOF\n");
        let tokens = result.expect("parse ok");
        assert_eq!(tokens.len(), 3, "got: {tokens:?}"); // Word("cat"), Heredoc{...}, Newline
        assert!(matches!(tokens[0].kind, TokenKind::Word(_)));
        assert!(matches!(tokens[1].kind, TokenKind::Heredoc { .. }));
        assert!(matches!(tokens[2].kind, TokenKind::Newline));
    }

    #[test]
    fn tokenize_heredoc_simple_expand() {
        // cat <<EOF\nhello\nEOF → TokenKind::Heredoc{body=Word[Literal{"hello"}, Literal{"\n"}],
        //                                         expand:true, strip_tabs:false}
        let tokens = tokenize("cat <<EOF\nhello\nEOF\n").unwrap();
        let body = heredoc_body(&tokens);
        // For an expanding heredoc, "hello" is a Literal{quoted:false} and "\n" is Literal{quoted:true}
        assert_eq!(body.0.len(), 2);
        assert_literal(&body.0[0], "hello", false);
        assert_literal(&body.0[1], "\n", true);
        if let TokenKind::Heredoc { expand, strip_tabs, .. } = &tokens[1].kind {
            assert!(expand, "should be expanding");
            assert!(!strip_tabs, "should not strip tabs");
        }
    }

    #[test]
    fn tokenize_heredoc_literal_no_expand() {
        // cat <<'EOF'\n$HOME\nEOF → body is one Literal{quoted:true, text:"$HOME\n"}
        let tokens = tokenize("cat <<'EOF'\n$HOME\nEOF\n").unwrap();
        if let TokenKind::Heredoc { body, expand, strip_tabs } = &tokens[1].kind {
            assert!(!expand, "quoted delim → literal mode (no expand)");
            assert!(!strip_tabs);
            // Literal mode: entire body as one quoted Literal per line, plus newline parts.
            assert_eq!(body.0.len(), 2);
            assert_literal(&body.0[0], "$HOME", true);
            assert_literal(&body.0[1], "\n", true);
        } else {
            panic!("expected TokenKind::Heredoc, got {:?}", tokens[1]);
        }
    }

    #[test]
    fn tokenize_heredoc_strip_tabs_dash() {
        // <<-EOF\n\t\thello\n\tEOF → body "hello\n" (tabs stripped from body AND close line)
        let tokens = tokenize("<<-EOF\n\t\thello\n\tEOF\n").unwrap();
        if let TokenKind::Heredoc { body, expand, strip_tabs } = &tokens[0].kind {
            assert!(strip_tabs, "<<- should strip tabs");
            assert!(expand);
            // After tab stripping, body line is "hello"
            assert_eq!(body.0.len(), 2);
            assert_literal(&body.0[0], "hello", false);
            assert_literal(&body.0[1], "\n", true);
        } else {
            panic!("expected TokenKind::Heredoc");
        }
    }

    #[test]
    fn tokenize_heredoc_strip_tabs_with_literal_delim() {
        // <<-'EOF' composes strip + no-expansion
        let tokens = tokenize("cat <<-'EOF'\n\thello\n\tEOF\n").unwrap();
        if let TokenKind::Heredoc { body, expand, strip_tabs } = &tokens[1].kind {
            assert!(strip_tabs, "<<- should strip tabs");
            assert!(!expand, "quoted delim → literal mode");
            assert_eq!(body.0.len(), 2);
            assert_literal(&body.0[0], "hello", true);
            assert_literal(&body.0[1], "\n", true);
        } else {
            panic!("expected TokenKind::Heredoc");
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
        assert!(matches!(tokens[0].kind, TokenKind::Word(_)));
        assert!(matches!(tokens[3].kind, TokenKind::Newline));
        if let TokenKind::Heredoc { body: body_a, .. } = &tokens[1].kind {
            assert_eq!(body_a.0.len(), 2);
            assert_literal(&body_a.0[0], "body_a", false);
            assert_literal(&body_a.0[1], "\n", true);
        } else {
            panic!("tokens[1] should be TokenKind::Heredoc for A");
        }
        if let TokenKind::Heredoc { body: body_b, .. } = &tokens[2].kind {
            assert_eq!(body_b.0.len(), 2);
            assert_literal(&body_b.0[0], "body_b", false);
            assert_literal(&body_b.0[1], "\n", true);
        } else {
            panic!("tokens[2] should be TokenKind::Heredoc for B");
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
        if let TokenKind::Heredoc { body, expand, .. } = &tokens[1].kind {
            assert!(!expand, "partial quoting triggers literal mode");
            // Literal body: "$X" as-is
            assert_eq!(body.0.len(), 2);
            assert_literal(&body.0[0], "$X", true);
            assert_literal(&body.0[1], "\n", true);
        } else {
            panic!("expected TokenKind::Heredoc");
        }
    }

    #[test]
    fn tokenize_heredoc_delim_backslash_escaped_is_literal_mode() {
        // cat <<\EOF\n$X\nEOF → expand:false (backslash-escaped delim = literal mode)
        let tokens = tokenize("cat <<\\EOF\n$X\nEOF\n").unwrap();
        if let TokenKind::Heredoc { body, expand, .. } = &tokens[1].kind {
            assert!(!expand, "backslash-escaped delim triggers literal mode");
            assert_eq!(body.0.len(), 2);
            assert_literal(&body.0[0], "$X", true);
            assert_literal(&body.0[1], "\n", true);
        } else {
            panic!("expected TokenKind::Heredoc");
        }
    }

    #[test]
    fn tokenize_heredoc_expanding_backslash_newline_joins_lines() {
        // POSIX 2.7.4: \<NL> inside expanding heredoc is line continuation.
        let tokens = tokenize("cat <<EOF\nhello \\\nworld\nEOF\n").unwrap();
        // Find the Heredoc token and verify body literal is "hello world\n".
        let body_text: String = match &tokens[1].kind {
            TokenKind::Heredoc { body, .. } => body.0.iter()
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
        let body_text: String = match &tokens[1].kind {
            TokenKind::Heredoc { body, .. } => body.0.iter()
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
        let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!("expected Word, got {:?}", tokens[0]) };
        assert_eq!(parts.len(), 1);
        assert!(
            matches!(&parts[0], WordPart::Var { name, quoted: false } if name == "$"),
            "got {:?}", parts[0]
        );
    }

    #[test]
    fn lexer_dollar_bang_emits_var_name_bang() {
        let tokens = tokenize("$!").unwrap();
        let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!() };
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
        let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!() };
        assert!(matches!(&parts[0], WordPart::Var { name, .. } if name == "0"));
    }

    #[test]
    fn lexer_dollar_dollar_inside_double_quotes() {
        let tokens = tokenize("\"$$\"").unwrap();
        let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!() };
        let [WordPart::Quoted { style: QuoteStyle::Double, parts: inner }] = &parts[..]
        else { panic!("expected one double-quote run: {parts:?}") };
        assert!(matches!(&inner[0], WordPart::Var { name, quoted: true } if name == "$"));
    }

    #[test]
    fn lexer_dollar_bang_inside_double_quotes() {
        let tokens = tokenize("\"$!\"").unwrap();
        let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!() };
        let [WordPart::Quoted { style: QuoteStyle::Double, parts: inner }] = &parts[..]
        else { panic!("expected one double-quote run: {parts:?}") };
        assert!(matches!(&inner[0], WordPart::Var { name, quoted: true } if name == "!"));
    }

    #[test]
    fn lexer_dollar_dollar_concatenates_with_literal() {
        let tokens = tokenize("pre-$$-post").unwrap();
        let TokenKind::Word(Word(parts)) = &tokens[0].kind else { panic!() };
        assert_eq!(parts.len(), 3);
        assert!(matches!(&parts[0], WordPart::Literal { text, .. } if text == "pre-"));
        assert!(matches!(&parts[1], WordPart::Var { name, .. } if name == "$"));
        assert!(matches!(&parts[2], WordPart::Literal { text, .. } if text == "-post"));
    }

    // ---- v27 here-string lexer tests -------------------------------------------

    #[test]
    fn tokenize_here_string_op_alone() {
        let tokens = tokenize("<<<").unwrap();
        assert_eq!(tokens, vec![TokenKind::Op(Operator::HereString)]);
    }

    #[test]
    fn tokenize_here_string_with_unquoted_word() {
        let tokens = tokenize("cat <<< hello").unwrap();
        assert_eq!(tokens.len(), 3);
        assert!(matches!(tokens[0].kind, TokenKind::Word(_)));
        assert!(matches!(tokens[1].kind, TokenKind::Op(Operator::HereString)));
        assert!(matches!(tokens[2].kind, TokenKind::Word(_)));
    }

    #[test]
    fn tokenize_here_string_with_quoted_word() {
        let tokens = tokenize("cat <<< \"hi there\"").unwrap();
        let TokenKind::Word(Word(parts)) = &tokens[2].kind else { panic!("got {:?}", tokens[2]) };
        let [WordPart::Quoted { style: QuoteStyle::Double, parts: inner }] = &parts[..]
        else { panic!("expected one double-quote run: {parts:?}") };
        assert!(matches!(&inner[0], WordPart::Literal { text, quoted: true } if text == "hi there"));
    }

    #[test]
    fn tokenize_here_string_with_var_in_body() {
        let tokens = tokenize("cat <<< $FOO").unwrap();
        let TokenKind::Word(Word(parts)) = &tokens[2].kind else { panic!() };
        assert!(matches!(&parts[0], WordPart::Var { name, .. } if name == "FOO"));
    }

    #[test]
    fn tokenize_here_string_with_command_sub_in_body() {
        let tokens = tokenize("cat <<< $(echo hi)").unwrap();
        let TokenKind::Word(Word(parts)) = &tokens[2].kind else { panic!() };
        assert!(matches!(&parts[0], WordPart::CommandSub { .. }));
    }

    #[test]
    fn tokenize_double_less_still_heredoc() {
        // Regression: `<<EOF` must still lex as Heredoc, not split into `<<` + `<EOF`.
        let tokens = tokenize("cat <<EOF\nbody\nEOF\n").unwrap();
        assert!(tokens.iter().any(|t| matches!(&t.kind, TokenKind::Heredoc { .. })),
            "expected Heredoc token, got {:?}", tokens);
    }

    #[test]
    fn tokenize_dup_out_basic() {
        let tokens = tokenize(">&").unwrap();
        assert_eq!(tokens, vec![TokenKind::Op(Operator::DupOut)]);
    }

    #[test]
    fn tokenize_dup_err_basic() {
        let tokens = tokenize("2>&").unwrap();
        assert_eq!(tokens, vec![TokenKind::Op(Operator::DupErr)]);
    }

    #[test]
    fn tokenize_and_redir_out() {
        let tokens = tokenize("&>").unwrap();
        assert_eq!(tokens, vec![TokenKind::Op(Operator::AndRedirOut)]);
    }

    #[test]
    fn tokenize_and_redir_append() {
        let tokens = tokenize("&>>").unwrap();
        assert_eq!(tokens, vec![TokenKind::Op(Operator::AndRedirAppend)]);
    }

    #[test]
    fn tokenize_dup_in_context() {
        let tokens = tokenize("cmd 2>&1").unwrap();
        assert_eq!(tokens.len(), 3);
        assert!(matches!(tokens[0].kind, TokenKind::Word(_)));
        assert!(matches!(tokens[1].kind, TokenKind::Op(Operator::DupErr)));
        assert!(matches!(tokens[2].kind, TokenKind::Word(_)));
    }

    #[test]
    fn tokenize_redir_out_regression() {
        assert_eq!(tokenize(">").unwrap(), vec![TokenKind::Op(Operator::RedirOut)]);
        assert_eq!(tokenize(">>").unwrap(), vec![TokenKind::Op(Operator::RedirAppend)]);
    }

    #[test]
    fn tokenize_redir_err_regression() {
        assert_eq!(tokenize("2>").unwrap(), vec![TokenKind::Op(Operator::RedirErr)]);
        assert_eq!(tokenize("2>>").unwrap(), vec![TokenKind::Op(Operator::RedirErrAppend)]);
    }

    #[test]
    fn tokenize_explicit_fd1_redir_out() {
        // `1>` lexes as RedirOut (same as `>`).
        let tokens = tokenize("1>").unwrap();
        assert_eq!(tokens, vec![TokenKind::Op(Operator::RedirOut)]);
    }

    #[test]
    fn tokenize_explicit_fd1_redir_append() {
        let tokens = tokenize("1>>").unwrap();
        assert_eq!(tokens, vec![TokenKind::Op(Operator::RedirAppend)]);
    }

    #[test]
    fn tokenize_explicit_fd1_dup() {
        let tokens = tokenize("1>&").unwrap();
        assert_eq!(tokens, vec![TokenKind::Op(Operator::DupOut)]);
    }

    #[test]
    fn tokenize_one_as_arg_when_has_token() {
        // `cmd 1` where 1 is an argument — should NOT trigger the new arm.
        let tokens = tokenize("cmd 1").unwrap();
        assert_eq!(tokens.len(), 2);
        assert!(matches!(tokens[0].kind, TokenKind::Word(_)));
        assert!(matches!(tokens[1].kind, TokenKind::Word(_)));
    }

    #[test]
    fn tokenize_background_regression() {
        assert_eq!(tokenize("&").unwrap(), vec![TokenKind::Op(Operator::Background)]);
        assert_eq!(tokenize("&&").unwrap(), vec![TokenKind::Op(Operator::And)]);
    }

    // ──────────────────────────────────────────────────────────────
    // >| clobber redirect tests (v123)
    // ──────────────────────────────────────────────────────────────

    #[test]
    fn lex_clobber_stdout() {
        assert_eq!(tokenize(">|").unwrap(), vec![TokenKind::Op(Operator::RedirClobber)]);
        assert_eq!(tokenize("1>|").unwrap(), vec![TokenKind::Op(Operator::RedirClobber)]);
    }

    #[test]
    fn lex_clobber_stderr() {
        assert_eq!(tokenize("2>|").unwrap(), vec![TokenKind::Op(Operator::RedirErrClobber)]);
    }

    #[test]
    fn lex_clobber_with_target() {
        assert_eq!(
            tokenize("cmd >|f").unwrap(),
            vec![w("cmd"), TokenKind::Op(Operator::RedirClobber).into(), w("f")]
        );
    }

    #[test]
    fn lex_redir_then_pipe_unaffected() {
        assert_eq!(
            tokenize("> |").unwrap(),
            vec![TokenKind::Op(Operator::RedirOut), TokenKind::Op(Operator::Pipe)]
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
            matches!(&tokens[0].kind, TokenKind::Word(Word(parts))
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
            matches!(&tokens[0].kind, TokenKind::Word(Word(parts))
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
        assert!(matches!(&tokens[0].kind, TokenKind::Word(_)), "expected Word, got {:?}", tokens[0]);
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
        let TokenKind::Word(Word(parts)) = &toks[0].kind else { panic!() };
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
        match &toks[0].kind {
            TokenKind::Word(Word(parts)) => {
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
        let TokenKind::Word(Word(parts)) = &toks[0].kind else { panic!("expected Word") };
        assert_eq!(parts.len(), 1);
        let WordPart::Literal { text, quoted } = ansi_c_inner(&parts[0]) else { panic!("expected Literal") };
        assert_eq!(text, "a\tb");
        assert!(*quoted);
    }

    #[test]
    fn ansi_c_quote_backslash_and_quote() {
        // $'\\\'' → literal backslash + literal quote (two chars)
        let toks = tokenize("$'\\\\\\''").expect("lex");
        let TokenKind::Word(Word(parts)) = &toks[0].kind else { panic!("expected Word") };
        assert_eq!(parts.len(), 1);
        let WordPart::Literal { text, .. } = ansi_c_inner(&parts[0]) else { panic!("expected Literal") };
        assert_eq!(text, "\\'");
    }

    #[test]
    fn ansi_c_quote_hex_escapes() {
        // \x48\x69 → "Hi"
        let toks = tokenize("$'\\x48\\x69'").expect("lex");
        let TokenKind::Word(Word(parts)) = &toks[0].kind else { panic!("expected Word") };
        let WordPart::Literal { text, .. } = ansi_c_inner(&parts[0]) else { panic!("expected Literal") };
        assert_eq!(text, "Hi");
    }

    #[test]
    fn ansi_c_quote_octal_escapes() {
        // \110\151 → "Hi"  (0o110=72='H', 0o151=105='i')
        let toks = tokenize("$'\\110\\151'").expect("lex");
        let TokenKind::Word(Word(parts)) = &toks[0].kind else { panic!("expected Word") };
        let WordPart::Literal { text, .. } = ansi_c_inner(&parts[0]) else { panic!("expected Literal") };
        assert_eq!(text, "Hi");
    }

    #[test]
    fn ansi_c_quote_octal_greedy_stops_at_non_octal() {
        // \18 → \1 followed by literal '8' → "\x01" + "8"
        let toks = tokenize("$'\\18'").expect("lex");
        let TokenKind::Word(Word(parts)) = &toks[0].kind else { panic!("expected Word") };
        let WordPart::Literal { text, .. } = ansi_c_inner(&parts[0]) else { panic!("expected Literal") };
        assert_eq!(text, "\x018");
    }

    #[test]
    fn ansi_c_quote_unicode_4digit() {
        // é → é (U+00E9, "LATIN SMALL LETTER E WITH ACUTE")
        let toks = tokenize("$'\\u00e9'").expect("lex");
        let TokenKind::Word(Word(parts)) = &toks[0].kind else { panic!("expected Word") };
        let WordPart::Literal { text, .. } = ansi_c_inner(&parts[0]) else { panic!("expected Literal") };
        assert_eq!(text, "é");
    }

    #[test]
    fn ansi_c_quote_unicode_8digit() {
        // \U0001F600 → 😀 (grinning face)
        let toks = tokenize("$'\\U0001F600'").expect("lex");
        let TokenKind::Word(Word(parts)) = &toks[0].kind else { panic!("expected Word") };
        let WordPart::Literal { text, .. } = ansi_c_inner(&parts[0]) else { panic!("expected Literal") };
        assert_eq!(text, "\u{1F600}");
    }

    #[test]
    fn ansi_c_quote_control_chars() {
        // \cA → \x01, \cZ → \x1A
        let toks = tokenize("$'\\cA\\cZ'").expect("lex");
        let TokenKind::Word(Word(parts)) = &toks[0].kind else { panic!("expected Word") };
        let WordPart::Literal { text, .. } = ansi_c_inner(&parts[0]) else { panic!("expected Literal") };
        assert_eq!(text, "\x01\x1a");
    }

    #[test]
    fn ansi_c_quote_unknown_escape_preserves_both() {
        // \q → literal "\q" (two chars)
        let toks = tokenize("$'\\q'").expect("lex");
        let TokenKind::Word(Word(parts)) = &toks[0].kind else { panic!("expected Word") };
        let WordPart::Literal { text, .. } = ansi_c_inner(&parts[0]) else { panic!("expected Literal") };
        assert_eq!(text, "\\q");
    }

    #[test]
    fn ansi_c_quote_empty() {
        let toks = tokenize("$''").expect("lex");
        let TokenKind::Word(Word(parts)) = &toks[0].kind else { panic!("expected Word") };
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
        let TokenKind::Word(Word(parts)) = &toks[0].kind else { panic!("expected Word") };
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
            .filter_map(|t| match &t.kind {
                TokenKind::Word(Word(parts)) => {
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
            .filter_map(|t| match &t.kind {
                TokenKind::Word(Word(parts)) => Some(parts),
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
        let word_count = toks.iter().filter(|t| matches!(&t.kind, TokenKind::Word(_))).count();
        assert_eq!(word_count, 2, "expected 2 Words (echo + the quoted literal), got {word_count}");
    }

    #[test]
    fn tokenize_single_quoted_brace_not_expanded() {
        let toks = tokenize("echo '{a,b}'").expect("lex");
        let word_count = toks.iter().filter(|t| matches!(&t.kind, TokenKind::Word(_))).count();
        assert_eq!(word_count, 2, "expected 2 Words, got {word_count}");
    }

    #[test]
    fn tokenize_backslash_brace_not_expanded() {
        // The lexer's `\X` arm pushes each escaped char as a
        // one-char QUOTED Literal (quoted: true). Brace expansion
        // only fires on UNQUOTED Literals, so `\{a,b\}` survives
        // as a single Word.
        let toks = tokenize("echo \\{a,b\\}").expect("lex");
        let word_count = toks.iter().filter(|t| matches!(&t.kind, TokenKind::Word(_))).count();
        assert_eq!(word_count, 2, "expected 2 Words, got {word_count}");
    }

    #[test]
    fn arith_block_simple() {
        let tokens = tokenize("((1+2))").unwrap();
        assert_eq!(tokens.len(), 1);
        match &tokens[0].kind {
            TokenKind::ArithBlock(s, _) => assert_eq!(s, "1+2"),
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
            !matches!(toks[0].kind, TokenKind::ArithBlock(..)),
            "head must not be an ArithBlock: {toks:?}"
        );
        assert!(matches!(toks[0].kind, TokenKind::Op(Operator::LParen)));
        assert!(matches!(toks[1].kind, TokenKind::Op(Operator::LParen)));
    }

    #[test]
    fn double_paren_nested_subshell_not_arith() {
        // v184: `((echo a) | cat)` has no matching `))` → nested subshells `( (`,
        // NOT an arithmetic block. Lexes to two LParens, no ArithBlock.
        let toks = tokenize("((echo a) | cat)").unwrap();
        assert!(
            !toks.iter().any(|t| matches!(&t.kind, TokenKind::ArithBlock(..))),
            "must not be an ArithBlock: {toks:?}"
        );
        assert!(matches!(toks[0].kind, TokenKind::Op(Operator::LParen)));
        assert!(matches!(toks[1].kind, TokenKind::Op(Operator::LParen)));
    }

    #[test]
    fn double_paren_real_arith_still_arithblock() {
        // v184 regression: a `((` that DOES close as `))` stays an ArithBlock.
        let toks = tokenize("((1+2))").unwrap();
        assert_eq!(toks.len(), 1);
        assert!(matches!(toks[0].kind, TokenKind::ArithBlock(..)));
    }

    #[test]
    fn double_paren_deep_nesting_not_arith() {
        // v184: `((( echo a ) ) )` — the closing parens are not adjacent, so no
        // `))` for the outer `((` → LParens, not an ArithBlock.
        let toks = tokenize("((( echo a ) ) )").unwrap();
        assert!(
            !toks.iter().any(|t| matches!(&t.kind, TokenKind::ArithBlock(..))),
            "must not be an ArithBlock: {toks:?}"
        );
    }

    #[test]
    fn arith_block_with_semicolons() {
        let tokens = tokenize("((a;b;c))").unwrap();
        assert_eq!(tokens.len(), 1);
        match &tokens[0].kind {
            TokenKind::ArithBlock(s, _) => assert_eq!(s, "a;b;c"),
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
        match &tokens[0].kind {
            TokenKind::ArithBlock(s, _) => assert_eq!(s, "((a+b)*c)"),
            other => panic!("expected ArithBlock, got {other:?}"),
        }
    }

    #[test]
    fn arith_block_with_internal_whitespace() {
        let tokens = tokenize("((  1 + 2  ))").unwrap();
        assert_eq!(tokens.len(), 1);
        match &tokens[0].kind {
            TokenKind::ArithBlock(s, _) => assert_eq!(s, "  1 + 2  "),
            other => panic!("expected ArithBlock, got {other:?}"),
        }
    }

    #[test]
    fn arith_block_empty_body() {
        let tokens = tokenize("(())").unwrap();
        assert_eq!(tokens.len(), 1);
        match &tokens[0].kind {
            TokenKind::ArithBlock(s, _) => assert_eq!(s, ""),
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
            !toks.iter().any(|t| matches!(&t.kind, TokenKind::ArithBlock(..))),
            "must not be an ArithBlock: {toks:?}"
        );
        assert!(matches!(toks[0].kind, TokenKind::Op(Operator::LParen)));
        assert!(matches!(toks[1].kind, TokenKind::Op(Operator::LParen)));
    }

    #[test]
    fn arith_block_single_paren_at_end_falls_back_to_lparens() {
        // `((1+2)` — one closing paren, not two, so no matching `))`. As of
        // v184 this falls back to nested subshells `( (` rather than erroring.
        let toks = tokenize("((1+2)").unwrap();
        assert!(
            !toks.iter().any(|t| matches!(&t.kind, TokenKind::ArithBlock(..))),
            "must not be an ArithBlock: {toks:?}"
        );
        assert!(matches!(toks[0].kind, TokenKind::Op(Operator::LParen)));
        assert!(matches!(toks[1].kind, TokenKind::Op(Operator::LParen)));
    }

    #[test]
    fn space_between_parens_is_not_arith_block() {
        // `( (cmd) )` — whitespace between the two `(`s. Should tokenize
        // as two LParen ops, a Word, and two RParen ops (nested-subshell
        // path per M-11). The arith-block detector must NOT fire.
        let tokens = tokenize("( (cmd) )").unwrap();
        let lparen_count = tokens
            .iter()
            .filter(|t| matches!(&t.kind, TokenKind::Op(Operator::LParen)))
            .count();
        let arith_count = tokens
            .iter()
            .filter(|t| matches!(&t.kind, TokenKind::ArithBlock(..)))
            .count();
        assert_eq!(lparen_count, 2, "expected two LParen tokens: {tokens:?}");
        assert_eq!(arith_count, 0, "did not expect ArithBlock: {tokens:?}");
    }

    // ── v241 T2: ParamExpansion head-mode tests ────────────────────────────────

    /// Drive `Mode::ParamExpansion` directly and collect all head atoms through
    /// (and including) `ParamClose`.
    fn head_atoms(s: &str) -> Vec<TokenKind> {
        let mut lx = Lexer::new(s, LexerOptions::default(), true);
        lx.push_mode(Mode::ParamExpansion { seen_name: false, indirect: false, start_off: 0 });
        let mut out = Vec::new();
        while let Some(t) = lx.next_token().unwrap() {
            let stop = matches!(t.kind, TokenKind::ParamClose);
            out.push(t.kind);
            if stop { break; }
        }
        out
    }

    /// Like `head_atoms` but stops after the first `ParamOp` is emitted
    /// (operand is a different mode, so we stop at the operator boundary).
    fn head_atoms_until_op(s: &str) -> Vec<TokenKind> {
        let mut lx = Lexer::new(s, LexerOptions::default(), true);
        lx.push_mode(Mode::ParamExpansion { seen_name: false, indirect: false, start_off: 0 });
        let mut out = Vec::new();
        while let Some(t) = lx.next_token().unwrap() {
            let stop = matches!(t.kind, TokenKind::ParamOp(_));
            out.push(t.kind);
            if stop { break; }
        }
        out
    }

    #[test]
    fn head_bare_name() {
        assert_eq!(
            head_atoms("${name}"),
            vec![
                TokenKind::ParamOpen { quoted: false },
                TokenKind::ParamName("name".into()),
                TokenKind::ParamClose,
            ]
        );
    }

    #[test]
    fn head_value_operator() {
        // stops emitting at the operator; operand is a different mode (Task 3)
        let a = head_atoms_until_op("${x:-foo}");
        assert_eq!(
            a,
            vec![
                TokenKind::ParamOpen { quoted: false },
                TokenKind::ParamName("x".into()),
                TokenKind::ParamOp(ParamOpKind::UseDefault(true)),
            ]
        );
    }

    #[test]
    fn head_length_and_indirect() {
        assert_eq!(
            head_atoms("${#x}"),
            vec![
                TokenKind::ParamOpen { quoted: false },
                TokenKind::ParamLengthPrefix,
                TokenKind::ParamName("x".into()),
                TokenKind::ParamClose,
            ]
        );
        assert_eq!(
            head_atoms("${!x}"),
            vec![
                TokenKind::ParamOpen { quoted: false },
                TokenKind::ParamIndirect,
                TokenKind::ParamName("x".into()),
                TokenKind::ParamClose,
            ]
        );
    }

    #[test]
    fn head_special_param_names() {
        // bare special-param names: ${@} ${#} ${?} ${!}
        for (s, n) in [("${@}", "@"), ("${#}", "#"), ("${?}", "?"), ("${!}", "!")] {
            assert_eq!(
                head_atoms(s),
                vec![
                    TokenKind::ParamOpen { quoted: false },
                    TokenKind::ParamName(n.into()),
                    TokenKind::ParamClose,
                ],
                "for {s}"
            );
        }
    }

    #[test]
    fn head_subscript() {
        // ${a[...] emits ParamOpen, ParamName(a), LBracket then yields to subscript mode
        let mut lx = Lexer::new("${a[1]}", LexerOptions::default(), true);
        lx.push_mode(Mode::ParamExpansion { seen_name: false, indirect: false, start_off: 0 });
        assert!(matches!(lx.next_token().unwrap().unwrap().kind, TokenKind::ParamOpen { .. }));
        assert!(matches!(lx.next_token().unwrap().unwrap().kind, TokenKind::ParamName(ref n) if n == "a"));
        assert!(matches!(lx.next_token().unwrap().unwrap().kind, TokenKind::LBracket));
    }

    /// Prove stack-safety: pushing a nested `ParamExpansion` frame must not
    /// corrupt the outer frame's `seen_name` phase.  Before the fix the phase
    /// was held in a flat `param_head_seen_name` field on `Lexer`, so any push
    /// reset it to `false` — the outer frame "forgot" it had already seen its
    /// name.  Now each `Mode::ParamExpansion { seen_name }` carries the state
    /// in the stack frame itself, so nesting is safe.
    #[test]
    fn head_nested_param_expansion_phase_is_per_frame() {
        // We drive the outer frame manually: push it, pull ParamOpen then
        // ParamName("a") so the outer frame transitions to seen_name=true.
        // Then simulate the parser entering a nested ${b} by pushing a fresh
        // inner frame, pull its ParamOpen + ParamName("b"), then pop it.
        // The outer frame's seen_name must still be true afterwards.
        let mut lx = Lexer::new("${a${b}}", LexerOptions::default(), true);
        lx.push_mode(Mode::ParamExpansion { seen_name: false, indirect: false, start_off: 0 });

        // Outer frame: pull ParamOpen (${ of outer).
        assert!(matches!(lx.next_token().unwrap().unwrap().kind, TokenKind::ParamOpen { .. }));
        // Outer frame: pull ParamName("a") → seen_name becomes true.
        assert!(matches!(lx.next_token().unwrap().unwrap().kind, TokenKind::ParamName(ref n) if n == "a"));
        // Outer frame must now be in seen_name=true (post-name phase).
        assert!(
            matches!(lx.modes.last(), Some(Mode::ParamExpansion { seen_name: true, .. })),
            "outer frame should be seen_name=true after pulling its name"
        );

        // Simulate parser detecting nested ${b} and pushing a fresh inner frame.
        lx.push_mode(Mode::ParamExpansion { seen_name: false, indirect: false, start_off: 0 });
        // Inner frame: pull ParamOpen (the ${ of ${b}).
        assert!(matches!(lx.next_token().unwrap().unwrap().kind, TokenKind::ParamOpen { .. }));
        // Inner frame: pull ParamName("b") → inner seen_name becomes true.
        assert!(matches!(lx.next_token().unwrap().unwrap().kind, TokenKind::ParamName(ref n) if n == "b"));

        // Parser exits the nested expansion: pop the inner frame.
        lx.pop_mode();

        // The OUTER frame must still be seen_name=true (was corrupted before fix).
        assert!(
            matches!(lx.modes.last(), Some(Mode::ParamExpansion { seen_name: true, .. })),
            "outer frame seen_name must survive nested push/pop"
        );
    }

    // ── v241 T3: ParamExpansion operand-mode atom emission tests ─────────────

    /// Drive an operand mode directly and collect atoms until `ParamClose`,
    /// `RBracket`, or `ParamSep` (a separator also ends the immediate operand
    /// segment — the test inputs may not include a trailing `}` after the sep).
    fn operand_atoms(s: &str, mode: Mode) -> Vec<TokenKind> {
        let mut lx = Lexer::new(s, LexerOptions::default(), true);
        lx.push_mode(mode);
        let mut out = Vec::new();
        while let Some(t) = lx.next_token().unwrap() {
            // CmdSubOpen / BeginBacktick / ArithOpen are parser hand-off signals:
            // without the parser pushing Mode::CommandSub / Mode::Backtick /
            // Mode::Arith, further scanning would spin on the same `$(` / `` ` `` /
            // `$((` (the signal is emitted without advancing the cursor). Stop here
            // just like we stop at boundary atoms. (v246 T6: ArithOpen added — this
            // exact omission OOM-crashed a prior session; see v245 T6 for BeginBacktick.)
            let stop = matches!(t.kind,
                TokenKind::ParamClose | TokenKind::RBracket | TokenKind::ParamSep
                    | TokenKind::CmdSubOpen | TokenKind::BeginBacktick | TokenKind::ArithOpen);
            out.push(t.kind);
            if stop { break; }
        }
        out
    }

    #[test]
    fn operand_plain_literal() {
        assert_eq!(
            operand_atoms("foo}", Mode::ParamWordOperand { in_dquote: false, enclosing_dquote: false }),
            vec![
                TokenKind::Lit { text: "foo".into(), quoted: false },
                TokenKind::ParamClose,
            ]
        );
    }

    #[test]
    fn operand_var_and_nested() {
        // Plain `$a` followed by terminator.
        assert_eq!(
            operand_atoms("$a}", Mode::ParamWordOperand { in_dquote: false, enclosing_dquote: false }),
            vec![TokenKind::DollarName { name: "a".into(), quoted: false }, TokenKind::ParamClose]
        );
        // Nested `${b}` — the parser would push ParamExpansion mode on ParamOpen;
        // in this standalone test the first atom is ParamOpen and that is sufficient.
        let nested = operand_atoms("${b}}", Mode::ParamWordOperand { in_dquote: false, enclosing_dquote: false });
        assert_eq!(nested[0], TokenKind::ParamOpen { quoted: false });
    }

    #[test]
    fn operand_subst_separator() {
        assert_eq!(
            operand_atoms("pat/", Mode::ParamSubstPatternOperand { in_dquote: false, enclosing_dquote: false }),
            vec![
                TokenKind::Lit { text: "pat".into(), quoted: false },
                TokenKind::ParamSep,
            ]
        );
    }

    #[test]
    fn operand_substring_separator() {
        assert_eq!(
            operand_atoms("1:", Mode::ParamSubstringOffsetOperand { in_dquote: false, enclosing_dquote: false }),
            vec![
                TokenKind::Lit { text: "1".into(), quoted: false },
                TokenKind::ParamSep,
            ]
        );
    }

    #[test]
    fn operand_deferred_cmdsub() {
        // v244 T4: unquoted `$(cmd)` in an operand emits CmdSubOpen (signal to parse_command_sub).
        // v245 T6: backtick emits BeginBacktick (signal to parse_backtick_sub).
        // v246 T6: `$((` emits ArithOpen (signal to parse_arith_expansion).
        let a = operand_atoms("$(x)}", Mode::ParamWordOperand { in_dquote: false, enclosing_dquote: false });
        assert_eq!(a[0], TokenKind::CmdSubOpen, "$(cmd) must emit CmdSubOpen signal");

        // `$((` now emits ArithOpen (v246 T6) — no longer DeferredExpansion.
        let b = operand_atoms("$((1+1))}", Mode::ParamWordOperand { in_dquote: false, enclosing_dquote: false });
        assert_eq!(b[0], TokenKind::ArithOpen, "$((…)) must emit ArithOpen signal");

        // Backtick now emits BeginBacktick signal (v245 T6).
        let c = operand_atoms("`echo x`}", Mode::ParamWordOperand { in_dquote: false, enclosing_dquote: false });
        assert_eq!(c[0], TokenKind::BeginBacktick, "backtick must emit BeginBacktick signal");
    }

    #[test]
    fn operand_arith_signal() {
        // v246 T6: `$((` in an unquoted operand emits ArithOpen (zero-width signal).
        let a = operand_atoms("$((1+1))}", Mode::ParamWordOperand { in_dquote: false, enclosing_dquote: false });
        assert_eq!(a[0], TokenKind::ArithOpen, "$(( must emit ArithOpen signal");
    }

    // ── v247 T7: Command-mode atom-stream shape ──────────────────────────────

    /// Drive a `command_atoms` lexer's FLAT stream and collect the raw atoms.
    /// Stops at the parser hand-off signals (`CmdSubOpen`/`BeginBacktick`/
    /// `ArithOpen`/`ParamOpen`) exactly like `operand_atoms`, so a raw drive
    /// with no parser mode-push cannot spin on the same zero-width opener.
    fn command_atoms_of(s: &str) -> Vec<TokenKind> {
        let mut lx = Lexer::new_live_atoms(s, &Default::default(), LexerOptions::default());
        let mut out = Vec::new();
        while let Some(t) = lx.next_token().unwrap() {
            let stop = matches!(t.kind,
                TokenKind::CmdSubOpen | TokenKind::BeginBacktick
                    | TokenKind::ArithOpen | TokenKind::ParamOpen { .. });
            out.push(t.kind);
            if stop { break; }
        }
        out
    }

    #[test]
    fn command_atoms_stream_shape() {
        // `Blank` splits words; literals carry `quoted:false`.
        assert_eq!(
            command_atoms_of("echo hi"),
            vec![
                TokenKind::Lit { text: "echo".into(), quoted: false },
                TokenKind::Blank,
                TokenKind::Lit { text: "hi".into(), quoted: false },
            ],
        );
        // `Blank` NEVER appears in the Word-mode (production) stream.
        let words = tokenize_with_opts("echo hi", LexerOptions::default()).unwrap();
        assert!(words.iter().all(|t| !matches!(t.kind, TokenKind::Blank)),
            "Blank must never appear in the Word-mode stream");
    }

    // ── v250 T1: heredoc opener atom (dormant; body emission is a later task) ──

    #[test]
    fn heredoc_opener_atom_parses_delim() {
        // The atom `<<` opener consumes the delimiter and carries the correct
        // `expand`/`strip_tabs`; the delimiter is NOT left as a following word.
        let toks = command_atoms_of("cat <<EOF");
        assert!(matches!(toks.last(), Some(TokenKind::Heredoc { expand: true, strip_tabs: false, .. })),
            "unquoted delim → expanding; got {toks:?}");
        // No trailing Lit("EOF") — the delimiter was consumed by the opener.
        assert!(!toks.iter().any(|t| matches!(t, TokenKind::Lit { text, .. } if text == "EOF")),
            "delimiter must be consumed, not emitted as a word: {toks:?}");
        // Quoted delimiter → literal (expand:false); `<<-` → strip_tabs.
        let q = command_atoms_of("cat <<'EOF'");
        assert!(matches!(q.last(), Some(TokenKind::Heredoc { expand: false, .. })), "quoted delim → literal: {q:?}");
        let dash = command_atoms_of("cat <<-EOF");
        assert!(matches!(dash.last(), Some(TokenKind::Heredoc { strip_tabs: true, expand: true, .. })), "<<- → strip_tabs: {dash:?}");
    }

    // ── v251 T1: ProcSubOpen atom (dormant; parser assembly is a later task) ──

    #[test]
    fn procsub_open_atoms_disambiguate() {
        // `<(`/`>(` → ProcSubOpen signal (NOT a redirect op); the `(` is NOT consumed.
        let a = command_atoms_of("cat <(echo hi)");
        assert!(a.iter().any(|t| matches!(t, TokenKind::ProcSubOpen { dir: ProcDir::In })),
            "expected ProcSubOpen In, got {a:?}");
        assert!(!a.iter().any(|t| matches!(t, TokenKind::Op(Operator::RedirIn))),
            "`<(` must NOT emit RedirIn: {a:?}");
        let b = command_atoms_of("tee >(cat)");
        assert!(b.iter().any(|t| matches!(t, TokenKind::ProcSubOpen { dir: ProcDir::Out })),
            "expected ProcSubOpen Out, got {b:?}");
        // Non-`(` `<`/`>` are unaffected.
        let r = command_atoms_of("cat < f");
        assert!(r.iter().any(|t| matches!(t, TokenKind::Op(Operator::RedirIn))), "plain `<` still RedirIn: {r:?}");
        let rr = command_atoms_of("echo >> f");
        assert!(rr.iter().any(|t| matches!(t, TokenKind::Op(Operator::RedirAppend))), "`>>` still RedirAppend: {rr:?}");
    }

    #[test]
    fn heredoc_literal_body_atoms() {
        // Quoted delimiter → literal body: Newline, then Begin, one raw Lit, End.
        let toks = command_atoms_of("cat <<'EOF'\nhello $x\nEOF\n");
        // Find the body bracket.
        let begin = toks.iter().position(|t| matches!(t, TokenKind::HeredocBodyBegin { .. })).expect("Begin");
        assert!(matches!(&toks[begin + 1], TokenKind::Lit { text, quoted: true } if text == "hello $x\n"),
            "literal body is one raw Lit (no expansion of $x): {toks:?}");
        assert!(matches!(&toks[begin + 2], TokenKind::HeredocBodyEnd), "End after body: {toks:?}");
    }

    #[test]
    fn heredoc_literal_body_atoms_dash_strips_tabs() {
        // `<<-` strips leading TABS (only) from each body line AND the delimiter check.
        let toks = command_atoms_of("cat <<-'EOF'\n\t\thello\n\tEOF\n");
        let begin = toks.iter().position(|t| matches!(t, TokenKind::HeredocBodyBegin { .. })).expect("Begin");
        assert!(matches!(&toks[begin + 1], TokenKind::Lit { text, quoted: true } if text == "hello\n"),
            "<<- strips leading tabs from body lines: {toks:?}");
        assert!(matches!(&toks[begin + 2], TokenKind::HeredocBodyEnd), "End after body: {toks:?}");
    }

    #[test]
    fn heredoc_literal_body_atoms_empty_body() {
        // Empty body → no Lit emitted, just Begin then End.
        let toks = command_atoms_of("cat <<'EOF'\nEOF\n");
        let begin = toks.iter().position(|t| matches!(t, TokenKind::HeredocBodyBegin { .. })).expect("Begin");
        assert!(matches!(&toks[begin + 1], TokenKind::HeredocBodyEnd),
            "empty body emits no Lit: {toks:?}");
    }

    #[test]
    fn operand_subscript_close() {
        assert_eq!(
            operand_atoms("3]", Mode::ParamSubscriptOperand { in_dquote: false, enclosing_dquote: false }),
            vec![
                TokenKind::Lit { text: "3".into(), quoted: false },
                TokenKind::RBracket,
            ]
        );
    }

    #[test]
    fn operand_dquote_simple_is_one_lit() {
        // `"a}b"` — no expansion: ONE quoted Lit (the `}` is literal because it's
        // inside `"…"`), then ParamClose.
        let a = operand_atoms("\"a}b\"}", Mode::ParamWordOperand { in_dquote: false, enclosing_dquote: false });
        assert_eq!(
            a,
            vec![
                TokenKind::Lit { text: "a}b".into(), quoted: true },
                TokenKind::ParamClose,
            ]
        );
    }

    #[test]
    fn operand_dquote_with_nested_expansion_splits() {
        // `"a${y}b"` — split into a quoted Lit for `a`, then ParamOpen (NOT
        // DeferredExpansion).  The parser would push ParamExpansion on ParamOpen;
        // this test confirms the lexer emits the correct flat tokens.
        let mut lx = Lexer::new("\"a${y}b\"}", LexerOptions::default(), true);
        lx.push_mode(Mode::ParamWordOperand { in_dquote: false, enclosing_dquote: false });
        assert_eq!(
            lx.next_token().unwrap().unwrap().kind,
            TokenKind::Lit { text: "a".into(), quoted: true }
        );
        assert_eq!(
            lx.next_token().unwrap().unwrap().kind,
            TokenKind::ParamOpen { quoted: true }
        );
    }

    #[test]
    fn operand_dquote_var_inside() {
        // `"$a"` — a DollarName token, not a DeferredExpansion.
        let mut lx = Lexer::new("\"$a\"}", LexerOptions::default(), true);
        lx.push_mode(Mode::ParamWordOperand { in_dquote: false, enclosing_dquote: false });
        assert_eq!(
            lx.next_token().unwrap().unwrap().kind,
            TokenKind::DollarName { name: "a".into(), quoted: true }
        );
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
        let seq = crate::command::parse(&mut Lexer::from_tokens(tokens)).expect("parse").expect("non-empty");
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
        let seq = crate::command::parse(&mut Lexer::from_tokens(tokens)).expect("parse").expect("non-empty");
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
        let seq = crate::command::parse(&mut Lexer::from_tokens(tokens)).expect("parse").expect("non-empty");
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
            let parts = match &tokenize(src).unwrap()[0].kind {
                TokenKind::Word(Word(p)) => p.clone(),
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
        assert!(matches!(&toks[0].kind,
            TokenKind::Word(Word(p)) if matches!(&p[0],
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
        let words: Vec<&Word> = toks.iter().filter_map(|t| match &t.kind {
            TokenKind::Word(w) => Some(w), _ => None,
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
        let w = toks.iter().find_map(|t| match &t.kind {
            TokenKind::Word(w) if matches!(w.0.first(), Some(WordPart::ProcessSub { .. })) => Some(w),
            _ => None,
        }).expect("a process-sub word");
        assert!(matches!(w.0[0], WordPart::ProcessSub { dir: ProcDir::Out, .. }));
    }

    #[test]
    fn quoted_process_sub_is_literal() {
        let toks = tokenize("echo \"<(echo hi)\"").unwrap();
        let has = toks.iter().any(|t| matches!(&t.kind,
            TokenKind::Word(w) if w.0.iter().any(|p| matches!(p, WordPart::ProcessSub { .. }))));
        assert!(!has, "quoted <( must stay literal");
    }

    #[test]
    fn lone_redirect_still_redirect() {
        let toks = tokenize("cat < file").unwrap();
        assert!(toks.iter().any(|t| matches!(&t.kind, TokenKind::Op(Operator::RedirIn))),
            "`< file` is still a redirect");
    }

    #[test]
    fn nested_process_sub_balances() {
        let toks = tokenize("cat <(cat <(echo deep))").unwrap();
        let outer = toks.iter().find_map(|t| match &t.kind {
            TokenKind::Word(w) if matches!(w.0.first(), Some(WordPart::ProcessSub { .. })) => Some(w),
            _ => None,
        }).expect("outer process sub");
        assert!(matches!(outer.0[0], WordPart::ProcessSub { dir: ProcDir::In, .. }));
    }

    #[test]
    fn redirect_from_process_sub_tokenizes() {
        // `wc < <(cmd)` -> Word("wc"), Op(RedirIn), Word(ProcessSub{In})
        let toks = tokenize("wc < <(printf hi)").unwrap();
        assert!(toks.iter().any(|t| matches!(&t.kind, TokenKind::Op(Operator::RedirIn))),
            "the standalone `<` is still a redirect operator");
        let last_word = toks.iter().rev().find_map(|t| match &t.kind {
            TokenKind::Word(w) => Some(w), _ => None,
        }).expect("a trailing word");
        assert!(matches!(last_word.0.first(), Some(WordPart::ProcessSub { dir: ProcDir::In, .. })),
            "the `<(printf hi)` is a process-sub word");
    }

    // --- bad-substitution lexer tests (v233) ---

    /// Extract the first WordPart from a single-word tokenization result.
    fn first_word_part(input: &str) -> WordPart {
        let mut toks = crate::lexer::tokenize(input).expect("lex");
        let word = match toks.remove(0).kind {
            TokenKind::Word(w) => w,
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

    #[test]
    fn extquote_name_decodes_to_identifier() {
        // `"${$'x1'}"` (double-quoted) -> name "x1"; unquoted is now bad subst (M-156).
        let toks = tokenize(r#""${$'x1'}""#).unwrap();
        let TokenKind::Word(Word(parts)) = &toks[0].kind else { panic!() };
        // Unwrap a possible outer Quoted wrapper.
        let inner = match &parts[0] {
            WordPart::Quoted { parts, .. } => &parts[0],
            other => other,
        };
        let (WordPart::ParamExpansion { name, .. } | WordPart::Var { name, .. }) = inner
        else { panic!("expected name-bearing part, got {:?}", inner) };
        assert_eq!(name, "x1");
    }

    #[test]
    fn extquote_name_concatenates() {
        // `"${a$'b'}"` (double-quoted) -> name "ab"; unquoted is now bad subst (M-156).
        let toks = tokenize(r#""${a$'b'}""#).unwrap();
        let TokenKind::Word(Word(parts)) = &toks[0].kind else { panic!() };
        let inner = match &parts[0] {
            WordPart::Quoted { parts, .. } => &parts[0],
            other => other,
        };
        let (WordPart::ParamExpansion { name, .. } | WordPart::Var { name, .. }) = inner
        else { panic!("got {:?}", inner) };
        assert_eq!(name, "ab");
    }

    #[test]
    fn extquote_locale_name_is_bad_subst() {
        // `${$"x1"}` -> bash bad substitution.
        let toks = tokenize(r#"${$"x1"}"#).unwrap();
        let TokenKind::Word(Word(parts)) = &toks[0].kind else { panic!() };
        assert!(matches!(parts[0], WordPart::ParamExpansion { modifier: ParamModifier::BadSubst { .. }, .. }));
    }

    #[test]
    fn extquote_decoded_invalid_name_is_bad_subst() {
        // `${$'x\ty'}` is UNQUOTED: fires at the quote-context gate (not the
        // invalid-name gate) — unquoted extquote name -> bad substitution.
        let toks = tokenize("${$'x\\ty'}").unwrap();
        let TokenKind::Word(Word(parts)) = &toks[0].kind else { panic!() };
        assert!(matches!(parts[0], WordPart::ParamExpansion { modifier: ParamModifier::BadSubst { .. }, .. }));
    }

    #[test]
    fn extquote_decoded_invalid_name_quoted_is_bad_subst() {
        // Inside `"…"` the quote-context gate PASSES (extquote allowed), but the
        // decoded name "x<TAB>y" is not a valid identifier -> bad substitution.
        // This exercises the invalid-name gate (the path `!is_valid_param_name`).
        let toks = tokenize(r#""${$'x\ty'}""#).unwrap();
        let TokenKind::Word(Word(parts)) = &toks[0].kind else { panic!() };
        let inner = match &parts[0] {
            WordPart::Quoted { parts, .. } => &parts[0],
            other => other,
        };
        assert!(
            matches!(inner, WordPart::ParamExpansion { modifier: ParamModifier::BadSubst { .. }, .. }),
            "expected BadSubst for invalid decoded name, got {inner:?}"
        );
    }

    #[test]
    fn extquote_name_unquoted_defers() {
        // Top-level unquoted `${$'x1'}` -> BadSubst (the default tokenize path
        // is unquoted).
        let toks = tokenize(r#"${$'x1'}"#).unwrap();
        let TokenKind::Word(Word(parts)) = &toks[0].kind else { panic!() };
        assert!(matches!(parts[0], WordPart::ParamExpansion { modifier: ParamModifier::BadSubst { .. }, .. }));
    }

    #[test]
    fn extquote_name_double_quoted_decodes() {
        // Inside `"…"` the name decodes (no BadSubst).
        let toks = tokenize(r#""${$'x1'}""#).unwrap();
        let TokenKind::Word(Word(parts)) = &toks[0].kind else { panic!() };
        // The single part is the decoded name `x1` (Var or ParamExpansion),
        // NOT a BadSubst.
        let inner = match &parts[0] {
            WordPart::Quoted { parts, .. } => &parts[0],
            other => other,
        };
        let name = match inner {
            WordPart::ParamExpansion { name, modifier, .. } => {
                assert!(!matches!(modifier, ParamModifier::BadSubst { .. }), "should not be BadSubst");
                name
            }
            WordPart::Var { name, .. } => name,
            other => panic!("expected name-bearing part, got {other:?}"),
        };
        assert_eq!(name, "x1");
    }

    #[test]
    fn pull_api_reproduces_token_sequence() {
        let toks = tokenize("echo foo | grep bar").unwrap();
        let mut lx = Lexer::from_tokens(toks.clone());
        assert_eq!(lx.remaining(), toks.len());
        assert_eq!(lx.peek_kind().unwrap(), Some(&toks[0].kind));
        assert_eq!(lx.peek2_kind().unwrap(), Some(&toks[1].kind));
        assert_eq!(lx.peek_span().unwrap(), Some(toks[0].span));
        let mut drained = Vec::new();
        while let Some(t) = lx.next().unwrap() { drained.push(t); }
        assert_eq!(drained, toks);
        assert_eq!(lx.peek_kind().unwrap(), None);
        assert_eq!(lx.next_kind().unwrap(), None);
    }

    #[test]
    fn pull_next_kind_matches_next_dot_kind() {
        let toks = tokenize("a b c").unwrap();
        let mut lx = Lexer::from_tokens(toks.clone());
        assert_eq!(lx.next_kind().unwrap(), Some(toks[0].kind.clone()));
        assert_eq!(lx.peek_kind().unwrap(), Some(&toks[1].kind));
    }

    #[test]
    fn pull_surfaces_lex_error_as_err() {
        // A genuinely unterminated construct: the pull returns Err at the failing scan.
        let mut lx = Lexer::new("echo \"unterminated", LexerOptions::default(), true);
        // drain until we hit the error
        let mut got_err = false;
        loop {
            match lx.next() {
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(_) => { got_err = true; break; }
            }
        }
        assert!(got_err, "unterminated quote must surface as Err from the pull");
    }

    // --- Task 4: alias storage + command-position expansion ---

    fn lx_with_alias(input: &str, pairs: &[(&str, &str)]) -> Lexer<'static> {
        let toks = tokenize(input).unwrap();
        let mut lx = Lexer::from_tokens(toks);
        let mut m = std::collections::HashMap::new();
        for (k, v) in pairs { m.insert(k.to_string(), v.to_string()); }
        lx.set_aliases(m);
        lx
    }

    fn wtext(k: &TokenKind) -> String {
        // Test helper: extract all literal text (including quoted parts) so that
        // quoted words like `'ll'` show as "ll" for assertion display. Recurses into
        // WordPart::Quoted wrappers (single/double/etc). Distinct from
        // word_literal_text (which returns None for quoted parts to block alias
        // expansion); this is for verifying WHAT was returned, not WHETHER to expand.
        fn extract(parts: &[WordPart], s: &mut String) {
            for part in parts {
                match part {
                    WordPart::Literal { text, .. } => s.push_str(text),
                    WordPart::Quoted { parts: inner, .. } => extract(inner, s),
                    _ => {}
                }
            }
        }
        if let TokenKind::Word(w) = k {
            let mut s = String::new();
            extract(&w.0, &mut s);
            s
        } else {
            String::new()
        }
    }

    #[test]
    fn alias_expands_at_command_position() {
        let mut lx = lx_with_alias("ll /tmp", &[("ll", "ls -l")]);
        lx.expand_command_alias().unwrap();
        assert_eq!(lx.peek_kind().unwrap().map(|k| wtext(k)), Some("ls".into()));
        lx.expand_command_alias().unwrap();
        assert_eq!(lx.next().unwrap().map(|t| wtext(&t.kind)), Some("ls".into()));
        assert_eq!(lx.next_kind().unwrap().map(|k| wtext(&k)), Some("-l".into()));
        assert_eq!(lx.next_kind().unwrap().map(|k| wtext(&k)), Some("/tmp".into()));
    }

    #[test]
    fn alias_not_expanded_at_argument_position() {
        let mut lx = lx_with_alias("echo ll", &[("ll", "ls -l")]);
        lx.expand_command_alias().unwrap();
        assert_eq!(lx.next().unwrap().map(|t| wtext(&t.kind)), Some("echo".into()));
        assert_eq!(lx.next_kind().unwrap().map(|k| wtext(&k)), Some("ll".into()));
    }

    #[test]
    fn alias_recursion_guard_terminates() {
        let mut lx = lx_with_alias("ls", &[("ls", "ls -a")]);
        lx.expand_command_alias().unwrap();
        assert_eq!(lx.next().unwrap().map(|t| wtext(&t.kind)), Some("ls".into()));
        assert_eq!(lx.next_kind().unwrap().map(|k| wtext(&k)), Some("-a".into()));
    }

    #[test]
    fn alias_trailing_blank_makes_next_word_eligible() {
        let mut lx = lx_with_alias("a c", &[("a", "b "), ("c", "d")]);
        lx.expand_command_alias().unwrap();
        assert_eq!(lx.next().unwrap().map(|t| wtext(&t.kind)), Some("b".into()));
        lx.expand_command_alias().unwrap();
        assert_eq!(lx.next().unwrap().map(|t| wtext(&t.kind)), Some("d".into()));
    }

    #[test]
    fn quoted_word_not_expanded() {
        let mut lx = lx_with_alias("'ll'", &[("ll", "ls")]);
        lx.expand_command_alias().unwrap();
        assert_eq!(lx.next().unwrap().map(|t| wtext(&t.kind)), Some("ll".into()));
    }

    #[test]
    fn bad_alias_body_returns_err() {
        let mut lx = lx_with_alias("x", &[("x", "echo \"")]); // unterminated quote in body
        assert!(lx.expand_command_alias().is_err());
    }

    // --- Task 7: incrementality + live-lexer error tests ---

    #[test]
    fn parser_pull_is_incremental_not_batch() {
        let input = (0..50).map(|i| format!("echo {i}")).collect::<Vec<_>>().join("\n");
        let empty = std::collections::HashMap::new();
        let mut lx = Lexer::new_live(&input, &empty, LexerOptions::default());
        let _ = crate::command::parse_one_unit(&mut lx).unwrap();
        assert!(lx.scanned_token_count() < 10, "scanned too much: not incremental");
    }

    #[test]
    fn bad_alias_body_surfaces_as_parse_error() {
        let mut m = std::collections::HashMap::new();
        m.insert("x".to_string(), "echo \"".to_string()); // unterminated quote in body
        let mut lx = Lexer::new_live("x", &m, LexerOptions::default());
        let r = crate::command::parse(&mut lx);
        assert!(matches!(r, Err(crate::command::ParseError::Lex(_))));
    }

    // --- Task 2 (v240): mark/rewind checkpoint tests ---

    #[test]
    fn rewind_reproduces_tokens_same_mode() {
        let mut lx = Lexer::new("echo one two; echo three", LexerOptions::default(), true);
        let m = lx.mark();
        let first: Vec<Token> = (0..4).map(|_| lx.next_token().unwrap().unwrap()).collect();
        lx.rewind(&m);
        let again: Vec<Token> = (0..4).map(|_| lx.next_token().unwrap().unwrap()).collect();
        // `Token` equality compares kind (v237's kind-only PartialEq); compare spans
        // separately to prove the cursor reset to the exact byte offsets.
        assert_eq!(first, again);
        let first_spans: Vec<Span> = first.iter().map(|t| t.span).collect();
        let again_spans: Vec<Span> = again.iter().map(|t| t.span).collect();
        assert_eq!(first_spans, again_spans);
    }

    #[test]
    fn rewind_across_buffered_lookahead() {
        let mut lx = Lexer::new("alpha beta gamma", LexerOptions::default(), true);
        // Buffer history[0] without consuming it (pos stays 0) so mark() resumes
        // from the buffered token's span, not the advanced cursor.
        lx.fill_to(0).unwrap();
        assert_eq!(lx.pos, 0);
        assert!(lx.history.len() >= 1);
        let m = lx.mark();
        let a = lx.next_token().unwrap().unwrap();
        let b = lx.next_token().unwrap().unwrap();
        lx.rewind(&m);
        let a2 = lx.next_token().unwrap().unwrap();
        let b2 = lx.next_token().unwrap().unwrap();
        assert_eq!(a, a2);
        assert_eq!(b, b2);
        assert_eq!((a.span, b.span), (a2.span, b2.span));
    }

    #[test]
    fn rewind_restores_line_and_column() {
        let mut lx = Lexer::new("a\nbb\nccc", LexerOptions::default(), true);
        let _ = lx.next_token().unwrap().unwrap(); // Word "a" (line 1)
        let _ = lx.next_token().unwrap().unwrap(); // Newline (line 1)
        let m = lx.mark();                         // at start of "bb" on line 2
        let bb1 = lx.next_token().unwrap().unwrap().span;
        assert_eq!(bb1.line, 2);
        lx.rewind(&m);
        let bb2 = lx.next_token().unwrap().unwrap().span;
        assert_eq!((bb1.offset, bb1.line, bb1.column), (bb2.offset, bb2.line, bb2.column));
    }

    #[test]
    fn rewind_restores_mode_stack() {
        let mut lx = Lexer::new("x", LexerOptions::default(), true);
        lx.push_mode(Mode::Arith { paren_depth: 0, in_squote: false, in_dquote: false, body_started: false, for_header: false, delim: ArithDelim::Paren });
        let m = lx.mark();
        lx.push_mode(Mode::CommandSub { body_started: false });
        assert_eq!(lx.current_mode(), Mode::CommandSub { body_started: false });
        lx.rewind(&m);
        assert_eq!(lx.current_mode(), Mode::Arith { paren_depth: 0, in_squote: false, in_dquote: false, body_started: false, for_header: false, delim: ArithDelim::Paren });
        assert_eq!(lx.pop_mode(), Mode::Arith { paren_depth: 0, in_squote: false, in_dquote: false, body_started: false, for_header: false, delim: ArithDelim::Paren });
        assert_eq!(lx.current_mode(), Mode::Command);
    }

    #[test]
    fn rewind_restores_scalar_flags() {
        let mut lx = Lexer::new("[[ $x =~ ab*c ]] && echo y", LexerOptions::default(), true);
        let _ = lx.next_token().unwrap().unwrap(); // [[
        let _ = lx.next_token().unwrap().unwrap(); // $x
        assert_eq!(lx.dbracket_depth, 1);          // inside [[ … ]]
        let m = lx.mark();
        while lx.next_token().unwrap().is_some() {} // drain to EOF; depth returns to 0
        assert_eq!(lx.dbracket_depth, 0);
        lx.rewind(&m);
        assert_eq!(lx.dbracket_depth, 1);          // restored from the snapshot
    }

    #[test]
    fn rewind_on_replay_lexer_does_not_truncate() {
        let toks = tokenize_with_opts("echo hi there", LexerOptions::default()).unwrap();
        let mut lx = Lexer::from_tokens(toks);
        let m = lx.mark();
        let a = lx.next_token().unwrap().unwrap();
        let _ = lx.next_token().unwrap().unwrap();
        let len_before = lx.history.len();
        lx.rewind(&m);
        assert_eq!(lx.history.len(), len_before); // replay history is NOT truncated
        let a2 = lx.next_token().unwrap().unwrap();
        assert_eq!(a, a2);
        assert_eq!(a.span, a2.span);
    }
}

