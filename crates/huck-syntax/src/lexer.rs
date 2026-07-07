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
    /// The scan produced tokens without consuming any input for
    /// `SCAN_STALL_CAP` steps in a row — a forward-progress safety net against a
    /// zero-width opener signal re-emitted with no parser to consume it.
    NoProgress,
}

impl std::fmt::Display for LexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&crate::errors::lex_error_message_impl(self))
    }
}

impl std::error::Error for LexError {}

/// Forward-progress guard: max consecutive `scan_step` calls that PRODUCE a
/// token while consuming zero input chars before `LexError::NoProgress`.
const SCAN_STALL_CAP: u32 = 1024;

/// History prune threshold: prune the consumed prefix once `pos` reaches this
/// many tokens, bounding the buffer to ~one command's worth (the "at most ~1000
/// tokens" target). Not a hard cap — a single giant command still buffers O(command).
pub(crate) const HISTORY_PRUNE_THRESHOLD: usize = 1024;

/// A char cursor over a `&str` that also tracks the byte offset and 1-based
/// line number of the next char to be produced. Drop-in for the
/// `Peekable<Chars>` the lexer used: implements `Iterator<Item = char>`, a
/// `peek()` returning `Option<&char>`, `Clone`, and (via `Iterator`) `by_ref()`.
/// `offset()` is the byte position of the char that the next `next()`/`peek()`
/// will yield (or `s.len()` at end). `line()` is the 1-based line of that same
/// char — it advances to the next line immediately after a `'\n'` is consumed,
/// exactly mirroring how `offset()` advances after each byte.
/// One injected input source pushed onto `CharCursor`'s source stack (v266:
/// the alias input-source stack). While a frame is active the cursor reads its
/// `body` inline — emitting normal atoms + zero-width opener signals the parser
/// consumes — instead of the base `s`; when the body is exhausted the frame is
/// popped and reading resumes in the parent source. An injected body has no
/// base-input offset space of its own, so every token produced while it is
/// active reports the FROZEN `anchor_*` coordinates captured at push time (the
/// span of the alias-name invocation), exactly as the old history-splice set
/// `name_span` on each spliced token. `alias_name` lets the recursion guard
/// derive the active-set membership directly from the live stack.
#[derive(Clone)]
struct Injected {
    body: String,
    pos: usize,
    peeked: Option<char>,
    peeked_len: usize,
    anchor_off: usize,
    anchor_line: u32,
    anchor_col: u32,
    alias_name: String,
}

impl Injected {
    /// True when the body is fully read (no buffered peek and `pos` at the end).
    fn exhausted(&self) -> bool {
        self.peeked.is_none() && self.pos >= self.body.len()
    }
}

#[derive(Clone)]
pub struct CharCursor<'a> {
    s: &'a str,
    pos: usize,
    line: u32,
    column: u32,            // NEW: 1-based character column
    peeked: Option<char>,
    peeked_len: usize,
    /// v266: alias input-source stack. Top of stack is the active source;
    /// empty ⇒ read the base `s`. See [`Injected`].
    injected: Vec<Injected>,
    /// Monotonic count of chars yielded by `next()` — main string AND injected
    /// (alias-body) chars. Progress metric for the forward-progress guard; never
    /// decremented (rewind/seek reposition, they do not consume).
    consumed: u64,
}

impl<'a> CharCursor<'a> {
    pub fn new(s: &'a str) -> Self {
        CharCursor { s, pos: 0, line: 1, column: 1, peeked: None, peeked_len: 0, injected: Vec::new(), consumed: 0 }
    }

    /// v266: inject `body` as the active input source, read fully before the
    /// current source resumes. `anchor` is the span the produced tokens report
    /// (the alias-NAME invocation site) — frozen for the frame's lifetime, since
    /// the body has no base-offset space of its own. `alias_name` anchors the
    /// recursion guard to this stack frame.
    fn push_injection(&mut self, body: String, alias_name: String, anchor: Span) {
        self.injected.push(Injected {
            body,
            pos: 0,
            peeked: None,
            peeked_len: 0,
            anchor_off: anchor.offset,
            anchor_line: anchor.line,
            anchor_col: anchor.column,
            alias_name,
        });
    }

    /// v266: true iff some frame currently on the injection stack was pushed for
    /// alias `name`. The recursion guard reads this INSTEAD of a separate `active`
    /// set: a frame is on the stack for exactly as long as its body is being lexed,
    /// so membership == "this alias is mid-expansion" with no manual insert/remove.
    fn injected_has_alias(&self, name: &str) -> bool {
        self.injected.iter().any(|f| f.alias_name == name)
    }


    /// Peek the next char without consuming it. v266: reads from the topmost
    /// injected frame that still has content, seeing THROUGH fully-exhausted
    /// frames to the parent WITHOUT popping them (popping — and the matching
    /// recursion-guard release — is `next()`'s job, so a lookahead peek never
    /// releases an alias early).
    pub fn peek(&mut self) -> Option<&char> {
        // Find the topmost frame with remaining content (skip exhausted ones).
        let mut idx = self.injected.len();
        while idx > 0 && self.injected[idx - 1].exhausted() {
            idx -= 1;
        }
        if idx > 0 {
            let f = &mut self.injected[idx - 1];
            if f.peeked.is_none()
                && let Some(c) = f.body[f.pos..].chars().next()
            {
                f.peeked = Some(c);
                f.peeked_len = c.len_utf8();
            }
            return self.injected[idx - 1].peeked.as_ref();
        }
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
    /// v266: while an injected alias body is active AND still has content, this
    /// reports the frozen anchor (the alias-name site) so body tokens pin there;
    /// once all injected frames are drained it reports the base position again
    /// (all nested frames share the same anchor, so the topmost-with-content
    /// frame's anchor is the outermost alias-name span).
    pub fn offset(&self) -> usize {
        for f in self.injected.iter().rev() {
            if !f.exhausted() {
                return f.anchor_off;
            }
        }
        self.pos
    }

    /// 1-based line of the next char to be produced (mirrors `offset()`).
    /// After consuming a `'\n'`, this reflects the NEXT line.
    pub fn line(&self) -> u32 {
        for f in self.injected.iter().rev() {
            if !f.exhausted() {
                return f.anchor_line;
            }
        }
        self.line
    }

    /// 1-based character column of the next char to be produced.
    pub fn column(&self) -> u32 {
        for f in self.injected.iter().rev() {
            if !f.exhausted() {
                return f.anchor_col;
            }
        }
        self.column
    }

    /// Byte slice of the base source from `start` to the current offset. Used to
    /// reconstruct the raw `${…}` text for a deferred bad-substitution. v266:
    /// base-only — a raw-slice reconstruction is not expected to straddle an
    /// injected alias body (a malformed `${` inside an alias body is out of
    /// scope; see the module note). Debug builds assert the assumption.
    pub fn slice_from(&self, start: usize) -> &str {
        debug_assert!(self.injected.is_empty(), "slice_from must not straddle an injected alias body");
        &self.s[start..self.pos]
    }

    /// Reposition the cursor to a byte offset with explicit line/column, clearing
    /// any pending 1-char peek. Used by `Lexer::rewind` to re-lex from a checkpoint.
    /// v266: base-only — mark/rewind is not expected to straddle an injected alias
    /// body (the only live command-path rewind is the arith `$((`-bail, which does
    /// not occur mid-alias-body in practice; see the module note). Debug builds
    /// assert the assumption.
    pub fn seek(&mut self, offset: usize, line: u32, column: u32) {
        debug_assert!(self.injected.is_empty(), "seek must not straddle an injected alias body");
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
    /// v266: reads the topmost injected frame with content (a buffered `peeked`
    /// equals `body[pos]`, so reading `body[pos..]` covers it); does not chain a
    /// bounded lookahead across a frame boundary into the parent (alias bodies do
    /// not end mid-lookahead in practice).
    pub fn peek_nth(&self, n: usize) -> Option<char> {
        let src: &str = {
            let mut chosen: Option<&str> = None;
            for f in self.injected.iter().rev() {
                if f.pos < f.body.len() {
                    chosen = Some(&f.body[f.pos..]);
                    break;
                }
            }
            // `self.pos` always points at the raw byte offset of the next char
            // (the base `peeked` buffer, if any, starts at `self.pos`).
            chosen.unwrap_or(&self.s[self.pos..])
        };
        let mut it = src.chars();
        for _ in 0..n {
            it.next()?;
        }
        it.next()
    }
}

impl Iterator for CharCursor<'_> {
    type Item = char;
    fn next(&mut self) -> Option<char> {
        // v266: pop fully-exhausted injected frames (their bodies are drained);
        // dropping a frame releases its alias from the recursion guard, since the
        // guard derives membership from the live stack (`injected_has_alias`).
        while self.injected.last().is_some_and(Injected::exhausted) {
            self.injected.pop();
        }
        let c = if let Some(f) = self.injected.last_mut() {
            if let Some(c) = f.peeked.take() {
                f.pos += f.peeked_len;
                f.peeked_len = 0;
                Some(c)
            } else {
                // Not exhausted (the pop loop above guaranteed remaining content).
                let c = f.body[f.pos..].chars().next().expect("injected frame has content");
                f.pos += c.len_utf8();
                Some(c)
            }
        } else if let Some(c) = self.peeked.take() {
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
        };
        if c.is_some() {
            self.consumed += 1;
        }
        c
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SubstKind { First, All, Prefix, Suffix }   // / , // , /# , /%

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
    /// v268: the `=` / `+=` that follows a command-word-start subscript
    /// (`name[sub]=`). Emitted by the command scanner ONLY when
    /// `pending_lvalue_subscript` is set; the parser consumes it to confirm an
    /// indexed assignment and then calls `begin_assignment_value`.
    AssignEq { append: bool },
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
pub(crate) enum Mode {
    Command,        // default command-position scanner (emits atoms)
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
    Regex { paren_depth: u32, body_started: bool },  // v254: RHS of =~ inside [[ … ]]
    /// v264: an extglob group `<prefix>( … )` (`?(...)`/`*(...)`/`+(...)`/
    /// `@(...)`/`!(...)`, gated by `LexerOptions::extglob`). `paren_depth == 0`
    /// on push means "not yet entered" — the first scan consumes the prefix
    /// char + `(` and sets it to 1; the group closes (and this mode is popped)
    /// when a `)` brings `paren_depth` back to 0.
    Extglob { paren_depth: u32 },
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
    /// v268: see `Lexer::pending_lvalue_subscript`. Captured/restored so an
    /// arith mark/rewind spanning the flag's brief lifetime cannot desync it.
    pending_lvalue_subscript: bool,
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
    /// v268: set when the command scanner emitted a word-start `name[` `LBracket`;
    /// checked once, on the first command scan step after the parser assembles the
    /// subscript and consumes `RBracket`, to emit `AssignEq` (→ indexed assignment)
    /// or clear (→ ordinary glob word).
    pending_lvalue_subscript: bool,
    /// Consecutive `Produced` scan steps that consumed no input (see
    /// `scan_step_guarded` / `SCAN_STALL_CAP`).
    stall_steps: u32,
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
            alias_trailing_eligible: false,
            modes: vec![Mode::Command],
            retokenize_arith_as_cmdsub: false,
            cmd_at_word_start: true,
            assign_val_tilde_ok: false,
            pending_lvalue_subscript: false,
            stall_steps: 0,
            heredoc_gen: 0,
        }
    }

    pub(crate) fn current_mode(&self) -> Mode {
        *self.modes.last().expect("mode stack is never empty (Command is the floor)")
    }

    /// v264 flip-fix (Finding 1): read the brace-expand flag (`set +B` clears it).
    /// The atom command-assembly loop consults this to gate command-word brace
    /// expansion, mirroring the oracle's `emit_word_with_braces(self.brace_expand)`.
    pub(crate) fn brace_expand_enabled(&self) -> bool {
        self.brace_expand
    }

    /// v264 flip-fix (Finding 2): force mid-word state after a `$( )`/`<( )`/`>( )`
    /// close when the surrounding word CONTINUES. The `)` that ends a command/
    /// process substitution runs `boundary_reset()` (→ `cmd_at_word_start = true`),
    /// which leaks into the outer word continuation and mis-classifies a glued
    /// `#`/`~`. Callers invoke this only on the cmdsub/procsub continuation path,
    /// never for a subshell-close `)`, so `(cmd); next` still arms word-start.
    pub(crate) fn clear_cmd_at_word_start(&mut self) {
        self.cmd_at_word_start = false;
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

    pub(crate) fn push_mode(&mut self, m: Mode) {
        self.modes.push(m);
    }

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
            pending_lvalue_subscript: self.pending_lvalue_subscript,
            heredoc_gen: self.heredoc_gen,
        }
    }

    /// Restore a `Mark`: discard buffered/produced tokens at/after it, seek the
    /// cursor back, and restore flags + mode stack. The next pull re-lexes from
    /// the checkpoint under the now-current mode. A replay (`from_tokens`) lexer
    /// never scans, so history is left intact and only `pos`/flags are reset.
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
        self.pending_lvalue_subscript = m.pending_lvalue_subscript;
        debug_assert_eq!(
            self.heredoc_gen, m.heredoc_gen,
            "mark/rewind must not span heredoc-body emission (v250)"
        );
    }

    /// v268 fix: clear `pending_lvalue_subscript` after a rewind on the
    /// "never validly closed" path in the parser's `LBracket` arm
    /// (`parse_word_command`). The mark there is taken AFTER the lexer sets
    /// the flag `true` (emitting the zero-width `LBracket` for `name[`), so a
    /// plain `rewind(&mark)` restores it to `true` — and the flag-check hook
    /// in `scan_step_command_atoms` then re-fires on the very next scan,
    /// misreading a bare `=`/`+=` right after `[` as a spurious `AssignEq`
    /// even though the parser has just decided this is NOT an assignment.
    /// Call this right after `rewind` on that path only; the success
    /// (assignment) and glob-fold-back paths never take this route and don't
    /// need it.
    pub(crate) fn clear_pending_lvalue_subscript(&mut self) {
        self.pending_lvalue_subscript = false;
    }

    /// Drop the consumed prefix `history[0..pos]` and reset `pos` to 0, bounding
    /// the buffer to the live (unconsumed) tail. Acts only once `pos` crosses
    /// `HISTORY_PRUNE_THRESHOLD`, to avoid churn.
    ///
    /// SAFETY (why `pos`-reset can't invalidate an outstanding `Mark`): the only
    /// `Mark`s the parser takes are the arith disambiguation marks in
    /// `parse_arith_expansion` / `parse_arith_command`, and an arith mark's entire
    /// lifetime is spent with `Mode::Arith` pushed (`modes.len() >= 2`). By pruning
    /// only at genuine top level (`modes.len() == 1`), no prune can ever fire while
    /// such a mark is live, so a later `rewind(&mark)` never sees a stale `pos`.
    /// (A nested `$((… $({compound})…))` reaches `parse_and_or_opts` — hence this
    /// call site — with the arith mark still outstanding; the depth guard is what
    /// makes that safe.) No-op for a replay lexer, and skipped while any heredoc
    /// body is pending (`pending_heredocs` stores history `token_idx`; a
    /// mid-collection body must not have its prefix shifted).
    pub(crate) fn maybe_prune_history(&mut self) {
        if self.replay
            || self.modes.len() > 1
            || self.pos < HISTORY_PRUNE_THRESHOLD
            || !self.pending_heredocs.is_empty()
            || !self.atom_pending_heredocs.is_empty()
        {
            return;
        }
        self.history.drain(0..self.pos);
        self.pos = 0;
    }

    /// Scan one step under the current mode. v241 T2 implements `ParamExpansion`;
    /// v241 T3 implements the four operand modes; remaining Phase C modes are
    /// forward declarations (never pushed in production).
    fn scan_step(&mut self) -> Result<Step, LexError> {
        match self.current_mode() {
            Mode::Command => self.scan_step_command_atoms(),
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
        }
    }

    /// Wraps `scan_step` with a forward-progress guard: if a step PRODUCES a
    /// token without consuming any input char (`cursor.consumed` unchanged) more
    /// than `SCAN_STALL_CAP` times in a row, return `LexError::NoProgress` instead
    /// of looping forever. Catches a zero-width opener signal re-emitted with no
    /// parser to consume it (the v266-resume OOM). Any step that consumes input
    /// resets the counter, so normal parser-driven flow never trips it.
    fn scan_step_guarded(&mut self) -> Result<Step, LexError> {
        let before = self.cursor.consumed;
        let step = self.scan_step()?;
        if matches!(step, Step::Produced) {
            if self.cursor.consumed == before {
                self.stall_steps += 1;
                if self.stall_steps > SCAN_STALL_CAP {
                    return Err(LexError::NoProgress);
                }
            } else {
                self.stall_steps = 0;
            }
        }
        Ok(step)
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

    /// v247 atom-emitting Command scanner (dormant). Built up across T2–T6:
    /// word-atoms + `Blank` splitting (T2), command-position expansions (T3),
    /// assignments (T4), redirects/operators/comments (T5), compounds (T6).
    /// Atom-native: at `$(`/`${`/`` ` ``/`$((` it emits the opener SIGNAL and the
    /// parser pushes the sub-mode — it never calls the fat scanners.
    fn scan_step_command_atoms(&mut self) -> Result<Step, LexError> {
        // v268: first command scan step after the parser assembled a word-start
        // `name[sub]` subscript, back at the TRUE `Mode::Command` floor (this
        // wrapper — unlike `scan_step_command_atoms_core` — is reached ONLY via
        // `scan_step`'s `Mode::Command` dispatch arm, i.e. exactly when
        // `self.modes.len() == 1`; nested cmdsub/backtick body scanning reuses
        // the core through `scan_step_command_body` instead, at a deeper mode
        // stack, so it never sees this check even though the flag stays `true`
        // for the subscript's full lifetime, e.g. across a `$(...)` inside it —
        // `a[$(echo 2)]=v`'s nested "echo" atom must NOT consume the flag).
        // If `=`/`+=` immediately follows `]`, emit AssignEq (→ indexed
        // assignment); otherwise it was a glob word — clear and fall through to
        // ordinary scanning. Checked BEFORE the blank-skip so `a[i] =v` (space)
        // is NOT an assignment.
        if self.pending_lvalue_subscript {
            self.pending_lvalue_subscript = false;
            let off = self.cursor.offset(); let l = self.cursor.line(); let c = self.cursor.column();
            match self.cursor.peek().copied() {
                Some('=') => {
                    self.cursor.next();
                    self.history.push(Token::new(TokenKind::AssignEq { append: false }, Span::new(off, l, c)));
                    return Ok(Step::Produced);
                }
                Some('+') if { let mut p = self.cursor.clone(); p.next(); p.peek() == Some(&'=') } => {
                    self.cursor.next(); self.cursor.next(); // `+` `=`
                    self.history.push(Token::new(TokenKind::AssignEq { append: true }, Span::new(off, l, c)));
                    return Ok(Step::Produced);
                }
                _ => { /* glob: fall through to normal scanning below */ }
            }
        }
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
    /// heredoc body. (v268: also — deliberately — without the
    /// `pending_lvalue_subscript` check above, which lives in the wrapper so it
    /// fires only at the TRUE `Mode::Command` floor; see that check's comment.)
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
    ///   - `name[`         → (v268) the lexer no longer decides indexed-lvalue vs.
    ///                        glob word itself: it emits `Lit name` + a zero-width
    ///                        `LBracket` and sets `pending_lvalue_subscript`. The
    ///                        PARSER assembles the subscript under
    ///                        `Mode::ParamSubscriptOperand` and, once it sees the
    ///                        closing `]`, the lexer's `pending_lvalue_subscript`
    ///                        hook (top of `scan_step_command_atoms_core`) emits
    ///                        `AssignEq` iff `=`/`+=` immediately follows — that is
    ///                        what the parser uses to decide assignment-vs-glob.
    ///
    /// Sets `in_assignment_value` and seeds `assign_val_tilde_ok` (true only after
    /// the bare `name=`, whose buffer ends in `=`; false after `+=`, whose buffer
    /// is empty). The `name[` arm does neither — that happens later, in
    /// `begin_assignment_value`, once the parser has confirmed the assignment.
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
            // `name[` at command-word-start — a possible indexed lvalue OR an
            // ordinary glob word (`a[bc]`). The lexer no longer decides: emit the
            // name and a zero-width `LBracket`, set `pending_lvalue_subscript`, and
            // let the PARSER assemble the subscript and decide by the trailing
            // `AssignEq` (v268 — severs the old forward-scan + parse_fragment_word).
            Some('[') => {
                for _ in 0..name.len() { self.cursor.next(); }
                self.cursor.next(); // consume `[`
                self.cmd_at_word_start = false;
                self.pending_lvalue_subscript = true;
                self.history.push(Token::new(TokenKind::Lit { text: name, quoted: false }, Span::new(off, l, c)));
                let (bo, bl, bc) = (self.cursor.offset(), self.cursor.line(), self.cursor.column());
                self.history.push(Token::new(TokenKind::LBracket, Span::new(bo, bl, bc)));
                Ok(Some(Step::Produced))
            }
            _ => Ok(None),
        }
    }

    /// v268: enter assignment-value lexing for an indexed lvalue whose `AssignEq`
    /// the parser just consumed. Mirrors the value-mode state the old Indexed arm
    /// of `try_scan_assign_prefix` set, with `assign_val_tilde_ok = true` (D2 fix:
    /// a leading `~` in `a[i]=~/x` now expands, matching the scalar `name=` arm).
    /// Also runs the compound-array `(` probe so `a[i]=(…)` pushes `Mode::ArrayLiteral`.
    pub(crate) fn begin_assignment_value(&mut self, _append: bool) {
        self.cmd_at_word_start = false;
        self.in_assignment_value = true;
        self.assign_val_tilde_ok = true;
        skip_line_continuations(&mut self.cursor);
        if self.cursor.peek() == Some(&'(') {
            let (ao, al, ac) = (self.cursor.offset(), self.cursor.line(), self.cursor.column());
            self.history.push(Token::new(TokenKind::ArrayOpen, Span::new(ao, al, ac)));
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
    ///
    /// Test-only since the oracle was removed: production pulls via
    /// `fill_to`/`next`/`peek` (which drive `scan_step` directly). Retained to
    /// exercise the incremental single-token pull in the v238 lexer tests.
    #[cfg(test)]
    fn next_token(&mut self) -> Result<Option<Token>, LexError> {
        loop {
            if self.pos < self.history.len() && !self.backfill_pending_at(self.pos) {
                let t = self.history[self.pos].clone();
                self.pos += 1;
                return Ok(Some(t));
            }
            match self.scan_step_guarded()? {
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
            match self.scan_step_guarded()? {
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

    /// The kind of the last significant token PULLED INTO history (skipping the
    /// inter-token `Blank`/`Newline` atoms), or `None` if none was pulled.
    ///
    /// Reads already-buffered `history` only — it NEVER scans. This is the
    /// atom-path replacement for the old `tokenize(...).last()` check used by
    /// REPL continuation classification to detect a trailing `|`/`&&`/`||`
    /// connector. A fresh standalone lex scan cannot be used for that: the atom
    /// lexer emits zero-width *opener signals* (`$((`→`ArithOpen`, `$(`→
    /// `CmdSubOpen`, `${`→`ParamOpen`, backtick→`BeginBacktick`) that the PARSER
    /// is responsible for consuming and pushing a mode for; driven without a
    /// parser the cursor never advances past the `$`, so `next()` re-emits the
    /// same signal forever (unbounded `history` growth). Because the parser has
    /// already driven mode-pushing while producing `self`, the connector — the
    /// buffer's last token — is guaranteed to have been pulled (consumed or via
    /// lookahead), so inspecting `history` here is both correct and bounded.
    pub fn last_significant_kind(&self) -> Option<&TokenKind> {
        self.history
            .iter()
            .rev()
            .map(|t| &t.kind)
            .find(|k| !matches!(k, TokenKind::Blank | TokenKind::Newline))
    }

    /// True when a heredoc redirect (`<<EOF`) was scanned but its body was never
    /// supplied — the input ended on the redirect LINE, before any newline could
    /// trigger body collection, so the heredoc still sits in the atom-path
    /// pending queue unattached.
    ///
    /// Used by REPL continuation classification: in SCRIPT mode a bare `cat
    /// <<EOF` is a complete (empty-body) command — bash warns and huck's atom
    /// parser returns `Ok` to match — but INTERACTIVELY bash prompts `>` for the
    /// body, so the REPL must treat it as incomplete. The old whole-buffer
    /// `tokenize` reported this as `UnterminatedHeredoc`; the fused atom path
    /// does not, so classification checks this predicate explicitly.
    pub fn has_unattached_heredoc(&self) -> bool {
        !self.atom_pending_heredocs.is_empty()
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

    /// Expand a registered alias at command position by pushing its body onto the
    /// lexer's input-source stack (v266). Implements bash's read-time alias rules
    /// (recursion guard, trailing-blank, span inheritance).
    ///
    /// The body is NOT re-lexed standalone (that would spin forever on a `$(`/`${`
    /// opener — those atoms are zero-width signals the PARSER consumes). Instead
    /// `CharCursor::push_injection` makes the body the active input source; the
    /// next parser-driven pull lexes it inline, emitting normal atoms + openers
    /// the parser drains, and the cursor pops back to the parent source at the
    /// body's end. Every token produced while the body is active reports the
    /// alias-NAME span (the frozen `anchor_*` on the injected frame). The
    /// recursion guard is the live injection stack itself: `name` is "active" for
    /// exactly as long as its body frame is on the stack, so a body whose leading
    /// word is `name` cannot re-expand it (`injected_has_alias`), and a body
    /// containing a `;`-separated re-use of `name` is guarded too — matching
    /// bash's whole-body recursion prevention.
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
            // `Lit` is the ENTIRE word, nothing glued on) happens below.
            TokenKind::Lit { text, quoted: false } => {
                (text.clone(), true)
            }
            _ => return Ok(()),
        };
        if is_bare_atom_lit {
            // The `Lit` is a maximal unquoted run; it is the ENTIRE command word
            // iff what follows it is a WORD BOUNDARY. There are two ways to read
            // the follower, and which is correct depends on whether the parser
            // has already looked ahead:
            //
            // * If a follower is ALREADY buffered (`history.len() > pos + 1`, e.g.
            //   `case`'s `peek2` for `;;`), the char cursor sits after the LAST
            //   buffered token — NOT after this word — so consulting it would
            //   sample the wrong char. Read the buffered follower TOKEN instead:
            //   boundary iff its kind is `Blank`/`Newline`/`ArrayClose`/`Op`/EOF
            //   (the atom analogue of the old oracle follower check). Any other
            //   atom (`Lit`/`DollarName`/`ParamOpen`/`CmdSubOpen`/`BeginDquote`/…)
            //   means the word has more parts and is not a bare literal name.
            // * If NOTHING is buffered past the command word (`history.len() ==
            //   pos + 1`), the cursor sits exactly on the stop char; read it
            //   directly (whitespace / `; | & < > ( )` / EOF). This avoids
            //   scanning a follower into `history` ahead of the injection point
            //   in the common no-lookahead case (a scanned follower would shift
            //   to `pos` on `history.remove` and be emitted before the body).
            let boundary = if self.history.len() > self.pos + 1 {
                matches!(
                    self.history.get(self.pos + 1).map(|t| &t.kind),
                    None | Some(TokenKind::Blank)
                        | Some(TokenKind::Newline)
                        | Some(TokenKind::ArrayClose)
                        | Some(TokenKind::Op(_))
                )
            } else {
                match self.cursor.peek().copied() {
                    None => true,
                    Some(c) => c.is_whitespace() || matches!(c, ';' | '|' | '&' | '<' | '>' | '(' | ')'),
                }
            };
            if !boundary { return Ok(()); }
        }
        // Recursion guard: `name` is active iff its body frame is still on the
        // injection stack (derived, not a separate set) — no re-expand while
        // mid-expansion. The leading body word is checked below during the
        // re-drive, when this just-pushed frame is present, so it cannot loop.
        if self.cursor.injected_has_alias(&name) { return Ok(()); }
        let Some(body) = self.aliases.get(&name).cloned() else { return Ok(()) };
        // Remove the already-produced command-word `Lit`/`Word` token; the pushed
        // body re-lexes atoms in its place. The cursor was NOT advanced past the
        // name's follower (no `fill_to(pos+1)`), so pushing the body and reading
        // it, then popping back, resumes the parent source exactly after `name`.
        self.history.remove(self.pos);
        // Push the body as the active source, pinning its tokens to the
        // alias-name span. For a NESTED expansion `name_span` is already the
        // outer frozen anchor (the nested name `Lit` was produced under the outer
        // frame), so the anchor propagates outward-most automatically.
        self.cursor.push_injection(body.clone(), name.clone(), name_span);
        // Re-drive: lex the body's first command word so a DIFFERENT leading alias
        // still expands. `name` is now on the stack, so it cannot re-expand itself.
        self.maybe_expand_command_alias()?;
        // bash's trailing-blank rule: a body ending in whitespace makes the NEXT
        // command-word eligible for expansion (carried by `take_trailing_eligible`).
        // Set AFTER the re-drive so the OUTERMOST body in a nested chain wins (the
        // re-drive's own assignment for an inner body is overwritten here), exactly
        // as the old post-recursion ordering did.
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
        Lexer::new_live(input, aliases, opts)
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


/// True when `s` ends with an odd-length run of backslashes — the final
/// backslash is unescaped and acts as a line-continuation marker.
pub fn ends_with_continuation_backslash(s: &str) -> bool {
    s.chars().rev().take_while(|&c| c == '\\').count() % 2 == 1
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



fn is_user_name_start(c: char) -> bool {
    c == '_' || c.is_ascii_alphabetic()
}

fn is_user_name_continue(c: char) -> bool {
    c == '_' || c.is_ascii_alphanumeric()
}

/// 1-based line number of byte offset `off` within `src`
/// (1 + the count of '\n' bytes before `off`). Clamps `off` to `src.len()`.
/// Used in tests and for isolated single-offset lookups.
pub fn line_at_offset(src: &str, off: usize) -> u32 {
    1 + src.as_bytes()[..off.min(src.len())].iter().filter(|&&b| b == b'\n').count() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexer_has_no_production_parser_dependency() {
        // The one-way front-end invariant: no `crate::parser` reference in non-test
        // lexer code. Reads this source file and checks every `crate::parser` line is
        // inside the `#[cfg(test)]` tests module.
        let src = include_str!("lexer.rs");
        let test_mod = src.find("mod tests").unwrap_or(src.len());
        let prod = &src[..test_mod];
        assert!(!prod.contains("crate::parser"),
            "lexer production code must not reference crate::parser (one-way invariant)");
    }

    #[test]
    fn begin_assignment_value_sets_value_mode_and_tilde() {
        let empty = std::collections::HashMap::new();
        let mut lx = Lexer::new_live_atoms("x", &empty, LexerOptions::default());
        lx.begin_assignment_value(false);
        assert!(lx.in_assignment_value);
        assert!(lx.assign_val_tilde_ok, "D2: tilde enabled for indexed value");
        assert!(!lx.cmd_at_word_start);
    }

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
    fn char_cursor_consumed_counts_yielded_chars() {
        let mut c = CharCursor::new("abc");
        assert_eq!(c.consumed, 0);
        c.next();
        c.next();
        assert_eq!(c.consumed, 2);
        c.next(); // "c"
        assert_eq!(c.consumed, 3);
        assert_eq!(c.next(), None); // EOF must NOT bump
        assert_eq!(c.consumed, 3);
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


    fn word_text(t: &Token) -> Option<String> {
        // v266: atom-aware. A command-position word is a bare `Lit` atom; a
        // brace-expansion product is still a single-literal `Word`. Accept both.
        match &t.kind {
            TokenKind::Lit { text, quoted: false } => Some(text.clone()),
            TokenKind::Word(Word(parts))
                if parts.len() == 1
                    && matches!(&parts[0], WordPart::Literal { quoted: false, .. }) =>
            {
                if let WordPart::Literal { text, .. } = &parts[0] {
                    Some(text.clone())
                } else {
                    None
                }
            }
            _ => None,
        }
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
    fn char_cursor_offset_with_multibyte() {
        // 'é' is 2 bytes in UTF-8.
        let mut c = CharCursor::new("é!");
        assert_eq!(c.offset(), 0);
        assert_eq!(c.next(), Some('é'));
        assert_eq!(c.offset(), 2);
        assert_eq!(c.next(), Some('!'));
        assert_eq!(c.offset(), 3);
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
        // Repeated single-token reads return the ordered atom stream. v266: the
        // atom stream carries explicit `Blank` atoms between words (the old
        // Word-lexer absorbed them), so filter those and check the word/op order.
        let toks: Vec<Token> = drain("echo foo | grep bar")
            .into_iter()
            .filter(|t| !matches!(t.kind, TokenKind::Blank))
            .collect();
        assert_eq!(word_text(&toks[0]).as_deref(), Some("echo"));
        assert_eq!(word_text(&toks[1]).as_deref(), Some("foo"));
        assert_eq!(toks[2].kind, TokenKind::Op(Operator::Pipe));
        assert_eq!(word_text(&toks[3]).as_deref(), Some("grep"));
        assert_eq!(word_text(&toks[4]).as_deref(), Some("bar"));
        assert_eq!(toks.len(), 5, "unexpected extra tokens: {toks:?}");
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


    // v266: `next_token_brace_expansion_drains_one_at_a_time` and
    // `next_token_heredoc_body_complete_when_emitted` removed — both asserted the
    // pre-atom Word-lexer's raw `next_token` stream (command-word brace expansion
    // and heredoc-body backfill are now parser-mediated on the atom path). The
    // behaviors are covered end-to-end by the brace/heredoc bash-diff harnesses
    // and the atom-path tests in `array_parse_tests`.




















































































































































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



    // Local test helper: concatenate the literal text of a Word's parts
    // (expansions render as a placeholder so structure tests stay simple).











































































    // ---- Positional parameter lexer tests (v22 Task 4) ----------------------


































    // ---- v27 here-string lexer tests -------------------------------------------



















    // ──────────────────────────────────────────────────────────────
    // >| clobber redirect tests (v123)
    // ──────────────────────────────────────────────────────────────





    // ──────────────────────────────────────────────────────────────
    // [[ ]] keyword recognition tests (v30)
    // ──────────────────────────────────────────────────────────────


























































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

    // ── v265 T6: focused, explicit-expected token-stream tests ───────────────
    // Exact atom `Vec`s for self-contained command-mode inputs (the mode-pushing
    // constructs `"…"`/`${…}`/`$(…)`/`$((…))`/backtick are covered by the parser
    // AST tests instead — driving the lexer alone past their opener signals does
    // not terminate, since the PARSER owns those bodies).

    #[test]
    fn t6_atoms_simple_command() {
        assert_eq!(command_atoms_of("echo hi"), vec![
            TokenKind::Lit { text: "echo".into(), quoted: false },
            TokenKind::Blank,
            TokenKind::Lit { text: "hi".into(), quoted: false },
        ]);
    }

    #[test]
    fn t6_atoms_single_and_ansic_quote() {
        assert_eq!(command_atoms_of("'a b'"), vec![
            TokenKind::QuoteRun { style: QuoteStyle::Single, text: "a b".into() },
        ]);
        assert_eq!(command_atoms_of("$'a\\tb'"), vec![
            TokenKind::QuoteRun { style: QuoteStyle::AnsiC, text: "a\tb".into() },
        ]);
    }

    #[test]
    fn t6_atoms_dollar_name_and_param_open_signal() {
        assert_eq!(command_atoms_of("echo $x"), vec![
            TokenKind::Lit { text: "echo".into(), quoted: false },
            TokenKind::Blank,
            TokenKind::DollarName { name: "x".into(), quoted: false },
        ]);
        // `${x}` ends at the zero-width ParamOpen SIGNAL — the parser owns the body.
        assert!(matches!(
            command_atoms_of("echo ${x}").last(),
            Some(TokenKind::ParamOpen { quoted: false })
        ));
    }

    #[test]
    fn t6_atoms_brace_stays_literal() {
        // Brace expansion is a parser/expansion concern; the lexer keeps `{a,b}`
        // as one literal word.
        assert_eq!(command_atoms_of("echo {a,b}"), vec![
            TokenKind::Lit { text: "echo".into(), quoted: false },
            TokenKind::Blank,
            TokenKind::Lit { text: "{a,b}".into(), quoted: false },
        ]);
    }

    #[test]
    fn t6_atoms_redirect_ops() {
        assert_eq!(command_atoms_of("cat < f"), vec![
            TokenKind::Lit { text: "cat".into(), quoted: false }, TokenKind::Blank,
            TokenKind::Op(Operator::RedirIn), TokenKind::Blank,
            TokenKind::Lit { text: "f".into(), quoted: false },
        ]);
        assert_eq!(command_atoms_of("echo >> f"), vec![
            TokenKind::Lit { text: "echo".into(), quoted: false }, TokenKind::Blank,
            TokenKind::Op(Operator::RedirAppend), TokenKind::Blank,
            TokenKind::Lit { text: "f".into(), quoted: false },
        ]);
        // here-string operator
        assert!(command_atoms_of("cat <<< word").iter()
            .any(|t| matches!(t, TokenKind::Op(Operator::HereString))));
        // `2>&1`: fd-prefix (RedirFd) then dup-out operator.
        let d = command_atoms_of("echo 2>&1");
        assert!(d.iter().any(|t| matches!(t, TokenKind::RedirFd(crate::command::RedirFd::Number(2)))), "fd prefix: {d:?}");
        assert!(d.iter().any(|t| matches!(t, TokenKind::Op(Operator::DupOut))), "dup-out: {d:?}");
    }

    #[test]
    fn t6_atoms_array_literal_and_subscript_assign() {
        // `a=(1 2)` → `a=` literal, ArrayOpen signal, `(`, elements, `)`.
        assert_eq!(command_atoms_of("a=(1 2)"), vec![
            TokenKind::Lit { text: "a=".into(), quoted: false },
            TokenKind::ArrayOpen,
            TokenKind::Op(Operator::LParen),
            TokenKind::Lit { text: "1".into(), quoted: false },
            TokenKind::Blank,
            TokenKind::Lit { text: "2".into(), quoted: false },
            TokenKind::Op(Operator::RParen),
        ]);
        // `a[0]=v` → (v268) the lexer no longer assembles the subscript itself:
        // it emits `Lit "a"` + a zero-width `LBracket` and defers to the PARSER
        // (which pushes `Mode::ParamSubscriptOperand`, assembles the subscript,
        // and — seeing the confirmed `AssignEq` after the parser-driven `]` —
        // builds the `AssignPrefix { Indexed, .. } ` atom itself; see
        // `parser::tests::cmdword_indexed_assignment_builds_indexed_target`).
        // Driving the RAW lexer alone (no parser mode-push) past `LBracket`
        // just continues plain Command-mode literal scanning, so `0]=v` comes
        // back as one literal run — that is the new, correct lexer-only
        // contract, not a regression.
        assert_eq!(command_atoms_of("a[0]=v"), vec![
            TokenKind::Lit { text: "a".into(), quoted: false },
            TokenKind::LBracket,
            TokenKind::Lit { text: "0]=v".into(), quoted: false },
        ]);
    }

    #[test]
    fn t6_atoms_extglob_open_signal() {
        // `@(a|b)` with extglob on → a zero-width ExtglobOpen signal (the parser
        // assembles the group); the `(` is NOT consumed.
        let mut lx = Lexer::new_live_atoms(
            "@(a|b)", &Default::default(),
            LexerOptions { extglob: true, ..Default::default() },
        );
        let first = lx.next_token().unwrap().unwrap();
        assert!(matches!(first.kind, TokenKind::ExtglobOpen { prefix: '@' }), "{:?}", first.kind);
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









    // --- process substitution lexer tests ---







    // --- bad-substitution lexer tests (v233) ---
















    #[test]
    fn pull_surfaces_lex_error_as_err() {
        // A genuinely unterminated construct surfaces as a Lex error when the
        // PARSER drives the lexer. Driving through `parse_sequence` (not a raw
        // `next()` drain) is the only valid discipline: the atom lexer emits a
        // zero-width `BeginDquote` that only the parser consumes (pushing
        // `Mode::DoubleQuote`, whose scan then reaches EOF and errors). A raw
        // drain past the `"` would re-emit the opener forever — that is not how
        // the lexer is consumed in production. (See bad_alias_body_surfaces_on_drain.)
        let empty = std::collections::HashMap::new();
        let mut lx = Lexer::new_live_atoms("echo \"unterminated", &empty, LexerOptions::default());
        let r = crate::parser::parse_sequence(&mut lx);
        assert!(matches!(r, Err(crate::command::ParseError::Lex(_))), "got {r:?}");
    }

    // --- Task 4: alias storage + command-position expansion ---

    // v266: alias expansion is now an INPUT-SOURCE stack push (the body is
    // re-lexed inline by the live cursor), not a history splice of pre-tokenized
    // Word tokens. The old `lx_with_alias` replay helper (`from_tokens`) never
    // scans, so it cannot drive expansion; these tests use the live atom path
    // (`new_live_atoms`) — the production front-end.

    /// Build a LIVE atom lexer with `pairs` registered as aliases.
    fn lx_atoms_with_alias(input: &'static str, pairs: &[(&str, &str)]) -> Lexer<'static> {
        let mut m = std::collections::HashMap::new();
        for (k, v) in pairs { m.insert(k.to_string(), v.to_string()); }
        Lexer::new_live_atoms(input, &m, LexerOptions::default())
    }

    /// Literal text of a command-word atom (`Lit`/`QuoteRun`), else `None`.
    fn atom_word_text(k: &TokenKind) -> Option<String> {
        match k {
            TokenKind::Lit { text, .. } => Some(text.clone()),
            TokenKind::QuoteRun { text, .. } => Some(text.clone()),
            _ => None,
        }
    }

    /// Drive `lx` the way the parser does at command position: expand at the
    /// start, re-expand at a fresh command position (after `;`, skipping blanks)
    /// and after a trailing-blank-eligible boundary. Returns the word texts.
    fn drive_alias_words(lx: &mut Lexer) -> Vec<String> {
        let mut out = Vec::new();
        lx.expand_command_alias().unwrap();
        while let Some(k) = lx.peek_kind().unwrap().cloned() {
            match k {
                TokenKind::Blank | TokenKind::Newline => {
                    lx.next().unwrap();
                    if lx.take_trailing_eligible() {
                        lx.expand_command_alias().unwrap();
                    }
                }
                TokenKind::Op(Operator::Semi) => {
                    lx.next().unwrap();
                    while matches!(lx.peek_kind().unwrap(), Some(TokenKind::Blank)) {
                        lx.next().unwrap();
                    }
                    lx.expand_command_alias().unwrap();
                }
                _ => {
                    if let Some(t) = atom_word_text(&k) { out.push(t); }
                    lx.next().unwrap();
                }
            }
        }
        out
    }

    #[test]
    fn alias_expands_at_command_position() {
        let mut lx = lx_atoms_with_alias("ll /tmp", &[("ll", "ls -l")]);
        assert_eq!(drive_alias_words(&mut lx), vec!["ls", "-l", "/tmp"]);
    }

    #[test]
    fn alias_not_expanded_at_argument_position() {
        let mut lx = lx_atoms_with_alias("echo ll", &[("ll", "ls -l")]);
        assert_eq!(drive_alias_words(&mut lx), vec!["echo", "ll"]);
    }

    #[test]
    fn alias_recursion_guard_terminates() {
        // The body's own leading `ls` is in the active frame → not re-expanded.
        let mut lx = lx_atoms_with_alias("ls", &[("ls", "ls -a")]);
        assert_eq!(drive_alias_words(&mut lx), vec!["ls", "-a"]);
    }

    #[test]
    fn alias_semicolon_reuse_terminates() {
        // v266: `name` stays active for the WHOLE body frame, so a `;`-separated
        // re-use of `name` inside its own body is guarded too. (The old
        // synchronous guard released `name` after the leading word and looped
        // forever here — see the report's RED evidence.)
        let mut lx = lx_atoms_with_alias("r", &[("r", "echo hi; r")]);
        assert_eq!(drive_alias_words(&mut lx), vec!["echo", "hi", "r"]);
    }

    #[test]
    fn alias_mutual_chain_terminates() {
        // a→"b x"; b (active a) →"a y"; the leading `a` of b's body is in the
        // active frame → not re-expanded. Result: command `a`, args `y` then `x`.
        let mut lx = lx_atoms_with_alias("a", &[("a", "b x"), ("b", "a y")]);
        assert_eq!(drive_alias_words(&mut lx), vec!["a", "y", "x"]);
    }

    #[test]
    fn alias_trailing_blank_makes_next_word_eligible() {
        let mut lx = lx_atoms_with_alias("a c", &[("a", "b "), ("c", "d")]);
        assert_eq!(drive_alias_words(&mut lx), vec!["b", "d"]);
    }

    #[test]
    fn alias_body_tokens_pin_to_name_span() {
        // Every atom produced from the body carries the alias-NAME invocation
        // span (base offset of `ll`), not a body-relative offset — exactly as the
        // old history-splice set `name_span` on each spliced token. `ll` is at
        // offset 2 (after two leading blanks the parser skips before expanding).
        let mut lx = lx_atoms_with_alias("  ll", &[("ll", "ls -l")]);
        while matches!(lx.peek_kind().unwrap(), Some(TokenKind::Blank)) {
            lx.next().unwrap();
        }
        lx.expand_command_alias().unwrap();
        let mut saw_body_word = false;
        while let Some(t) = lx.next().unwrap() {
            if let Some(txt) = atom_word_text(&t.kind) {
                saw_body_word = true;
                assert_eq!(t.span.offset, 2, "body atom {txt:?} must pin to the name span (offset 2)");
                assert_eq!(t.span.line, 1, "body atom {txt:?} must pin to the name line");
            }
        }
        assert!(saw_body_word, "expected the body to produce word atoms");
    }

    #[test]
    fn quoted_word_not_expanded() {
        let mut lx = lx_atoms_with_alias("'ll'", &[("ll", "ls")]);
        assert_eq!(drive_alias_words(&mut lx), vec!["ll"]);
    }

    #[test]
    fn bad_alias_body_surfaces_on_drain() {
        // The body is no longer tokenized up front, so `expand_command_alias`
        // returns Ok; the unterminated quote in the injected body surfaces when
        // the PARSER drives the lexer. Driving through `parse_sequence` (not a
        // raw `next()` drain) is the ONLY valid discipline: the atom lexer emits
        // zero-width word-part opener signals (`BeginDquote` here) that only the
        // parser consumes — pushing `Mode::DoubleQuote`, whose scan then reaches
        // EOF and errors. A raw drain past a `"` never advances the mode, so it
        // would re-emit the opener forever (alias or not); that is not how the
        // lexer is ever consumed in production.
        let mut m = std::collections::HashMap::new();
        m.insert("x".to_string(), "echo \"".to_string()); // unterminated quote in body
        let mut lx = Lexer::new_live_atoms("x", &m, LexerOptions::default());
        let r = crate::parser::parse_sequence(&mut lx);
        assert!(matches!(r, Err(crate::command::ParseError::Lex(_))), "got {r:?}");
    }

    // --- Task 7: incrementality + live-lexer error tests ---



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

    // v266: `rewind_restores_scalar_flags` removed — it raw-drained `[[ … ]]` to
    // observe `dbracket_depth`, but on the atom path that depth is parser-driven
    // (the lexer emits `[[` as a plain `Op`), so a raw `next_token` drain never
    // moves it. Scalar-flag rewind is still guaranteed by the `Mark` snapshot and
    // exercised by `rewind_restores_mode_stack` / `rewind_reproduces_tokens_same_mode`.

    #[test]
    fn scan_stall_guard_stops_zero_width_opener_runaway() {
        // A bare `$((` at command position with NO parser to consume the ArithOpen
        // signal: scan_step re-emits it without advancing the cursor. The guard must
        // surface Err(NoProgress) within a bounded number of pulls, not loop/OOM.
        let empty = std::collections::HashMap::new();
        let mut lx = Lexer::new_live_atoms("$((", &empty, LexerOptions::default());
        let mut err = None;
        for _ in 0..(SCAN_STALL_CAP as usize + 100) {
            match lx.next() {
                Ok(Some(_)) => continue,
                Ok(None) => break,
                Err(e) => { err = Some(e); break; }
            }
        }
        assert!(matches!(err, Some(LexError::NoProgress)), "expected NoProgress, got {err:?}");
    }

    #[test]
    fn scan_stall_guard_allows_normal_openers() {
        for src in ["$((1+2))", "${x}", "$(echo hi)", "`echo hi`", "$(( $(( 1 + 1 )) + 1 ))"] {
            let empty = std::collections::HashMap::new();
            let mut lx = Lexer::new_live_atoms(src, &empty, LexerOptions::default());
            assert!(crate::parser::parse_sequence(&mut lx).is_ok(), "false NoProgress on {src:?}");
        }
    }

    #[test]
    fn scan_stall_guard_counts_injected_alias_body() {
        // An alias whose body is far longer than SCAN_STALL_CAP tokens. Each injected
        // char advances `consumed`, so the guard never fires — proving the metric is
        // injected-aware (a raw main-offset metric would false-stall here).
        let body = "a ".repeat(SCAN_STALL_CAP as usize + 500);
        let mut aliases = std::collections::HashMap::new();
        aliases.insert("x".to_string(), format!("echo {body}"));
        let mut lx = Lexer::new_live_atoms("x", &aliases, LexerOptions::default());
        assert!(crate::parser::parse_sequence(&mut lx).is_ok());
    }

    #[test]
    fn maybe_prune_history_drops_consumed_prefix() {
        let empty = std::collections::HashMap::new();
        let src = "a ".repeat(HISTORY_PRUNE_THRESHOLD + 50); // many simple words + blanks
        let mut lx = Lexer::new_live_atoms(&src, &empty, LexerOptions::default());
        for _ in 0..(HISTORY_PRUNE_THRESHOLD + 1) { let _ = lx.next().unwrap(); }
        assert!(lx.pos >= HISTORY_PRUNE_THRESHOLD);
        let frontier = lx.peek_kind().unwrap().cloned(); // fill + capture next token
        lx.maybe_prune_history();
        assert_eq!(lx.pos, 0, "pos reset to 0");
        assert!(lx.scanned_token_count() <= 8, "consumed prefix drained");
        assert_eq!(lx.peek_kind().unwrap().cloned(), frontier, "frontier token preserved");
    }

    #[test]
    fn maybe_prune_history_noop_below_threshold() {
        let empty = std::collections::HashMap::new();
        let mut lx = Lexer::new_live_atoms("echo a b c", &empty, LexerOptions::default());
        let _ = lx.next().unwrap();
        let _ = lx.next().unwrap();
        let pos = lx.pos;
        let len = lx.scanned_token_count();
        lx.maybe_prune_history(); // pos < threshold → no-op
        assert_eq!(lx.pos, pos);
        assert_eq!(lx.scanned_token_count(), len);
    }
}

