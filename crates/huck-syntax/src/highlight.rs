//! What a highlighter needs to know about a line — produced BY the lexer
//! (v363, #666).
//!
//! Roles are SEMANTIC, never colours: `huck-syntax` must not learn about SGR.
//! The mapping to escape sequences lives in the CLI crate, so the palette is
//! one table in one place.
//!
//! # Why the lexer produces this rather than a highlighter deriving it
//!
//! Every question here is one the lexer has already answered while scanning,
//! and re-answering it downstream would mean a second copy of a rule that can
//! drift (#641). Three concrete cases decided the design:
//!
//!   * **command position** — nothing in the token stream marks "this is the
//!     command word", but v361's `CommandPos` state machine tracks it. The
//!     recorder reads it in place rather than reconstructing it;
//!   * **globs** — an unquoted literal run is coalesced into ONE `Lit{text}`
//!     token, so `*.rs` arrives as a single token and the `*` is invisible. A
//!     highlighter re-scanning for metacharacters would also have to re-derive
//!     which of them are quoted;
//!   * **escapes** — the double-quote scanner CONSUMES the backslash of `\$`,
//!     so by the time anything downstream sees the text it is gone.
//!
//! # Recording is off by default
//!
//! `LexerOptions::record_highlight` gates every push. With it false the lexer
//! must behave bit-identically, which the 3103-script parse sweep is the gate
//! for.

/// What a source region IS.
///
/// Not a colour and not a token kind — a token kind is what the lexer saw, a
/// role is what it MEANS for display. Several kinds map to one role (every
/// expansion opener is `Expansion`), and one kind can map to different roles by
/// context (a `Lit` is a `CommandWord`, a `Keyword`, or nothing at all).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// A bare unquoted word whose role the parser has not claimed — an
    /// argument, a `case` pattern, a `for` variable. Painted plain.
    ///
    /// It is recorded anyway, and that is the point: the extent of a word is the
    /// scanner's to know (escapes make it wider than its text), while what the
    /// word MEANS is the parser's. The scanner lays down the extent, the parser
    /// upgrades the role at consume time (`classify_consumed_word`). Without a
    /// placeholder there would be nothing to upgrade, and re-deriving the extent
    /// in a second place is how the two copies drift.
    Word,
    /// The command word of a simple command — the thing whose existence is
    /// checked. Recorded at command position INCLUDING inside a substitution
    /// body, so the `nosuch` in `echo $(nosuch)` is marked in its own right.
    CommandWord,
    /// A reserved word (`if`, `for`, `do`, …) at command position.
    Keyword,
    /// A `'…'` run. Distinct from `QuotedDouble` because the two are visually
    /// different things to a reader: one is inert, the other still expands.
    QuotedSingle,
    /// A `"…"` run.
    QuotedDouble,
    /// An expansion as a whole — `$(`, `${`, `$((`, `$[`, backtick, `<(`/`>(`.
    Expansion,
    /// The NAME inside an expansion (`$FOO`, `${FOO:-x}`), which is what the
    /// eye actually looks for; rendered bold.
    VarName,
    /// A control operator: `|`, `&&`, `||`, `;`, `&`.
    Operator,
    /// A redirection's fd or operator.
    Redirect,
    /// A `#` comment, to end of line.
    Comment,
    /// A glob metacharacter run in an UNQUOTED literal (`*`, `?`, `[a-z]`).
    Glob,
    /// A backslash escape the scanner consumed (`\$`, `\"` inside `"…"`).
    Escape,
    /// A `~` that will be tilde-expanded (not a literal one).
    Tilde,
}

/// One recorded region; `end` is EXCLUSIVE.
///
/// The lexer sets both ends. `Span` carries only a start, so deriving extents
/// downstream from consecutive token starts would make the highlighter a second
/// source of truth about where a token ends — and would be wrong for the last
/// token on the line, and for the zero-width opener signals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mark {
    pub start: usize,
    pub end: usize,
    pub role: Role,
}

/// A matched pair, recorded when its frame pops.
///
/// Nearly free: v361 put `open_off` on every `ModeFrame` and a frame pops at
/// its closer, so both ends are in hand at the pop site with nothing re-scanned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PairSpan {
    /// Offset of the opening character (`(` of `$(`, the `"`, …).
    pub open: usize,
    /// Offset of the closing character.
    pub close: usize,
}

/// Everything one highlight parse produced.
///
/// Two lists of DIFFERENT things — marks are regions, pairs are relations —
/// with no index correspondence between them, so this is not the parallel
/// structure #641 forbids. Appending to one never obliges you to append to the
/// other.
#[derive(Debug, Default, Clone)]
pub struct HighlightRecord {
    pub marks: Vec<Mark>,
    pub pairs: Vec<PairSpan>,
    /// Offset of the still-open pair at end of input, if any — the dangling
    /// opener. Filled from the same walk v362 built for EOF diagnostics
    /// (`pairs::reported_pair`), so the shell and the highlighter agree about
    /// which bracket is unfinished. Wired in Task 6.
    pub unterminated: Option<usize>,
}
