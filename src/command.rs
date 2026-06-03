use crate::lexer::{Operator, Token, Word, WordPart};

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Keyword {
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
    DoubleBracketOpen,   // [[
    DoubleBracketClose,  // ]]
    Function,
}

impl Keyword {
    fn name(self) -> &'static str {
        match self {
            Keyword::If => "if",
            Keyword::Then => "then",
            Keyword::Elif => "elif",
            Keyword::Else => "else",
            Keyword::Fi => "fi",
            Keyword::While => "while",
            Keyword::Until => "until",
            Keyword::Do => "do",
            Keyword::Done => "done",
            Keyword::For => "for",
            Keyword::In => "in",
            Keyword::Case => "case",
            Keyword::Esac => "esac",
            Keyword::LBrace => "{",
            Keyword::RBrace => "}",
            Keyword::DoubleBracketOpen => "[[",
            Keyword::DoubleBracketClose => "]]",
            Keyword::Function => "function",
        }
    }
}

/// Returns the keyword a token represents, or `None`. A token is a
/// keyword only when it is a `Word` of exactly one part — an *unquoted*
/// `Literal` whose text equals the keyword.
fn keyword_of(token: &Token) -> Option<Keyword> {
    let Token::Word(Word(parts)) = token else { return None };
    if parts.len() != 1 {
        return None;
    }
    let WordPart::Literal { text, quoted: false } = &parts[0] else {
        return None;
    };
    match text.as_str() {
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
        _ => None,
    }
}

/// If `word` is an assignment-shaped Word (either an `AssignPrefix`-led
/// structured form for `name+=…` / `name[…]=…` / `name[…]+=…`, or a
/// leading `Literal` of the form `NAME=…` for bare scalar `name=…`),
/// returns the structured `Assignment`. The value is moved (not cloned)
/// from the input parts. Otherwise returns `Err(word)` handing the
/// original back unchanged.
pub(crate) fn try_split_assignment(
    word: crate::lexer::Word,
) -> Result<Assignment, crate::lexer::Word> {
    use crate::lexer::WordPart;
    // 1. AssignPrefix-led: the lexer has already parsed the target.
    if matches!(word.0.first(), Some(WordPart::AssignPrefix { .. })) {
        let crate::lexer::Word(mut parts) = word;
        let prefix = parts.remove(0);
        let WordPart::AssignPrefix { target, append } = prefix else {
            unreachable!("checked above");
        };
        return Ok(Assignment {
            target,
            value: crate::lexer::Word(parts),
            append,
        });
    }
    // 2. Bare `name=value` shape: leading Literal whose text begins
    //    with an identifier followed by `=`.
    let first = match word.0.first() {
        Some(p) => p,
        None => return Err(word),
    };
    let text = match first {
        WordPart::Literal { text, .. } => text,
        _ => return Err(word),
    };
    let Some(eq) = text.find('=') else {
        return Err(word);
    };
    let name_slice = &text[..eq];
    if name_slice.is_empty() {
        return Err(word);
    }
    let mut name_chars = name_slice.chars();
    let Some(first_ch) = name_chars.next() else {
        return Err(word);
    };
    if !(first_ch == '_' || first_ch.is_ascii_alphabetic()) {
        return Err(word);
    }
    if !name_chars.all(|c| c == '_' || c.is_ascii_alphanumeric()) {
        return Err(word);
    }

    // Validation passed — destructure the word, moving parts into the value.
    let crate::lexer::Word(mut parts) = word;
    let first_part = parts.remove(0);
    let text = match first_part {
        WordPart::Literal { text, quoted } => {
            debug_assert!(
                !quoted,
                "assignment-eligible first Literal must be unquoted; lexer's `=` arm only fires while accumulating unquoted text"
            );
            text
        }
        _ => unreachable!("checked above"),
    };
    let (name, rest_of_first) = (text[..eq].to_string(), text[eq + 1..].to_string());
    let mut value_parts: Vec<WordPart> = Vec::with_capacity(parts.len() + 1);
    value_parts.push(WordPart::Literal { text: rest_of_first, quoted: false });
    value_parts.extend(parts);
    Ok(Assignment {
        target: AssignTarget::Bare(name),
        value: crate::lexer::Word(value_parts),
        append: false,
    })
}

/// Returns `true` if `w` looks like an assignment word without
/// consuming or cloning it. Mirrors the shape check in `try_split_assignment`
/// so the caller can decide whether to take ownership before calling the
/// real splitter. Detects both the structured `AssignPrefix` form and the
/// legacy bare `Literal("NAME=…")` form.
pub(crate) fn is_assignment_word(w: &crate::lexer::Word) -> bool {
    use crate::lexer::WordPart;
    if matches!(w.0.first(), Some(WordPart::AssignPrefix { .. })) {
        return true;
    }
    let text = match w.0.first() {
        Some(WordPart::Literal { text, quoted: false }) => text,
        _ => return false,
    };
    let Some(eq) = text.find('=') else { return false };
    let name_slice = &text[..eq];
    if name_slice.is_empty() {
        return false;
    }
    let mut chars = name_slice.chars();
    let first_ch = chars.next().expect("non-empty");
    (first_ch == '_' || first_ch.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// Constructs a single-part unquoted literal `Word` from a static string.
/// Used by the parser to synthesize the "1" source-word in `&>` / `&>>` desugaring.
fn lit_word(s: &str) -> Word {
    Word(vec![WordPart::Literal { text: s.to_string(), quoted: false }])
}

fn finalize_stage(
    program: crate::lexer::Word,
    args: Vec<crate::lexer::Word>,
    stdin: Option<Redirect>,
    stdout: Option<Redirect>,
    stderr: Option<Redirect>,
) -> SimpleCommand {
    let no_redirs = stdin.is_none() && stdout.is_none() && stderr.is_none();

    // Walk [program, args…] peeling leading assignments. Stops at the first
    // word that isn't a valid `NAME=value` (per try_split_assignment).
    // We peek by reference first (cheap) and only take ownership when the
    // word is confirmed to be assignment-shaped, avoiding a deep clone on
    // every non-assignment first word.
    let mut inline: Vec<Assignment> = Vec::new();
    let mut iter = std::iter::once(program).chain(args).peekable();
    while let Some(w) = iter.peek() {
        if !is_assignment_word(w) {
            break;
        }
        let owned = iter.next().expect("just peeked Some");
        match try_split_assignment(owned) {
            Ok(a) => inline.push(a),
            Err(_) => unreachable!("is_assignment_word confirmed assignment shape"),
        }
    }
    let remaining: Vec<Word> = iter.collect();

    if remaining.is_empty() && no_redirs && !inline.is_empty() {
        return SimpleCommand::Assign(inline);
    }
    // No trailing program word, but redirects (or zero words at all). Produce
    // an Exec with an empty program word; the executor treats this as a
    // "redirects only" command (POSIX 2.10.2 permits this — opens the files
    // for side effects, then exits 0).
    if remaining.is_empty() {
        return SimpleCommand::Exec(ExecCommand {
            inline_assignments: inline,
            program: Word(Vec::new()),
            args: Vec::new(),
            stdin,
            stdout,
            stderr,
        });
    }
    let mut remaining = remaining.into_iter();
    let program = remaining.next().expect("non-empty after peel");
    let args: Vec<Word> = remaining.collect();
    SimpleCommand::Exec(ExecCommand {
        inline_assignments: inline,
        program,
        args,
        stdin,
        stdout,
        stderr,
    })
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Redirect {
    /// `<file` — open file for reading on stdin.
    Read(Word),
    /// `>file` — open file for writing (truncate first).
    Truncate(Word),
    /// `>>file` — open file for writing (append).
    Append(Word),
    /// `<<DELIM` (and friends) — heredoc body.
    /// `expand` is false for `<<'DELIM'` (any quoted part of the delim
    /// word triggers literal mode). `strip_tabs` is true for `<<-`.
    /// The body has tabs already stripped at lex time for `<<-`.
    Heredoc { body: Word, expand: bool, strip_tabs: bool },
    /// `<<<word` — here-string: the body is a single Word to be expanded
    /// (no split/glob) with a trailing newline appended.
    HereString(Word),
    /// `>&N` / `2>&N` — duplicate an fd. `fd` is the target fd (1 for stdout,
    /// 2 for stderr); `source` is the Word to expand to get the source fd number.
    Dup { fd: i32, source: Word },
}

/// Left-hand side of an assignment. Bare `name=v` is `Bare`;
/// subscripted `name[expr]=v` is `Indexed`.
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum AssignTarget {
    Bare(String),
    Indexed { name: String, subscript: Word },
}

impl AssignTarget {
    /// Returns the underlying variable name regardless of whether the
    /// target is bare or subscripted.
    #[allow(dead_code)] // used by tests + by Tasks 3/4 array execution
    pub fn name(&self) -> &str {
        match self {
            AssignTarget::Bare(n) => n,
            AssignTarget::Indexed { name, .. } => name,
        }
    }
}

/// One assignment record: `name=value`, `name=(…)`, `name[i]=value`,
/// `name+=value`, `name+=(…)`, or `name[i]+=value`.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Assignment {
    pub target: AssignTarget,
    pub value: Word,
    pub append: bool,
}

/// Argument shape for declaration commands (`declare`, `typeset`, `local`,
/// `readonly`, `export`). Each surface argument is either a plain string
/// (flags like `-a`, bare names, or post-expansion `name=value`) or an
/// `Assignment` parsed from a compound-RHS word like `name=(x y z)`. The
/// executor populates the variant in `resolve()` so the builtins can route
/// compound-RHS assignments through the same Task-4 path used by ordinary
/// assignment commands. Non-declaration commands never see `DeclArg`.
#[derive(Debug, Clone)]
pub enum DeclArg {
    /// A post-expansion string — flag, bare name, or scalar `name=value`.
    Plain(String),
    /// A compound-RHS assignment (e.g. `name=(x y z)` or `name[i]+=v`).
    Assign(Assignment),
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ExecCommand {
    /// Leading assignments preceding the command word (`A=1 B=2 cmd`).
    /// Empty when the user wrote `cmd args` with no assignment prefix.
    pub inline_assignments: Vec<Assignment>,
    pub program: Word,
    pub args: Vec<Word>,
    // BREAKING CHANGE (v24): was Option<Word>; now Option<Redirect> so
    // `<file` (Read), `<<EOF` (Heredoc), and (future) `<<<` share a
    // uniform shape. Last-wins: a later redirect to stdin overwrites
    // an earlier one.
    pub stdin: Option<Redirect>,
    pub stdout: Option<Redirect>,
    pub stderr: Option<Redirect>,
}

impl ExecCommand {
    /// Returns the program name as a plain `String` if the program Word
    /// consists of exactly one unquoted `Literal` part — the common case
    /// for statically-written commands like `cat` or `grep`. Returns `None`
    /// for dynamic program words such as `$cmd` or `"name"` (quoted).
    ///
    /// Used by `classify_stage` in the executor for best-effort static
    /// resolution: if this returns `None`, the stage falls back to the
    /// InProcess (fork-subshell) path, which is always correct.
    pub fn program_static_text(&self) -> Option<String> {
        if self.program.0.len() == 1
            && let WordPart::Literal { text, quoted: false } = &self.program.0[0]
        {
            return Some(text.clone());
        }
        None
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum SimpleCommand {
    /// `A=1 B=2 …` with no following command — every assignment
    /// persists in the shell. Single-element vec is the v22-style
    /// single-assignment case.
    Assign(Vec<Assignment>),
    Exec(ExecCommand),
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Pipeline {
    // BREAKING CHANGE (v25): was Vec<SimpleCommand>; now Vec<Command>.
    // The parser rejects Command::Pipeline as a stage (nested multi-stage
    // pipelines aren't a POSIX construct at this level).
    // Task 1: parser still only emits Command::Simple stages.
    // Task 2: parser will emit compound stages too.
    pub commands: Vec<Command>,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Connector {
    Semi,
    And,
    Or,
}

// ──────────────────────────────────────────────────────────────
// [[ ]] extended test — AST types (v30)
// ──────────────────────────────────────────────────────────────

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum TestUnaryOp {
    FileExists,      // -e
    IsRegFile,       // -f
    IsDir,           // -d
    IsReadable,      // -r
    IsWritable,      // -w
    IsExecutable,    // -x
    IsNonEmpty,      // -s (non-empty file)
    IsSymlink,       // -L
    StringNonEmpty,  // -n
    StringEmpty,     // -z
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum TestBinaryOp {
    StringEq,  // == or = (bash alias)
    StringNe,  // !=
    StringLt,  // <  (lexicographic)
    StringGt,  // >  (lexicographic)
    IntEq,     // -eq
    IntNe,     // -ne
    IntLt,     // -lt
    IntGt,     // -gt
    IntLe,     // -le
    IntGe,     // -ge
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum TestExpr {
    Unary  { op: TestUnaryOp,  operand: Word },
    Binary { op: TestBinaryOp, lhs: Word, rhs: Word },
    Regex  { lhs: Word, pattern: Word },
    Not(Box<TestExpr>),
    And(Box<TestExpr>, Box<TestExpr>),
    Or(Box<TestExpr>,  Box<TestExpr>),
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Command {
    Pipeline(Pipeline),
    Simple(SimpleCommand), // NEW (v25 Shape A): pipeline stage wrapping a SimpleCommand
    If(Box<IfClause>),
    While(Box<WhileClause>),
    For(Box<ForClause>),
    Case(Box<CaseClause>),
    BraceGroup(Box<Sequence>),
    Subshell { body: Box<Sequence> }, // NEW (v28): `(list)` subshell
    FunctionDef { name: String, body: Box<Command> },
    /// NEW (v30): `[[ … ]]` extended test.
    /// `inline_assignments` holds any `NAME=value` prefixes (e.g. `FOO=hi [[ … ]]`).
    DoubleBracket {
        expr: Box<TestExpr>,
        inline_assignments: Vec<Assignment>,
    },
    /// NEW (v78): standalone `((expr))` command. Exit 0 if non-zero, 1 if zero.
    Arith(crate::arith::ArithExpr),
    /// NEW (v78): C-style `for ((init; cond; step)) do BODY done`.
    ArithFor(Box<ArithForClause>),
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct IfClause {
    pub condition: Sequence,
    pub then_body: Sequence,
    pub elif_branches: Vec<ElifBranch>,
    pub else_body: Option<Sequence>,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct WhileClause {
    pub condition: Sequence,
    pub body: Sequence,
    pub until: bool,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ForClause {
    /// The loop variable name — a validated identifier.
    pub var: String,
    /// The unexpanded `in` word list. Empty for the no-`in` form.
    pub words: Vec<Word>,
    /// The do…done body.
    pub body: Sequence,
}

/// NEW (v78): a C-style `for ((init; cond; step)) do BODY done` clause.
/// Each header section is optional; an empty cond is treated as always-true.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ArithForClause {
    pub init: Option<crate::arith::ArithExpr>,
    pub cond: Option<crate::arith::ArithExpr>,
    pub step: Option<crate::arith::ArithExpr>,
    pub body: Sequence,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct CaseClause {
    /// The word being matched — unexpanded.
    pub subject: Word,
    /// The clauses, in source order. May be empty.
    pub items: Vec<CaseItem>,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct CaseItem {
    /// The `|`-separated patterns, unexpanded. Always non-empty.
    pub patterns: Vec<Word>,
    /// The clause body. `None` means an empty body.
    pub body: Option<Sequence>,
    pub terminator: CaseTerminator,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum CaseTerminator {
    Break,         // ;;
    FallThrough,   // ;&
    ContinueMatch, // ;;&
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ElifBranch {
    pub condition: Sequence,
    pub body: Sequence,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Sequence {
    pub first: Command,
    pub rest: Vec<(Connector, Command)>,
    pub background: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    MissingCommand,
    MissingRedirectTarget,
    RedirectTargetIsOperator,
    UnexpectedBackground,
    UnterminatedIf,
    UnexpectedKeyword(String),
    UnterminatedLoop,
    UnexpectedToken,
    ForVariable,
    UnterminatedCase,
    UnterminatedBrace,
    FunctionName,
    FunctionBody,
    UnterminatedFunction,
    EmptySubshell,              // NEW (v28): `()` — empty subshell body
    UnterminatedSubshell,       // NEW (v28): `(cmd` with no closing `)`
    EmptyDoubleBracket,         // NEW (v30): `[[ ]]` — no expression
    UnterminatedDoubleBracket,  // NEW (v30): `[[ x == y` — missing `]]`
    TestExprBadOperator(String),// NEW (v30): unrecognised operator inside `[[ ]]`
    TestExprMissingOperand,     // NEW (v30): e.g. `[[ -f ]]` or `[[ x == ]]`
    /// NEW (v78): `crate::arith::parse` failed on the body of a `((...))`
    /// block or a for-loop header section. Carries the inner error message.
    ArithBlock(String),
    /// NEW (v78): `for ((header))` header did not split into exactly 3
    /// `;`-separated sections.
    ArithForHeader(String),
}

pub fn parse(tokens: Vec<Token>) -> Result<Option<Sequence>, ParseError> {
    let mut iter = tokens.into_iter().peekable();
    skip_newlines(&mut iter);
    if iter.peek().is_none() {
        return Ok(None);
    }
    let seq = parse_sequence(&mut iter, &[])?;
    if iter.peek().is_some() {
        // A stray terminator (`;;`/`;&`/`;;&`) left after the top-level
        // sequence — `parse_sequence` peek-breaks on those (see below).
        return Err(ParseError::UnexpectedToken);
    }
    Ok(Some(seq))
}

/// Parses commands joined by `;` / `&&` / `||` (and an optional trailing
/// `&` at top level only). Stops — without consuming — when the next
/// token is a keyword in `stop_at`. `stop_at` is empty only at the top
/// level; a non-empty `stop_at` means we are inside a compound command.
fn parse_sequence<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
    stop_at: &[Keyword],
) -> Result<Sequence, ParseError> {
    let at_top_level = stop_at.is_empty();
    let raw_first = parse_command(iter)?;
    // If the command is immediately followed by `|`, it is the first stage of
    // a pipeline — even if it is a compound command (if/while/for/case/brace).
    // `parse_command` dispatches compound commands without checking for a
    // trailing `|`, so we handle that here.
    let first = if matches!(iter.peek(), Some(Token::Op(Operator::Pipe))) {
        let mut stages = vec![raw_first];
        // Consume the `|` that was peeked above.
        iter.next();
        skip_newlines(iter);
        let mut more_stages = true;
        while more_stages {
            let (cmd, next_pipe) = parse_next_stage(iter)?;
            stages.push(cmd);
            if next_pipe {
                // Simple stage: it already consumed the `|` internally.
                // Continue to collect the next stage.
            } else {
                // Compound stage: did not consume a trailing `|`. Check now.
                if matches!(iter.peek(), Some(Token::Op(Operator::Pipe))) {
                    iter.next(); // consume `|`
                    skip_newlines(iter);
                } else {
                    more_stages = false;
                }
            }
        }
        Command::Pipeline(Pipeline { commands: stages })
    } else {
        raw_first
    };
    let mut rest = Vec::new();
    let mut background = false;

    loop {
        match iter.peek() {
            None => break,
            Some(Token::Op(
                Operator::DoubleSemi | Operator::SemiAmp | Operator::DoubleSemiAmp,
            )) => break,
            Some(tok) => {
                if let Some(kw) = keyword_of(tok)
                    && stop_at.contains(&kw)
                {
                    break;
                }
            }
        }
        let token = iter.next().unwrap();
        match token {
            Token::Op(Operator::Background) => {
                if !at_top_level {
                    return Err(ParseError::UnexpectedBackground);
                }
                // Skip any trailing Newline tokens that the heredoc body
                // collector emits at the end of the input (e.g. when the
                // buffer is "cat <<EOF &\nbody\nEOF" the lexer emits a
                // Newline after the heredoc body). Trailing Newlines after
                // `&` are not meaningful — `&` already terminates the
                // sequence.
                skip_newlines(iter);
                if iter.peek().is_some() {
                    return Err(ParseError::UnexpectedBackground);
                }
                background = true;
                break;
            }
            Token::Op(Operator::Semi) | Token::Newline => {
                skip_newlines(iter);
                match iter.peek() {
                    None => break,
                    Some(Token::Op(
                        Operator::DoubleSemi | Operator::SemiAmp | Operator::DoubleSemiAmp,
                    )) => break,
                    Some(tok) => {
                        if keyword_of(tok).map(|k| stop_at.contains(&k)).unwrap_or(false) {
                            break;
                        }
                    }
                }
                rest.push((Connector::Semi, parse_command(iter)?));
            }
            Token::Op(Operator::And) => {
                skip_newlines(iter);
                rest.push((Connector::And, parse_command(iter)?));
            }
            Token::Op(Operator::Or) => {
                skip_newlines(iter);
                rest.push((Connector::Or, parse_command(iter)?));
            }
            other => {
                if let Some(kw) = keyword_of(&other) {
                    return Err(ParseError::UnexpectedKeyword(kw.name().to_string()));
                }
                // A non-keyword, non-connector token after a command —
                // e.g. a stray word or `|` after a closed `if`/`while`.
                return Err(ParseError::UnexpectedToken);
            }
        }
    }

    Ok(Sequence { first, rest, background })
}

/// Parses a single sequence element: a subshell, compound command, or pipeline.
fn parse_command<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<Command, ParseError> {
    skip_newlines(iter);

    // Standalone arith block: `((expr))` at command position.
    if matches!(iter.peek(), Some(Token::ArithBlock(_))) {
        let Some(Token::ArithBlock(text)) = iter.next() else {
            unreachable!("matches! guard above guarantees ArithBlock")
        };
        let expr = crate::arith::parse(&text)
            .map_err(|e| ParseError::ArithBlock(e.to_string()))?;
        return Ok(Command::Arith(expr));
    }

    match iter.peek().and_then(keyword_of) {
        Some(Keyword::If) => Ok(Command::If(Box::new(parse_if(iter)?))),
        Some(Keyword::While) | Some(Keyword::Until) => {
            Ok(Command::While(Box::new(parse_while(iter)?)))
        }
        Some(Keyword::For) => parse_for_command(iter),
        Some(Keyword::Case) => Ok(Command::Case(Box::new(parse_case(iter)?))),
        Some(Keyword::LBrace) => Ok(Command::BraceGroup(Box::new(parse_brace_group(iter)?))),
        Some(Keyword::DoubleBracketOpen) => parse_double_bracket(iter),
        Some(Keyword::Function) => parse_function_keyword_def(iter),
        Some(other) => Err(ParseError::UnexpectedKeyword(other.name().to_string())),
        None => {
            // Check for bare `(` at command-start → subshell `(list)`.
            // This check MUST come BEFORE the IDENT+LParen function-def path,
            // but in practice they don't overlap: function-def starts with
            // a Word token, not an Op(LParen). Still, the comment clarifies
            // intent.
            if matches!(iter.peek(), Some(Token::Op(Operator::LParen))) {
                return parse_subshell(iter);
            }
            // Non-keyword, non-LParen: may be a function definition
            // `name() compound`, or a plain pipeline. Need two-token lookahead.
            if matches!(iter.peek(), Some(Token::Word(_))) {
                // Consume the word; peek for `(`.
                let Some(Token::Word(w)) = iter.next() else { unreachable!() };
                if matches!(iter.peek(), Some(Token::Op(Operator::LParen))) {
                    return parse_function_def(w, iter);
                }
                // Detect inline assignments before `[[`:
                // e.g. `FOO=hi [[ $FOO == hi ]]`.
                //
                // We speculatively peel assignment words. If `[[` follows,
                // dispatch to `parse_double_bracket_with_assigns`. Otherwise
                // fall through to normal pipeline parsing using the cloned
                // words — we must NOT drain `iter` in the fallback path.
                if is_assignment_word(&w) {
                    let w_clone = w.clone();
                    let mut assigns: Vec<Assignment> = Vec::new();
                    match try_split_assignment(w) {
                        Ok(a) => assigns.push(a),
                        Err(_) => unreachable!("is_assignment_word confirmed"),
                    }
                    let mut extra_clones: Vec<Word> = Vec::new();
                    // Peel further consecutive assignment words.
                    while let Some(Token::Word(nw)) = iter.peek() {
                        if !is_assignment_word(nw) {
                            break;
                        }
                        let Some(Token::Word(nw)) = iter.next() else { unreachable!() };
                        let nw_clone = nw.clone();
                        match try_split_assignment(nw) {
                            Ok(a) => assigns.push(a),
                            Err(_) => unreachable!("is_assignment_word confirmed"),
                        }
                        extra_clones.push(nw_clone);
                    }
                    // If `[[` follows, dispatch as DoubleBracket with assigns.
                    if iter.peek().and_then(keyword_of) == Some(Keyword::DoubleBracketOpen) {
                        return parse_double_bracket_with_assigns(iter, assigns);
                    }
                    // Fallback: no `[[`. Restore the pipeline starting from the
                    // first cloned assignment word.
                    //
                    // Single-assign case (most common — e.g. `A=1 cmd`):
                    // extra_clones is empty; pass w_clone directly to
                    // parse_pipeline_with_first with the original iter.
                    if extra_clones.is_empty() {
                        return Ok(Command::Pipeline(
                            parse_pipeline_with_first(Some(w_clone), vec![], iter)?
                        ));
                    }
                    // Multi-assign case (e.g. `A=1 B=2 cmd`): extra assignment
                    // words were consumed from `iter` during speculative peeling.
                    // Re-inject them as prefix_tokens so that `iter` is NOT
                    // drained — the outer parse_sequence needs `iter` intact to
                    // pick up any trailing `;`/`&&`/`||` separators after this
                    // pipeline ends.
                    let prefix: Vec<Token> =
                        extra_clones.into_iter().map(Token::Word).collect();
                    return Ok(Command::Pipeline(
                        parse_pipeline_with_first(Some(w_clone), prefix, iter)?
                    ));
                }
                // Not a function def — pipeline with `w` as the first word.
                Ok(Command::Pipeline(parse_pipeline_with_first(Some(w), vec![], iter)?))
            } else {
                Ok(Command::Pipeline(parse_pipeline(iter)?))
            }
        }
    }
}

/// True if `body` is one of the compound-command shapes that's
/// allowed as a function body in both POSIX `name() body` form and
/// the bash `function NAME body` form.
fn is_function_body_shape(body: &Command) -> bool {
    matches!(
        body,
        Command::If(_)
            | Command::While(_)
            | Command::For(_)
            | Command::Case(_)
            | Command::BraceGroup(_)
            | Command::Subshell { .. }
            | Command::DoubleBracket { .. }
            | Command::Arith(_)
            | Command::ArithFor(_)
    )
}

/// Parses `name() compound-command`. The caller has consumed the name
/// (`name_word`) and verified the next token is `(`.
fn parse_function_def<I: Iterator<Item = Token>>(
    name_word: Word,
    iter: &mut std::iter::Peekable<I>,
) -> Result<Command, ParseError> {
    let name = valid_identifier_text(&name_word).ok_or(ParseError::FunctionName)?;
    // Consume `(`.
    iter.next();
    // Expect `)`.
    match iter.next() {
        Some(Token::Op(Operator::RParen)) => {}
        _ => return Err(ParseError::FunctionBody),
    }
    skip_newlines(iter);
    if iter.peek().is_none() {
        return Err(ParseError::UnterminatedFunction);
    }
    let body = parse_command(iter)?;
    if !is_function_body_shape(&body) {
        return Err(ParseError::FunctionBody);
    }
    Ok(Command::FunctionDef { name, body: Box::new(body) })
}

/// Parses `function NAME [()] compound-command`. The caller has
/// verified the next token is the `function` keyword (still in the
/// iterator). Consumes the keyword, the name, optional `()`, and
/// the compound body.
fn parse_function_keyword_def<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<Command, ParseError> {
    // Consume `function` keyword.
    iter.next();

    // Read the name. Must be a Word that's a valid POSIX identifier
    // and not a reserved keyword.
    let name_word = match iter.next() {
        Some(Token::Word(w)) => w,
        _ => return Err(ParseError::FunctionName),
    };
    let name = valid_identifier_text(&name_word).ok_or(ParseError::FunctionName)?;

    // Optionally consume `()`.
    if matches!(iter.peek(), Some(Token::Op(Operator::LParen))) {
        iter.next(); // consume `(`
        match iter.next() {
            Some(Token::Op(Operator::RParen)) => {}
            _ => return Err(ParseError::FunctionBody),
        }
    }

    // Allow newlines between name (or `()`) and the body.
    skip_newlines(iter);
    if iter.peek().is_none() {
        return Err(ParseError::UnterminatedFunction);
    }

    let body = parse_command(iter)?;
    if !is_function_body_shape(&body) {
        return Err(ParseError::FunctionBody);
    }
    Ok(Command::FunctionDef { name, body: Box::new(body) })
}

/// Consumes a run of `Newline` tokens. Newlines are soft separators —
/// they are skipped wherever a command is expected but not yet present.
fn skip_newlines<I: Iterator<Item = Token>>(iter: &mut std::iter::Peekable<I>) {
    while matches!(iter.peek(), Some(Token::Newline)) {
        iter.next();
    }
}

/// Consumes one token and checks it is the expected keyword.
fn expect_keyword<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
    expected: Keyword,
    on_missing: ParseError,
) -> Result<(), ParseError> {
    match iter.next() {
        Some(ref t) if keyword_of(t) == Some(expected) => Ok(()),
        _ => Err(on_missing),
    }
}

/// Runs `parse_sequence` for a compound command's condition or body.
/// If it fails with `MissingCommand` because input simply ran out
/// (the iterator is exhausted), the failure is the compound command
/// being unterminated — report `unterminated` instead. A
/// `MissingCommand` with tokens still pending is a genuine error and
/// passes through unchanged.
///
/// Known edge case: a compound section consisting of a bare leading
/// `|` (e.g. `if |`) also yields `MissingCommand` with an exhausted
/// iterator and is mis-remapped to `unterminated`. This is harmless in
/// practice — the REPL's completeness classifier intercepts any buffer
/// ending in a bare `|`/`&&`/`||` before `parse` is reached, so this
/// path is unreachable through the shell.
fn parse_compound_section<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
    stop_at: &[Keyword],
    unterminated: ParseError,
) -> Result<Sequence, ParseError> {
    match parse_sequence(iter, stop_at) {
        Err(ParseError::MissingCommand) if iter.peek().is_none() => Err(unterminated),
        other => other,
    }
}

/// Parses `if LIST; then LIST; [elif LIST; then LIST;]... [else LIST;] fi`.
fn parse_if<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<IfClause, ParseError> {
    expect_keyword(iter, Keyword::If, ParseError::UnterminatedIf)?;
    let condition = parse_compound_section(iter, &[Keyword::Then], ParseError::UnterminatedIf)?;
    expect_keyword(iter, Keyword::Then, ParseError::UnterminatedIf)?;
    let then_body = parse_compound_section(
        iter,
        &[Keyword::Elif, Keyword::Else, Keyword::Fi],
        ParseError::UnterminatedIf,
    )?;

    let mut elif_branches = Vec::new();
    while iter.peek().and_then(keyword_of) == Some(Keyword::Elif) {
        iter.next(); // consume `elif`
        let condition = parse_compound_section(iter, &[Keyword::Then], ParseError::UnterminatedIf)?;
        expect_keyword(iter, Keyword::Then, ParseError::UnterminatedIf)?;
        let body = parse_compound_section(
            iter,
            &[Keyword::Elif, Keyword::Else, Keyword::Fi],
            ParseError::UnterminatedIf,
        )?;
        elif_branches.push(ElifBranch { condition, body });
    }

    let else_body = if iter.peek().and_then(keyword_of) == Some(Keyword::Else) {
        iter.next(); // consume `else`
        Some(parse_compound_section(iter, &[Keyword::Fi], ParseError::UnterminatedIf)?)
    } else {
        None
    };

    expect_keyword(iter, Keyword::Fi, ParseError::UnterminatedIf)?;
    Ok(IfClause { condition, then_body, elif_branches, else_body })
}

/// Returns the text of `word` if it is a single, unquoted `Literal` whose
/// text is a valid identifier (`[A-Za-z_][A-Za-z0-9_]*`) and is not a
/// reserved keyword. Used by `for`-loop variable names and function names.
fn valid_identifier_text(word: &Word) -> Option<String> {
    if word.0.len() != 1 {
        return None;
    }
    let WordPart::Literal { text, quoted: false } = &word.0[0] else {
        return None;
    };
    // Reject reserved keywords. Build a single-Word token to reuse keyword_of.
    let tok = Token::Word(Word(vec![WordPart::Literal {
        text: text.clone(),
        quoted: false,
    }]));
    if keyword_of(&tok).is_some() {
        return None;
    }
    let mut chars = text.chars();
    let first = chars.next()?;
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return None;
    }
    if !chars.all(|c| c == '_' || c.is_ascii_alphanumeric()) {
        return None;
    }
    Some(text.clone())
}

/// Returns the loop-variable name if `token` is a single, unquoted
/// `Literal` `Word` whose text is a valid identifier and not a reserved
/// keyword. Otherwise `None`.
fn for_variable_name(token: &Token) -> Option<String> {
    let Token::Word(w) = token else { return None };
    valid_identifier_text(w)
}

/// Splits `text` on `;` at paren depth 0. Useful for arith-for headers
/// where the body of `for ((init; cond; step))` may contain
/// parenthesized sub-expressions that should not split.
fn split_top_level_semi(text: &str) -> Vec<String> {
    let mut sections: Vec<String> = vec![String::new()];
    let mut depth: i32 = 0;
    for c in text.chars() {
        match c {
            '(' => {
                depth += 1;
                sections.last_mut().unwrap().push(c);
            }
            ')' => {
                depth -= 1;
                sections.last_mut().unwrap().push(c);
            }
            ';' if depth == 0 => sections.push(String::new()),
            _ => sections.last_mut().unwrap().push(c),
        }
    }
    sections
}

/// Triple of optional arith expressions — the three sections of an
/// arith-for header `((init; cond; step))`. Each may be `None`.
type ArithForHeaderTriple = (
    Option<crate::arith::ArithExpr>,
    Option<crate::arith::ArithExpr>,
    Option<crate::arith::ArithExpr>,
);

/// Splits an arith-for header into three optional arith expressions.
/// Empty sections (e.g., the cond in `((;;))`) yield `None`. Returns
/// `ArithForHeader` if the header doesn't split into exactly 3 sections.
fn parse_arith_for_header(text: &str) -> Result<ArithForHeaderTriple, ParseError> {
    let sections = split_top_level_semi(text);
    if sections.len() != 3 {
        return Err(ParseError::ArithForHeader(format!(
            "expected 3 sections separated by `;`, got {}",
            sections.len()
        )));
    }
    let parse_section = |s: &str| -> Result<Option<crate::arith::ArithExpr>, ParseError> {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            Ok(None)
        } else {
            crate::arith::parse(trimmed)
                .map(Some)
                .map_err(|e| ParseError::ArithBlock(e.to_string()))
        }
    };
    Ok((
        parse_section(&sections[0])?,
        parse_section(&sections[1])?,
        parse_section(&sections[2])?,
    ))
}

/// Parses the body of `for ((header)) [;|newline]* do BODY done`. The
/// caller has consumed `for` and verified the next token is
/// `Token::ArithBlock`. This function consumes the ArithBlock, the
/// separators before `do`, the `do` keyword, the body, and the `done`
/// keyword.
fn parse_arith_for_clause<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<ArithForClause, ParseError> {
    let header_text = match iter.next() {
        Some(Token::ArithBlock(text)) => text,
        _ => unreachable!("caller verified peek"),
    };
    let (init, cond, step) = parse_arith_for_header(&header_text)?;

    // Skip `;` and newline separators between the header and `do`.
    while matches!(
        iter.peek(),
        Some(Token::Op(Operator::Semi)) | Some(Token::Newline)
    ) {
        iter.next();
    }
    expect_keyword(iter, Keyword::Do, ParseError::UnterminatedLoop)?;

    let body = parse_compound_section(iter, &[Keyword::Done], ParseError::UnterminatedLoop)?;
    expect_keyword(iter, Keyword::Done, ParseError::UnterminatedLoop)?;

    Ok(ArithForClause { init, cond, step, body })
}

/// Dispatches `for` to either the POSIX form (`for VAR in WORDS; do ...`)
/// or the bash arith form (`for ((init;cond;step)) do ...`). Consumes
/// the `for` keyword itself.
fn parse_for_command<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<Command, ParseError> {
    expect_keyword(iter, Keyword::For, ParseError::UnterminatedLoop)?;

    // Peek the next token to choose the variant. Skip newlines first so
    // `for\n((...))` works the same as `for ((...))`.
    while matches!(iter.peek(), Some(Token::Newline)) {
        iter.next();
    }

    if matches!(iter.peek(), Some(Token::ArithBlock(_))) {
        return Ok(Command::ArithFor(Box::new(parse_arith_for_clause(iter)?)));
    }

    Ok(Command::For(Box::new(parse_for_after_keyword(iter)?)))
}

/// Parses `for NAME [in WORD...] sep do LIST done` AFTER the `for`
/// keyword has already been consumed by the caller.
fn parse_for_after_keyword<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<ForClause, ParseError> {
    // Loop variable. End-of-input means the command is incomplete (the
    // v19 classifier maps UnterminatedLoop to "read more"); a present
    // but invalid token is a genuine error.
    let var = match iter.next() {
        None => return Err(ParseError::UnterminatedLoop),
        Some(tok) => for_variable_name(&tok).ok_or(ParseError::ForVariable)?,
    };

    // POSIX allows a linebreak between the variable and `in`.
    skip_newlines(iter);

    // Optional `in` plus the word list.
    let mut words: Vec<Word> = Vec::new();
    if iter.peek().and_then(keyword_of) == Some(Keyword::In) {
        iter.next(); // consume `in`
        loop {
            match iter.peek() {
                None | Some(Token::Newline) | Some(Token::Op(Operator::Semi)) => break,
                Some(tok) => {
                    if keyword_of(tok) == Some(Keyword::Do) {
                        break;
                    }
                    match iter.next() {
                        Some(Token::Word(w)) => words.push(w),
                        Some(Token::Op(_)) => return Err(ParseError::UnexpectedToken),
                        _ => unreachable!("peek already ruled out Newline/Semi/None here"),
                    }
                }
            }
        }
    }

    // Skip `;`/newline separators, then `do`.
    while matches!(
        iter.peek(),
        Some(Token::Op(Operator::Semi)) | Some(Token::Newline)
    ) {
        iter.next();
    }
    expect_keyword(iter, Keyword::Do, ParseError::UnterminatedLoop)?;

    let body = parse_compound_section(iter, &[Keyword::Done], ParseError::UnterminatedLoop)?;
    expect_keyword(iter, Keyword::Done, ParseError::UnterminatedLoop)?;
    Ok(ForClause { var, words, body })
}

/// Parses `case WORD in [clause]... esac`. The caller has peeked `case`.
fn parse_case<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<CaseClause, ParseError> {
    expect_keyword(iter, Keyword::Case, ParseError::UnterminatedCase)?;
    skip_newlines(iter);

    let subject = match iter.next() {
        None => return Err(ParseError::UnterminatedCase),
        Some(Token::Word(w)) => w,
        Some(_) => return Err(ParseError::UnexpectedToken),
    };

    skip_newlines(iter);
    expect_keyword(iter, Keyword::In, ParseError::UnterminatedCase)?;
    skip_newlines(iter);

    let mut items: Vec<CaseItem> = Vec::new();
    while iter.peek().and_then(keyword_of) != Some(Keyword::Esac) {
        if iter.peek().is_none() {
            return Err(ParseError::UnterminatedCase);
        }
        items.push(parse_case_item(iter)?);
        skip_newlines(iter);
    }
    expect_keyword(iter, Keyword::Esac, ParseError::UnterminatedCase)?;
    Ok(CaseClause { subject, items })
}

/// Parses one `[(] pattern [| pattern]... ) [body] [terminator]` clause.
fn parse_case_item<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<CaseItem, ParseError> {
    // Optional leading `(`.
    if matches!(iter.peek(), Some(Token::Op(Operator::LParen))) {
        iter.next();
    }

    // Pattern list — Word (`|` Word)* `)`, non-empty.
    let mut patterns: Vec<Word> = Vec::new();
    loop {
        skip_newlines(iter);
        match iter.next() {
            None => return Err(ParseError::UnterminatedCase),
            Some(Token::Word(w)) => patterns.push(w),
            Some(_) => return Err(ParseError::UnexpectedToken),
        }
        match iter.peek() {
            None => return Err(ParseError::UnterminatedCase),
            Some(Token::Op(Operator::Pipe)) => {
                iter.next();
            }
            Some(Token::Op(Operator::RParen)) => {
                iter.next();
                break;
            }
            Some(_) => return Err(ParseError::UnexpectedToken),
        }
    }

    // Body — empty if the next token is a terminator or `esac`.
    skip_newlines(iter);
    let body = match iter.peek() {
        None => return Err(ParseError::UnterminatedCase),
        Some(Token::Op(
            Operator::DoubleSemi | Operator::SemiAmp | Operator::DoubleSemiAmp,
        )) => None,
        Some(tok) if keyword_of(tok) == Some(Keyword::Esac) => None,
        Some(_) => Some(parse_sequence(iter, &[Keyword::Esac])?),
    };

    // Terminator — an absent one (next token is `esac` or end) is `Break`.
    let terminator = match iter.peek() {
        Some(Token::Op(Operator::DoubleSemi)) => {
            iter.next();
            CaseTerminator::Break
        }
        Some(Token::Op(Operator::SemiAmp)) => {
            iter.next();
            CaseTerminator::FallThrough
        }
        Some(Token::Op(Operator::DoubleSemiAmp)) => {
            iter.next();
            CaseTerminator::ContinueMatch
        }
        _ => CaseTerminator::Break,
    };

    Ok(CaseItem { patterns, body, terminator })
}

/// Parses `{ LIST }`. The caller has peeked the leading `{`.
fn parse_brace_group<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<Sequence, ParseError> {
    expect_keyword(iter, Keyword::LBrace, ParseError::UnterminatedBrace)?;
    let body = parse_compound_section(iter, &[Keyword::RBrace], ParseError::UnterminatedBrace)?;
    expect_keyword(iter, Keyword::RBrace, ParseError::UnterminatedBrace)?;
    Ok(body)
}

/// Parses `( LIST )`. The caller has already confirmed the next token is `(`.
///
/// Consumes the leading `(`, parses an inner sequence stopping at `)`, then
/// expects the closing `)`. Returns:
/// - `Err(ParseError::EmptySubshell)` if the body is empty (bare `()`).
/// - `Err(ParseError::UnterminatedSubshell)` if no closing `)` is found.
/// - `Ok(Command::Subshell { body })` otherwise.
fn parse_subshell<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<Command, ParseError> {
    // Consume `(`.
    iter.next();

    // Empty subshell `()` — immediately hit `)` with no commands inside.
    if matches!(iter.peek(), Some(Token::Op(Operator::RParen))) {
        iter.next(); // consume `)`
        return Err(ParseError::EmptySubshell);
    }

    // No tokens at all → unterminated.
    if iter.peek().is_none() {
        return Err(ParseError::UnterminatedSubshell);
    }

    // Parse the inner sequence using parse_subshell_sequence, which mirrors
    // parse_sequence but terminates on `)` instead of on stop-keywords.
    let body = parse_subshell_sequence(iter)?;
    Ok(Command::Subshell { body: Box::new(body) })
}

/// Parses a sequence of commands terminated by `)`. Mirrors `parse_sequence`
/// but:
/// - breaks on `Token::Op(Operator::RParen)` (consuming it) instead of keywords.
/// - returns `Err(UnterminatedSubshell)` if the token stream ends before `)`.
fn parse_subshell_sequence<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<Sequence, ParseError> {
    // Parse first command. It may itself be a subshell, compound command, etc.
    let raw_first = parse_command(iter)?;

    // If followed by `|`, wrap into a pipeline (same logic as parse_sequence).
    let first = if matches!(iter.peek(), Some(Token::Op(Operator::Pipe))) {
        let mut stages = vec![raw_first];
        iter.next(); // consume `|`
        skip_newlines(iter);
        let mut more_stages = true;
        while more_stages {
            let (cmd, next_pipe) = parse_next_stage(iter)?;
            stages.push(cmd);
            if next_pipe {
                // simple stage already consumed `|`; continue.
            } else {
                if matches!(iter.peek(), Some(Token::Op(Operator::Pipe))) {
                    iter.next();
                    skip_newlines(iter);
                } else {
                    more_stages = false;
                }
            }
        }
        Command::Pipeline(Pipeline { commands: stages })
    } else {
        raw_first
    };

    let mut rest = Vec::new();
    loop {
        match iter.peek() {
            // End of tokens before `)` → unterminated.
            None => return Err(ParseError::UnterminatedSubshell),
            // `)` terminates the subshell body — consume and return.
            Some(Token::Op(Operator::RParen)) => {
                iter.next();
                break;
            }
            Some(Token::Op(Operator::Semi)) | Some(Token::Newline) => {
                iter.next(); // consume `;` or newline
                skip_newlines(iter);
                // Trailing `;` or newline before `)` — break cleanly.
                if matches!(iter.peek(), Some(Token::Op(Operator::RParen))) {
                    iter.next(); // consume `)`
                    break;
                }
                if iter.peek().is_none() {
                    return Err(ParseError::UnterminatedSubshell);
                }
                let raw = parse_command(iter)?;
                let cmd = if matches!(iter.peek(), Some(Token::Op(Operator::Pipe))) {
                    let mut stages = vec![raw];
                    iter.next();
                    skip_newlines(iter);
                    let mut more = true;
                    while more {
                        let (c, np) = parse_next_stage(iter)?;
                        stages.push(c);
                        if !np {
                            if matches!(iter.peek(), Some(Token::Op(Operator::Pipe))) {
                                iter.next();
                                skip_newlines(iter);
                            } else {
                                more = false;
                            }
                        }
                    }
                    Command::Pipeline(Pipeline { commands: stages })
                } else {
                    raw
                };
                rest.push((Connector::Semi, cmd));
            }
            Some(Token::Op(Operator::Background)) => {
                iter.next(); // consume `&`
                // `&` inside a subshell body backgrounds the preceding command
                // and acts as a separator. Skip any redundant `;` or newlines
                // that follow (`&;` is equivalent to `&` in bash).
                while matches!(
                    iter.peek(),
                    Some(Token::Op(Operator::Semi)) | Some(Token::Newline)
                ) {
                    iter.next();
                }
                skip_newlines(iter);
                // If `)` follows (or stream ends), this `&` terminates the
                // whole body as a backgrounded sequence.
                if matches!(iter.peek(), Some(Token::Op(Operator::RParen))) {
                    iter.next(); // consume `)`
                    return Ok(Sequence { first, rest, background: true });
                }
                if iter.peek().is_none() {
                    return Err(ParseError::UnterminatedSubshell);
                }
                // More commands follow (`(cmd1 &; cmd2)` pattern): parse the
                // next command and continue. The `&` acts as a `;` separator
                // here; only the trailing `&` before `)` sets background.
                let raw = parse_command(iter)?;
                let cmd = if matches!(iter.peek(), Some(Token::Op(Operator::Pipe))) {
                    let mut stages = vec![raw];
                    iter.next();
                    skip_newlines(iter);
                    let mut more = true;
                    while more {
                        let (c, np) = parse_next_stage(iter)?;
                        stages.push(c);
                        if !np {
                            if matches!(iter.peek(), Some(Token::Op(Operator::Pipe))) {
                                iter.next();
                                skip_newlines(iter);
                            } else {
                                more = false;
                            }
                        }
                    }
                    Command::Pipeline(Pipeline { commands: stages })
                } else {
                    raw
                };
                rest.push((Connector::Semi, cmd));
            }
            Some(Token::Op(Operator::And)) => {
                iter.next();
                skip_newlines(iter);
                rest.push((Connector::And, parse_command(iter)?));
            }
            Some(Token::Op(Operator::Or)) => {
                iter.next();
                skip_newlines(iter);
                rest.push((Connector::Or, parse_command(iter)?));
            }
            // Any other token (stray keyword, another `(`, etc.) after a
            // complete command and before `)` is unexpected.
            Some(_) => return Err(ParseError::UnterminatedSubshell),
        }
    }

    Ok(Sequence { first, rest, background: false })
}

/// Parses `while LIST; do LIST; done` or `until LIST; do LIST; done`.
/// The caller has already peeked the leading `while`/`until`.
fn parse_while<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<WhileClause, ParseError> {
    let until = match iter.next().as_ref().and_then(keyword_of) {
        Some(Keyword::While) => false,
        Some(Keyword::Until) => true,
        _ => unreachable!("parse_command guarantees a while/until keyword here"),
    };
    let condition = parse_compound_section(iter, &[Keyword::Do], ParseError::UnterminatedLoop)?;
    expect_keyword(iter, Keyword::Do, ParseError::UnterminatedLoop)?;
    let body = parse_compound_section(iter, &[Keyword::Done], ParseError::UnterminatedLoop)?;
    expect_keyword(iter, Keyword::Done, ParseError::UnterminatedLoop)?;
    Ok(WhileClause { condition, body, until })
}

/// Parses a simple-command stage — accumulates program/args/redirects
/// tokens until a pipeline terminator (`|`, `;`, `&&`, `||`, newline,
/// etc.) is reached. Returns `Command::Simple(...)` and a flag indicating
/// whether a `|` was consumed (meaning more stages follow).
///
/// The first word may be supplied as `first` (already consumed by the
/// caller for function-def lookahead). When `first` is `None` the
/// function reads the initial word from the iterator.
fn parse_simple_stage<I: Iterator<Item = Token>>(
    first: Option<Word>,
    prefix_tokens: Vec<Token>,
    iter: &mut std::iter::Peekable<I>,
) -> Result<(Command, bool), ParseError> {
    let mut program: Option<Word> = first;
    let mut args: Vec<Word> = Vec::new();
    let mut stdin: Option<Redirect> = None;
    let mut stdout: Option<Redirect> = None;
    let mut stderr: Option<Redirect> = None;
    let mut pipe_follows = false;

    // Drain prefix_tokens first (extra assignment words re-injected by the
    // multi-assign speculative-peel path). These are always Token::Word items
    // so we process them directly without going through the pipeline-terminator
    // peek-break that guards the main loop.
    for tok in prefix_tokens {
        match tok {
            Token::Word(word) => {
                if program.is_none() {
                    program = Some(word);
                } else {
                    args.push(word);
                }
            }
            // prefix_tokens only ever contains Token::Word items in the
            // current caller; guard other variants to prevent silent misbehaviour.
            _ => unreachable!("prefix_tokens should only contain Token::Word"),
        }
    }

    while let Some(token) = iter.peek() {
        if matches!(
            token,
            Token::Op(
                Operator::Semi
                    | Operator::And
                    | Operator::Or
                    | Operator::Background
                    | Operator::DoubleSemi
                    | Operator::SemiAmp
                    | Operator::DoubleSemiAmp
                    // RParen terminates a subshell body — stop without
                    // consuming so parse_subshell_sequence can handle it.
                    | Operator::RParen
            ) | Token::Newline
        ) {
            break;
        }
        let token = iter.next().unwrap();
        match token {
            Token::Word(word) => {
                if program.is_none() {
                    program = Some(word);
                } else {
                    args.push(word);
                }
            }
            Token::Newline => {
                // Unreachable: the peek-break above stops the loop on a
                // Newline before it is ever consumed here.
                unreachable!("Newline terminates the stage via the peek-break above");
            }
            Token::Op(Operator::Pipe) => {
                pipe_follows = true;
                skip_newlines(iter);
                break;
            }
            Token::Op(Operator::LParen) => {
                // A `(` mid-argument (e.g. `cmd (args)`) is a syntax error.
                // Note: `(` at command-start is dispatched by parse_command
                // before parse_simple_stage is called.
                return Err(ParseError::UnexpectedToken);
            }
            Token::ArithBlock(_) => {
                // `((...))` mid-argument (e.g. `cmd ((1+2))`) is a syntax
                // error. The standalone arith block at command-start is
                // dispatched by parse_command before parse_simple_stage runs.
                return Err(ParseError::UnexpectedToken);
            }
            Token::Op(Operator::RParen) => {
                // Unreachable: the peek-break above stops on RParen.
                unreachable!("RParen terminates the stage via the peek-break above");
            }
            Token::Heredoc { body, expand, strip_tabs } => {
                // A here-doc token produced by the lexer — attach as stdin.
                // Last-wins: a later heredoc or <file overwrites this.
                stdin = Some(Redirect::Heredoc { body, expand, strip_tabs });
            }
            Token::Op(op) => {
                let target = match iter.next() {
                    Some(Token::Word(word)) => word,
                    Some(Token::Op(_)) => return Err(ParseError::RedirectTargetIsOperator),
                    Some(Token::Newline) | None => return Err(ParseError::MissingRedirectTarget),
                    Some(Token::Heredoc { .. }) => return Err(ParseError::RedirectTargetIsOperator),
                    Some(Token::ArithBlock(_)) => return Err(ParseError::RedirectTargetIsOperator),
                };
                match op {
                    Operator::RedirIn => stdin = Some(Redirect::Read(target)),
                    Operator::RedirOut => stdout = Some(Redirect::Truncate(target)),
                    Operator::RedirAppend => stdout = Some(Redirect::Append(target)),
                    Operator::RedirErr => stderr = Some(Redirect::Truncate(target)),
                    Operator::RedirErrAppend => stderr = Some(Redirect::Append(target)),
                    Operator::HereString => stdin = Some(Redirect::HereString(target)),
                    Operator::DupOut => {
                        stdout = Some(Redirect::Dup { fd: 1, source: target });
                    }
                    Operator::DupErr => {
                        stderr = Some(Redirect::Dup { fd: 2, source: target });
                    }
                    Operator::AndRedirOut => {
                        stdout = Some(Redirect::Truncate(target));
                        stderr = Some(Redirect::Dup { fd: 2, source: lit_word("1") });
                    }
                    Operator::AndRedirAppend => {
                        stdout = Some(Redirect::Append(target));
                        stderr = Some(Redirect::Dup { fd: 2, source: lit_word("1") });
                    }
                    Operator::Pipe
                    | Operator::And
                    | Operator::Or
                    | Operator::Semi
                    | Operator::Background
                    | Operator::LParen
                    | Operator::RParen
                    | Operator::DoubleSemi
                    | Operator::SemiAmp
                    | Operator::DoubleSemiAmp => {
                        unreachable!("handled in the outer arms or peek-break");
                    }
                }
            }
        }
    }

    let prog = program.ok_or(ParseError::MissingCommand)?;
    let cmd = Command::Simple(finalize_stage(prog, args, stdin, stdout, stderr));
    Ok((cmd, pipe_follows))
}

/// Parses one pipeline stage after `|` has been consumed and `skip_newlines`
/// called. Dispatches on the next token:
///
/// - Compound keyword (`if`/`while`/`until`/`for`/`case`/`{`): parse the
///   compound command and return it directly.
/// - Word followed by `(`: parse a function definition.
/// - Otherwise: parse a simple stage (accumulating program/args/redirects),
///   delegating to `parse_simple_stage(None, iter)`.
///
/// Returns the parsed `Command` and whether a further `|` was consumed
/// (true only for simple stages that ended on `|`; compound stages never
/// consume a trailing `|` — the outer loop checks for it).
fn parse_next_stage<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<(Command, bool), ParseError> {
    // Standalone arith block: `((expr))` at pipeline-stage position
    // (e.g., `((x++)) | cat`). Mirrors the dispatch in parse_command.
    if matches!(iter.peek(), Some(Token::ArithBlock(_))) {
        let Some(Token::ArithBlock(text)) = iter.next() else {
            unreachable!("matches! guard above guarantees ArithBlock")
        };
        let expr = crate::arith::parse(&text)
            .map_err(|e| ParseError::ArithBlock(e.to_string()))?;
        return Ok((Command::Arith(expr), false));
    }

    match iter.peek().and_then(keyword_of) {
        Some(Keyword::If) => Ok((Command::If(Box::new(parse_if(iter)?)), false)),
        Some(Keyword::While) | Some(Keyword::Until) => {
            Ok((Command::While(Box::new(parse_while(iter)?)), false))
        }
        Some(Keyword::For) => Ok((parse_for_command(iter)?, false)),
        Some(Keyword::Case) => Ok((Command::Case(Box::new(parse_case(iter)?)), false)),
        Some(Keyword::LBrace) => {
            Ok((Command::BraceGroup(Box::new(parse_brace_group(iter)?)), false))
        }
        Some(Keyword::DoubleBracketOpen) => Ok((parse_double_bracket(iter)?, false)),
        Some(other) => Err(ParseError::UnexpectedKeyword(other.name().to_string())),
        None => {
            // Bare `(` at pipeline-stage position → subshell.
            if matches!(iter.peek(), Some(Token::Op(Operator::LParen))) {
                return Ok((parse_subshell(iter)?, false));
            }
            // Non-keyword: may be a function definition `name() compound` or
            // a plain simple stage. Need two-token lookahead.
            if matches!(iter.peek(), Some(Token::Word(_))) {
                let Some(Token::Word(w)) = iter.next() else { unreachable!() };
                if matches!(iter.peek(), Some(Token::Op(Operator::LParen))) {
                    let cmd = parse_function_def(w, iter)?;
                    return Ok((cmd, false));
                }
                // Not a function def — simple stage with `w` as the first word.
                parse_simple_stage(Some(w), vec![], iter)
            } else {
                parse_simple_stage(None, vec![], iter)
            }
        }
    }
}

fn parse_pipeline_with_first<I: Iterator<Item = Token>>(
    first: Option<Word>,
    prefix_tokens: Vec<Token>,
    iter: &mut std::iter::Peekable<I>,
) -> Result<Pipeline, ParseError> {
    let mut commands: Vec<Command> = Vec::new();

    // Parse the first stage as a simple stage (the caller has already
    // consumed the first word for function-def lookahead purposes, so we
    // pass it along). The first stage is always a simple command because
    // `parse_command` dispatches compound commands before calling us, so
    // we only arrive here for simple-command pipelines.
    let (first_cmd, mut pipe_follows) = parse_simple_stage(first, prefix_tokens, iter)?;
    commands.push(first_cmd);

    // For each subsequent stage (after `|`), dispatch via `parse_next_stage`
    // which handles both compound commands and simple stages.
    //
    // Note: nested multi-stage Command::Pipeline as a stage is not possible
    // today because subshell syntax `(list)` is not yet lexed (M-11). When
    // M-11 lands, add a rejection guard here for Command::Pipeline stages
    // with commands.len() > 1.
    while pipe_follows {
        let (cmd, next_pipe) = parse_next_stage(iter)?;
        commands.push(cmd);

        // If the stage was a compound command (next_pipe=false from
        // parse_next_stage), it did not consume a trailing `|`; check manually.
        if !next_pipe {
            if matches!(iter.peek(), Some(Token::Op(Operator::Pipe))) {
                iter.next(); // consume `|`
                skip_newlines(iter);
                // pipe_follows remains true — loop continues.
            } else {
                pipe_follows = false;
            }
        }
        // else: simple stage already consumed `|`; pipe_follows remains true.
    }

    Ok(Pipeline { commands })
}

fn parse_pipeline<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<Pipeline, ParseError> {
    parse_pipeline_with_first(None, vec![], iter)
}

// ──────────────────────────────────────────────────────────────
// [[ ]] extended test — Pratt-style parser (v30)
// ──────────────────────────────────────────────────────────────

/// Returns `true` if the next token signals the end of a `[[ ]]` expression
/// body (i.e. we should stop trying to parse more at this precedence level).
/// This means we've hit `]]` (DoubleBracketClose keyword), `)` (RParen for
/// grouped sub-expressions), or end of input.
fn is_test_expr_stop<I: Iterator<Item = Token>>(iter: &mut std::iter::Peekable<I>) -> bool {
    match iter.peek() {
        None => true,
        Some(tok) => keyword_of(tok) == Some(Keyword::DoubleBracketClose)
            || matches!(tok, Token::Op(Operator::RParen)),
    }
}

/// Returns the Word's single unquoted Literal text, if it is exactly that shape.
/// Used to identify operator words like `==`, `!=`, `-eq`, etc.
fn word_literal_text(w: &Word) -> Option<&str> {
    if w.0.len() != 1 {
        return None;
    }
    match &w.0[0] {
        WordPart::Literal { text, quoted: false } => Some(text.as_str()),
        _ => None,
    }
}

/// Try to parse a unary test operator from a Word token.  Returns `Some(op)`
/// if the word is a single unquoted literal matching one of the file/string
/// test flags, otherwise `None`.
fn try_unary_op(w: &Word) -> Option<TestUnaryOp> {
    match word_literal_text(w)? {
        "-e" => Some(TestUnaryOp::FileExists),
        "-f" => Some(TestUnaryOp::IsRegFile),
        "-d" => Some(TestUnaryOp::IsDir),
        "-r" => Some(TestUnaryOp::IsReadable),
        "-w" => Some(TestUnaryOp::IsWritable),
        "-x" => Some(TestUnaryOp::IsExecutable),
        "-s" => Some(TestUnaryOp::IsNonEmpty),
        "-L" => Some(TestUnaryOp::IsSymlink),
        "-n" => Some(TestUnaryOp::StringNonEmpty),
        "-z" => Some(TestUnaryOp::StringEmpty),
        _ => None,
    }
}


/// Returns true if `w` is the literal word `!` (unquoted).
fn is_bang_word(tok: &Token) -> bool {
    match tok {
        Token::Word(w) => word_literal_text(w) == Some("!"),
        _ => false,
    }
}

/// Reads the next token and asserts it is a Word. Returns the Word, or
/// `ParseError::TestExprMissingOperand` if the iterator is exhausted or the
/// token is not a Word.
fn next_test_word<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<Word, ParseError> {
    match iter.peek() {
        None => return Err(ParseError::TestExprMissingOperand),
        Some(tok) => {
            if keyword_of(tok) == Some(Keyword::DoubleBracketClose)
                || matches!(tok, Token::Op(_))
            {
                return Err(ParseError::TestExprMissingOperand);
            }
        }
    }
    match iter.next() {
        Some(Token::Word(w)) => Ok(w),
        _ => Err(ParseError::TestExprMissingOperand),
    }
}

/// Lowest precedence: `||`.
fn parse_test_or<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<TestExpr, ParseError> {
    let mut lhs = parse_test_and(iter)?;
    while matches!(iter.peek(), Some(Token::Op(Operator::Or))) {
        iter.next(); // consume `||`
        let rhs = parse_test_and(iter)?;
        lhs = TestExpr::Or(Box::new(lhs), Box::new(rhs));
    }
    Ok(lhs)
}

/// Next precedence: `&&`.
fn parse_test_and<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<TestExpr, ParseError> {
    let mut lhs = parse_test_not(iter)?;
    while matches!(iter.peek(), Some(Token::Op(Operator::And))) {
        iter.next(); // consume `&&`
        let rhs = parse_test_not(iter)?;
        lhs = TestExpr::And(Box::new(lhs), Box::new(rhs));
    }
    Ok(lhs)
}

/// Next precedence: `!` (right-associative).
fn parse_test_not<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<TestExpr, ParseError> {
    if iter.peek().map(is_bang_word).unwrap_or(false) {
        iter.next(); // consume `!`
        let inner = parse_test_not(iter)?;
        return Ok(TestExpr::Not(Box::new(inner)));
    }
    parse_test_primary(iter)
}

/// Highest precedence: `( expr )` grouping or a single test atom.
fn parse_test_primary<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<TestExpr, ParseError> {
    // Grouping: `( expr )`.
    if matches!(iter.peek(), Some(Token::Op(Operator::LParen))) {
        iter.next(); // consume `(`
        let inner = parse_test_or(iter)?;
        // Expect `)`.
        match iter.next() {
            Some(Token::Op(Operator::RParen)) => {}
            _ => return Err(ParseError::TestExprMissingOperand),
        }
        return Ok(inner);
    }
    parse_test_atom(iter)
}

/// Parses a single test — either a unary test (`-f path`) or a
/// binary/regex test (`lhs op rhs`).
fn parse_test_atom<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<TestExpr, ParseError> {
    // Nothing at all (empty body or stop token).
    if is_test_expr_stop(iter) {
        return Err(ParseError::EmptyDoubleBracket);
    }

    // Peek at the first word to check if it is a unary op.
    let first_word = match iter.peek() {
        Some(Token::Word(w)) => w.clone(),
        _ => return Err(ParseError::TestExprMissingOperand),
    };

    // Unary op path: `-f`, `-d`, `-n`, `-z`, …
    if let Some(op) = try_unary_op(&first_word) {
        iter.next(); // consume the op word
        let operand = next_test_word(iter)?;
        return Ok(TestExpr::Unary { op, operand });
    }

    // Binary / regex path: lhs op rhs.
    // Consume the LHS word (first_word peeked above).
    iter.next();
    let lhs = first_word;

    // Peek at the operator token.  Operators like `==`, `!=`, `=~`, `<`, `>`,
    // `-eq`, etc. arrive as Word tokens because the lexer doesn't have them as
    // separate operators.  `<` and `>` ARE lexer operator tokens (RedirIn /
    // RedirOut) — handle those too.
    let op_token = iter.next();
    match op_token {
        None => Err(ParseError::UnterminatedDoubleBracket),
        // `<` and `>` are lexed as RedirIn / RedirOut even inside `[[ ]]`.
        Some(Token::Op(Operator::RedirIn)) => {
            let rhs = next_test_word(iter)?;
            Ok(TestExpr::Binary { op: TestBinaryOp::StringLt, lhs, rhs })
        }
        Some(Token::Op(Operator::RedirOut)) => {
            let rhs = next_test_word(iter)?;
            Ok(TestExpr::Binary { op: TestBinaryOp::StringGt, lhs, rhs })
        }
        Some(Token::Word(op_word)) => {
            let op_text = match word_literal_text(&op_word) {
                Some(t) => t.to_string(),
                None => return Err(ParseError::TestExprBadOperator(
                    format!("{op_word:?}")
                )),
            };
            match op_text.as_str() {
                "==" | "=" => {
                    let rhs = next_test_word(iter)?;
                    Ok(TestExpr::Binary { op: TestBinaryOp::StringEq, lhs, rhs })
                }
                "!=" => {
                    let rhs = next_test_word(iter)?;
                    Ok(TestExpr::Binary { op: TestBinaryOp::StringNe, lhs, rhs })
                }
                "=~" => {
                    let pattern = next_test_word(iter)?;
                    Ok(TestExpr::Regex { lhs, pattern })
                }
                "<" => {
                    let rhs = next_test_word(iter)?;
                    Ok(TestExpr::Binary { op: TestBinaryOp::StringLt, lhs, rhs })
                }
                ">" => {
                    let rhs = next_test_word(iter)?;
                    Ok(TestExpr::Binary { op: TestBinaryOp::StringGt, lhs, rhs })
                }
                "-eq" => {
                    let rhs = next_test_word(iter)?;
                    Ok(TestExpr::Binary { op: TestBinaryOp::IntEq, lhs, rhs })
                }
                "-ne" => {
                    let rhs = next_test_word(iter)?;
                    Ok(TestExpr::Binary { op: TestBinaryOp::IntNe, lhs, rhs })
                }
                "-lt" => {
                    let rhs = next_test_word(iter)?;
                    Ok(TestExpr::Binary { op: TestBinaryOp::IntLt, lhs, rhs })
                }
                "-gt" => {
                    let rhs = next_test_word(iter)?;
                    Ok(TestExpr::Binary { op: TestBinaryOp::IntGt, lhs, rhs })
                }
                "-le" => {
                    let rhs = next_test_word(iter)?;
                    Ok(TestExpr::Binary { op: TestBinaryOp::IntLe, lhs, rhs })
                }
                "-ge" => {
                    let rhs = next_test_word(iter)?;
                    Ok(TestExpr::Binary { op: TestBinaryOp::IntGe, lhs, rhs })
                }
                other => Err(ParseError::TestExprBadOperator(other.to_string())),
            }
        }
        Some(other_tok) => {
            // Something unexpected (a keyword, stray operator, …).
            if keyword_of(&other_tok) == Some(Keyword::DoubleBracketClose) {
                // Consumed `]]` where the operator should be — unterminated.
                return Err(ParseError::UnterminatedDoubleBracket);
            }
            Err(ParseError::TestExprBadOperator(format!("{other_tok:?}")))
        }
    }
}

/// Parses `[[ EXPR ]]`. The caller has already peeked the `[[` keyword and
/// confirmed we should dispatch here (via `parse_command`).
///
/// `inline_assignments` carries any `NAME=value` words that appeared before `[[`
/// on the same command line (e.g. `FOO=hi [[ $FOO == hi ]]`).
///
/// Consumes `[[`, parses the test expression tree via Pratt precedence,
/// then consumes `]]`. Returns `Command::DoubleBracket { expr, inline_assignments }`.
fn parse_double_bracket<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<Command, ParseError> {
    parse_double_bracket_with_assigns(iter, Vec::new())
}

fn parse_double_bracket_with_assigns<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
    inline_assignments: Vec<Assignment>,
) -> Result<Command, ParseError> {
    // Consume `[[`.
    iter.next();

    // Immediately hit `]]` — empty body.
    if iter.peek().and_then(keyword_of) == Some(Keyword::DoubleBracketClose) {
        return Err(ParseError::EmptyDoubleBracket);
    }

    // End of input — unterminated.
    if iter.peek().is_none() {
        return Err(ParseError::UnterminatedDoubleBracket);
    }

    let expr = parse_test_or(iter)
        .map_err(|e| match e {
            // Propagate unterminated from deeper levels.
            ParseError::UnterminatedDoubleBracket => ParseError::UnterminatedDoubleBracket,
            other => other,
        })?;

    // Consume `]]`.
    match iter.next() {
        Some(tok) if keyword_of(&tok) == Some(Keyword::DoubleBracketClose) => {}
        None => return Err(ParseError::UnterminatedDoubleBracket),
        _ => return Err(ParseError::UnterminatedDoubleBracket),
    }

    Ok(Command::DoubleBracket { expr: Box::new(expr), inline_assignments })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::WordPart;

    fn w_tok(s: &str) -> Token {
        Token::Word(Word(vec![WordPart::Literal { text: s.to_string(), quoted: false }]))
    }

    fn ww(s: &str) -> Word {
        Word(vec![WordPart::Literal { text: s.to_string(), quoted: false }])
    }

    /// Builds a SimpleCommand::Exec with no redirections, all-Literal Words.
    fn plain(program: &str, args: &[&str]) -> SimpleCommand {
        SimpleCommand::Exec(ExecCommand {
            inline_assignments: Vec::new(),
            program: ww(program),
            args: args.iter().map(|a| ww(a)).collect(),
            stdin: None,
            stdout: None,
            stderr: None,
        })
    }

    fn one_pipeline(commands: Vec<SimpleCommand>) -> Sequence {
        Sequence {
            first: Command::Pipeline(Pipeline {
                commands: commands.into_iter().map(Command::Simple).collect(),
            }),
            rest: vec![],
            background: false,
        }
    }

    /// Reaches through `Command::Pipeline` for tests that inspect the first
    /// element as a pipeline.
    fn first_pipeline(seq: &Sequence) -> &Pipeline {
        match &seq.first {
            Command::Pipeline(p) => p,
            Command::Simple(_) => panic!("expected a pipeline, got a simple command"),
            Command::If(_) => panic!("expected a pipeline, got an if"),
            Command::While(_) => panic!("expected a pipeline, got a while"),
            Command::For(_) => panic!("expected a pipeline, got a for"),
            Command::Case(_) => panic!("expected a pipeline, got a case"),
            Command::BraceGroup(_) => panic!("expected a pipeline, got a brace group"),
            Command::Subshell { .. } => panic!("expected a pipeline, got a subshell"),
            Command::FunctionDef { .. } => panic!("expected a pipeline, got a function def"),
            Command::DoubleBracket { .. } => panic!("expected a pipeline, got a double bracket"),
            Command::Arith(_) => panic!("expected a pipeline, got an arith command"),
            Command::ArithFor(_) => panic!("expected a pipeline, got an arith for"),
        }
    }

    fn exec_stdout(seq: &Sequence) -> &Option<Redirect> {
        match &first_pipeline(seq).commands[0] {
            Command::Simple(SimpleCommand::Exec(e)) => &e.stdout,
            _ => panic!("expected Simple(Exec)"),
        }
    }

    fn exec_stdin(seq: &Sequence) -> &Option<Redirect> {
        match &first_pipeline(seq).commands[0] {
            Command::Simple(SimpleCommand::Exec(e)) => &e.stdin,
            _ => panic!("expected Simple(Exec)"),
        }
    }

    fn exec_stderr(seq: &Sequence) -> &Option<Redirect> {
        match &first_pipeline(seq).commands[0] {
            Command::Simple(SimpleCommand::Exec(e)) => &e.stderr,
            _ => panic!("expected Simple(Exec)"),
        }
    }

    #[test]
    fn parse_empty_returns_none() {
        assert_eq!(parse(vec![]), Ok(None));
    }

    #[test]
    fn parse_program_only() {
        assert_eq!(
            parse(vec![w_tok("ls")]),
            Ok(Some(one_pipeline(vec![plain("ls", &[])])))
        );
    }

    #[test]
    fn parse_program_with_args() {
        assert_eq!(
            parse(vec![w_tok("ls"), w_tok("-la"), w_tok("/tmp")]),
            Ok(Some(one_pipeline(vec![plain("ls", &["-la", "/tmp"])])))
        );
    }

    #[test]
    fn parse_redirect_out() {
        let seq = parse(vec![w_tok("ls"), Token::Op(Operator::RedirOut), w_tok("f")])
            .unwrap()
            .unwrap();
        assert_eq!(exec_stdout(&seq), &Some(Redirect::Truncate(ww("f"))));
    }

    #[test]
    fn parse_redirect_append() {
        let seq = parse(vec![w_tok("ls"), Token::Op(Operator::RedirAppend), w_tok("f")])
            .unwrap()
            .unwrap();
        assert_eq!(exec_stdout(&seq), &Some(Redirect::Append(ww("f"))));
    }

    #[test]
    fn parse_redirect_in() {
        let seq = parse(vec![w_tok("cat"), Token::Op(Operator::RedirIn), w_tok("f")])
            .unwrap()
            .unwrap();
        assert_eq!(exec_stdin(&seq), &Some(Redirect::Read(ww("f"))));
    }

    #[test]
    fn parse_redirect_stderr() {
        let seq = parse(vec![w_tok("cmd"), Token::Op(Operator::RedirErr), w_tok("e")])
            .unwrap()
            .unwrap();
        assert_eq!(exec_stderr(&seq), &Some(Redirect::Truncate(ww("e"))));
    }

    #[test]
    fn parse_redirect_stderr_append() {
        let seq = parse(vec![
            w_tok("cmd"),
            Token::Op(Operator::RedirErrAppend),
            w_tok("e"),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(exec_stderr(&seq), &Some(Redirect::Append(ww("e"))));
    }

    #[test]
    fn parse_two_stage_pipeline() {
        let seq = parse(vec![w_tok("a"), Token::Op(Operator::Pipe), w_tok("b")])
            .unwrap()
            .unwrap();
        assert_eq!(
            first_pipeline(&seq).commands,
            vec![Command::Simple(plain("a", &[])), Command::Simple(plain("b", &[]))]
        );
    }

    #[test]
    fn parse_three_stage_pipeline() {
        let seq = parse(vec![
            w_tok("a"),
            Token::Op(Operator::Pipe),
            w_tok("b"),
            Token::Op(Operator::Pipe),
            w_tok("c"),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(first_pipeline(&seq).commands.len(), 3);
    }

    #[test]
    fn parse_leading_pipe_is_missing_command() {
        assert_eq!(
            parse(vec![Token::Op(Operator::Pipe), w_tok("a")]),
            Err(ParseError::MissingCommand)
        );
    }

    #[test]
    fn parse_trailing_pipe_is_missing_command() {
        assert_eq!(
            parse(vec![w_tok("a"), Token::Op(Operator::Pipe)]),
            Err(ParseError::MissingCommand)
        );
    }

    #[test]
    fn parse_double_pipe_is_missing_command() {
        assert_eq!(
            parse(vec![
                w_tok("a"),
                Token::Op(Operator::Pipe),
                Token::Op(Operator::Pipe),
                w_tok("b"),
            ]),
            Err(ParseError::MissingCommand)
        );
    }

    #[test]
    fn parse_redirect_without_program_is_missing_command() {
        assert_eq!(
            parse(vec![Token::Op(Operator::RedirOut), w_tok("f")]),
            Err(ParseError::MissingCommand)
        );
    }

    #[test]
    fn parse_redirect_without_target_is_error() {
        assert_eq!(
            parse(vec![w_tok("ls"), Token::Op(Operator::RedirOut)]),
            Err(ParseError::MissingRedirectTarget)
        );
    }

    #[test]
    fn parse_redirect_target_is_operator_is_error() {
        assert_eq!(
            parse(vec![
                w_tok("ls"),
                Token::Op(Operator::RedirOut),
                Token::Op(Operator::Pipe),
                w_tok("b"),
            ]),
            Err(ParseError::RedirectTargetIsOperator)
        );
    }

    #[test]
    fn parse_semicolon_sequence() {
        let seq = parse(vec![w_tok("a"), Token::Op(Operator::Semi), w_tok("b")])
            .unwrap()
            .unwrap();
        assert_eq!(first_pipeline(&seq).commands, vec![Command::Simple(plain("a", &[]))]);
        assert_eq!(seq.rest.len(), 1);
        assert_eq!(seq.rest[0].0, Connector::Semi);
    }

    #[test]
    fn parse_and_sequence() {
        let seq = parse(vec![w_tok("a"), Token::Op(Operator::And), w_tok("b")])
            .unwrap()
            .unwrap();
        assert_eq!(seq.rest[0].0, Connector::And);
    }

    #[test]
    fn parse_or_sequence() {
        let seq = parse(vec![w_tok("a"), Token::Op(Operator::Or), w_tok("b")])
            .unwrap()
            .unwrap();
        assert_eq!(seq.rest[0].0, Connector::Or);
    }

    #[test]
    fn parse_trailing_semicolon_is_allowed() {
        let seq = parse(vec![w_tok("a"), Token::Op(Operator::Semi)])
            .unwrap()
            .unwrap();
        assert_eq!(first_pipeline(&seq).commands, vec![Command::Simple(plain("a", &[]))]);
        assert!(seq.rest.is_empty());
    }

    #[test]
    fn parse_trailing_and_is_missing_command() {
        assert_eq!(
            parse(vec![w_tok("a"), Token::Op(Operator::And)]),
            Err(ParseError::MissingCommand)
        );
    }

    #[test]
    fn parse_leading_semicolon_is_missing_command() {
        assert_eq!(
            parse(vec![Token::Op(Operator::Semi), w_tok("a")]),
            Err(ParseError::MissingCommand)
        );
    }

    #[test]
    fn parse_double_sequencing_op_is_missing_command() {
        assert_eq!(
            parse(vec![
                w_tok("a"),
                Token::Op(Operator::And),
                Token::Op(Operator::And),
                w_tok("b"),
            ]),
            Err(ParseError::MissingCommand)
        );
    }

    fn assignment(name: &str, value: Word) -> SimpleCommand {
        SimpleCommand::Assign(vec![Assignment {
            target: AssignTarget::Bare(name.to_string()),
            value,
            append: false,
        }])
    }

    #[test]
    fn parse_simple_assignment() {
        let seq = parse(vec![w_tok("FOO=bar")]).unwrap().unwrap();
        assert_eq!(first_pipeline(&seq).commands, vec![Command::Simple(assignment("FOO", ww("bar")))]);
    }

    #[test]
    fn parse_empty_value_assignment() {
        let seq = parse(vec![w_tok("FOO=")]).unwrap().unwrap();
        assert_eq!(first_pipeline(&seq).commands, vec![Command::Simple(assignment("FOO", ww("")))]);
    }

    #[test]
    fn parse_assignment_with_expansion_in_value() {
        let var_part = WordPart::Var { name: "BAR".to_string(), quoted: false };
        let prog = Token::Word(Word(vec![
            WordPart::Literal { text: "FOO=".to_string(), quoted: false },
            var_part,
        ]));
        let seq = parse(vec![prog]).unwrap().unwrap();
        let expected_value = Word(vec![
            WordPart::Literal { text: "".to_string(), quoted: false },
            WordPart::Var { name: "BAR".to_string(), quoted: false },
        ]);
        assert_eq!(first_pipeline(&seq).commands, vec![Command::Simple(assignment("FOO", expected_value))]);
    }

    #[test]
    fn parse_assignment_invalid_name_is_exec() {
        let seq = parse(vec![w_tok("1FOO=bar")]).unwrap().unwrap();
        assert_eq!(first_pipeline(&seq).commands, vec![Command::Simple(plain("1FOO=bar", &[]))]);
    }

    #[test]
    fn parse_assignment_with_arg_is_exec() {
        // `FOO=bar baz` — FOO=bar is an inline assignment; `baz` becomes the program.
        let seq = parse(vec![w_tok("FOO=bar"), w_tok("baz")]).unwrap().unwrap();
        match &first_pipeline(&seq).commands[0] {
            Command::Simple(SimpleCommand::Exec(e)) => {
                assert_eq!(e.inline_assignments.len(), 1);
                assert_eq!(e.inline_assignments[0].target.name(), "FOO");
                assert_eq!(e.program, ww("baz"));
                assert!(e.args.is_empty());
            }
            _ => panic!("expected Simple(Exec)"),
        }
    }

    #[test]
    fn parse_assignment_with_redirect_is_exec() {
        // `FOO=bar > f` — assignment prefix with redirect, no program word.
        let seq = parse(vec![
            w_tok("FOO=bar"),
            Token::Op(Operator::RedirOut),
            w_tok("f"),
        ])
        .unwrap()
        .unwrap();
        match &first_pipeline(&seq).commands[0] {
            Command::Simple(SimpleCommand::Exec(e)) => {
                assert_eq!(e.inline_assignments.len(), 1);
                assert_eq!(e.inline_assignments[0].target.name(), "FOO");
                assert_eq!(e.program, Word(Vec::new()));
                assert_eq!(e.stdout, Some(Redirect::Truncate(ww("f"))));
            }
            _ => panic!("expected Simple(Exec)"),
        }
    }

    #[test]
    fn parse_assignment_in_pipeline_stage() {
        let seq = parse(vec![
            w_tok("FOO=bar"),
            Token::Op(Operator::Pipe),
            w_tok("cat"),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(first_pipeline(&seq).commands.len(), 2);
        assert_eq!(first_pipeline(&seq).commands[0], Command::Simple(assignment("FOO", ww("bar"))));
        assert_eq!(first_pipeline(&seq).commands[1], Command::Simple(plain("cat", &[])));
    }

    #[test]
    fn parse_assignment_with_command_sub_value_moves_parts() {
        // Simulates lexer output for `FOO=$(echo bar)`: one Word with two
        // parts — Literal("FOO=") and CommandSub. The parser's
        // try_split_assignment must MOVE the CommandSub into the value
        // (it can't be cloned without Clone on Sequence). Asserts the
        // resulting Assign carries a value Word [Literal(""), CommandSub].
        use crate::lexer::WordPart;
        let inner_seq = Sequence {
            first: Command::Pipeline(Pipeline {
                commands: vec![Command::Simple(SimpleCommand::Exec(ExecCommand {
                    inline_assignments: Vec::new(),
                    program: ww("echo"),
                    args: vec![ww("bar")],
                    stdin: None,
                    stdout: None,
                    stderr: None,
                }))],
            }),
            rest: vec![],
            background: false,
        };
        let program_word = Word(vec![
            WordPart::Literal { text: "FOO=".to_string(), quoted: false },
            WordPart::CommandSub { sequence: inner_seq, quoted: false },
        ]);
        let seq = parse(vec![Token::Word(program_word)]).unwrap().unwrap();
        assert_eq!(first_pipeline(&seq).commands.len(), 1);
        match &first_pipeline(&seq).commands[0] {
            Command::Simple(SimpleCommand::Assign(items)) => {
                assert_eq!(items.len(), 1);
                let a = &items[0];
                assert_eq!(a.target.name(), "FOO");
                assert!(!a.append);
                assert_eq!(a.value.0.len(), 2);
                match &a.value.0[0] {
                    WordPart::Literal { text, .. } => assert_eq!(text, ""),
                    other => panic!("expected Literal(\"\"), got {other:?}"),
                }
                assert!(matches!(&a.value.0[1], WordPart::CommandSub { .. }));
            }
            other => panic!("expected Simple(Assign), got {other:?}"),
        }
    }

    #[test]
    fn parse_command_with_background() {
        let seq = parse(vec![w_tok("sleep"), w_tok("1"), Token::Op(Operator::Background)])
            .unwrap()
            .unwrap();
        assert!(seq.background);
        assert!(seq.rest.is_empty());
        assert_eq!(first_pipeline(&seq).commands, vec![Command::Simple(plain("sleep", &["1"]))]);
    }

    #[test]
    fn parse_pipeline_backgrounded() {
        // cmd1 | cmd2 &
        let seq = parse(vec![
            w_tok("cmd1"),
            Token::Op(Operator::Pipe),
            w_tok("cmd2"),
            Token::Op(Operator::Background),
        ])
        .unwrap()
        .unwrap();
        assert!(seq.background);
        assert!(seq.rest.is_empty());
        assert_eq!(first_pipeline(&seq).commands.len(), 2);
    }

    #[test]
    fn parse_background_alone_is_missing_command() {
        assert_eq!(
            parse(vec![Token::Op(Operator::Background)]),
            Err(ParseError::MissingCommand)
        );
    }

    #[test]
    fn parse_background_mid_sequence_is_error() {
        assert_eq!(
            parse(vec![
                w_tok("cmd1"),
                Token::Op(Operator::Background),
                w_tok("cmd2"),
            ]),
            Err(ParseError::UnexpectedBackground)
        );
    }

    #[test]
    fn parse_two_backgrounds_is_unexpected() {
        assert_eq!(
            parse(vec![
                w_tok("cmd"),
                Token::Op(Operator::Background),
                Token::Op(Operator::Background),
            ]),
            Err(ParseError::UnexpectedBackground)
        );
    }

    #[test]
    fn parse_and_then_bg_is_backgrounded_sequence() {
        // cmd1 && cmd2 &
        let seq = parse(vec![
            w_tok("cmd1"),
            Token::Op(Operator::And),
            w_tok("cmd2"),
            Token::Op(Operator::Background),
        ])
        .unwrap()
        .unwrap();
        assert!(seq.background, "expected background=true");
        assert_eq!(seq.rest.len(), 1, "expected one connector entry");
    }

    #[test]
    fn parse_semi_chain_bg_is_backgrounded_sequence() {
        // cmd1 ; cmd2 &
        let seq = parse(vec![
            w_tok("cmd1"),
            Token::Op(Operator::Semi),
            w_tok("cmd2"),
            Token::Op(Operator::Background),
        ])
        .unwrap()
        .unwrap();
        assert!(seq.background, "expected background=true");
        assert_eq!(seq.rest.len(), 1);
    }

    #[test]
    fn parse_background_mid_sequence_after_andor_is_unexpected_background() {
        // cmd1 && cmd2 & cmd3 — the `&` is followed by another token,
        // which is now the only error (UnexpectedBackground). Previously
        // BackgroundedMultiPipelineSequence won; with that variant
        // removed, UnexpectedBackground is the appropriate error.
        assert_eq!(
            parse(vec![
                w_tok("cmd1"),
                Token::Op(Operator::And),
                w_tok("cmd2"),
                Token::Op(Operator::Background),
                w_tok("cmd3"),
            ]),
            Err(ParseError::UnexpectedBackground)
        );
    }

    #[test]
    fn parse_long_chain_bg() {
        // cmd1 && cmd2 || cmd3 ; cmd4 &
        let seq = parse(vec![
            w_tok("cmd1"),
            Token::Op(Operator::And),
            w_tok("cmd2"),
            Token::Op(Operator::Or),
            w_tok("cmd3"),
            Token::Op(Operator::Semi),
            w_tok("cmd4"),
            Token::Op(Operator::Background),
        ])
        .unwrap()
        .unwrap();
        assert!(seq.background, "expected background=true");
        assert_eq!(seq.rest.len(), 3, "expected 3 connector entries");
    }

    /// A bare unquoted keyword token (same shape as an ordinary word).
    fn kw(s: &str) -> Token {
        w_tok(s)
    }

    /// Extracts the IfClause from a sequence whose first command is an If.
    fn first_if(seq: &Sequence) -> &IfClause {
        match &seq.first {
            Command::If(c) => c,
            Command::Simple(_) => panic!("expected an if, got a simple command"),
            Command::Pipeline(_) => panic!("expected an if, got a pipeline"),
            Command::While(_) => panic!("expected an if, got a while"),
            Command::For(_) => panic!("expected an if, got a for"),
            Command::Case(_) => panic!("expected an if, got a case"),
            Command::BraceGroup(_) => panic!("expected an if, got a brace group"),
            Command::Subshell { .. } => panic!("expected an if, got a subshell"),
            Command::FunctionDef { .. } => panic!("expected an if, got a function def"),
            Command::DoubleBracket { .. } => panic!("expected an if, got a double bracket"),
            Command::Arith(_) => panic!("expected an if, got an arith command"),
            Command::ArithFor(_) => panic!("expected an if, got an arith for"),
        }
    }

    /// Extracts the WhileClause from a sequence whose first command is a While.
    fn first_while(seq: &Sequence) -> &WhileClause {
        match &seq.first {
            Command::While(c) => c,
            other => panic!("expected a while, got {other:?}"),
        }
    }

    /// Extracts the ForClause from a sequence whose first command is a For.
    fn first_for(seq: &Sequence) -> &ForClause {
        match &seq.first {
            Command::For(c) => c,
            other => panic!("expected a for, got {other:?}"),
        }
    }

    /// Extracts the CaseClause from a sequence whose first command is a Case.
    fn first_case(seq: &Sequence) -> &CaseClause {
        match &seq.first {
            Command::Case(c) => c,
            other => panic!("expected a case, got {other:?}"),
        }
    }

    #[test]
    fn parse_simple_case() {
        let seq = parse(vec![
            kw("case"), w_tok("x"), kw("in"),
            w_tok("a"), Token::Op(Operator::RParen), w_tok("echo"), w_tok("hi"),
            Token::Op(Operator::DoubleSemi),
            kw("esac"),
        ]).unwrap().unwrap();
        let clause = first_case(&seq);
        assert_eq!(clause.items.len(), 1);
        assert_eq!(clause.items[0].patterns.len(), 1);
        assert_eq!(clause.items[0].terminator, CaseTerminator::Break);
        assert!(clause.items[0].body.is_some());
    }

    #[test]
    fn parse_case_multiline_matches_singleline() {
        let multiline = parse(vec![
            kw("case"), w_tok("x"), kw("in"), Token::Newline,
            w_tok("a"), Token::Op(Operator::RParen), w_tok("echo"), Token::Newline,
            Token::Op(Operator::DoubleSemi), Token::Newline,
            kw("esac"),
        ]).unwrap().unwrap();
        let singleline = parse(vec![
            kw("case"), w_tok("x"), kw("in"),
            w_tok("a"), Token::Op(Operator::RParen), w_tok("echo"),
            Token::Op(Operator::DoubleSemi),
            kw("esac"),
        ]).unwrap().unwrap();
        assert_eq!(multiline, singleline);
    }

    #[test]
    fn parse_case_alternation() {
        let seq = parse(vec![
            kw("case"), w_tok("x"), kw("in"),
            w_tok("a"), Token::Op(Operator::Pipe), w_tok("b"),
            Token::Op(Operator::Pipe), w_tok("c"), Token::Op(Operator::RParen),
            w_tok("echo"), Token::Op(Operator::DoubleSemi),
            kw("esac"),
        ]).unwrap().unwrap();
        assert_eq!(first_case(&seq).items[0].patterns.len(), 3);
    }

    #[test]
    fn parse_case_leading_paren() {
        let seq = parse(vec![
            kw("case"), w_tok("x"), kw("in"),
            Token::Op(Operator::LParen), w_tok("a"), Token::Op(Operator::RParen),
            w_tok("echo"), Token::Op(Operator::DoubleSemi),
            kw("esac"),
        ]).unwrap().unwrap();
        assert_eq!(first_case(&seq).items[0].patterns.len(), 1);
    }

    #[test]
    fn parse_case_empty_body() {
        let seq = parse(vec![
            kw("case"), w_tok("x"), kw("in"),
            w_tok("a"), Token::Op(Operator::RParen),
            Token::Op(Operator::DoubleSemi),
            kw("esac"),
        ]).unwrap().unwrap();
        assert!(first_case(&seq).items[0].body.is_none());
    }

    #[test]
    fn parse_case_terminators() {
        let seq = parse(vec![
            kw("case"), w_tok("x"), kw("in"),
            w_tok("a"), Token::Op(Operator::RParen), w_tok("echo"),
            Token::Op(Operator::DoubleSemi),
            w_tok("b"), Token::Op(Operator::RParen), w_tok("echo"),
            Token::Op(Operator::SemiAmp),
            w_tok("c"), Token::Op(Operator::RParen), w_tok("echo"),
            Token::Op(Operator::DoubleSemiAmp),
            kw("esac"),
        ]).unwrap().unwrap();
        let items = &first_case(&seq).items;
        assert_eq!(items[0].terminator, CaseTerminator::Break);
        assert_eq!(items[1].terminator, CaseTerminator::FallThrough);
        assert_eq!(items[2].terminator, CaseTerminator::ContinueMatch);
    }

    #[test]
    fn parse_case_omitted_final_terminator() {
        // case x in a) echo ; esac — last clause with the `;;` omitted; a
        // separator (here `;`) is required before `esac`, as for `fi`/`done`.
        let seq = parse(vec![
            kw("case"), w_tok("x"), kw("in"),
            w_tok("a"), Token::Op(Operator::RParen), w_tok("echo"),
            Token::Op(Operator::Semi),
            kw("esac"),
        ]).unwrap().unwrap();
        assert_eq!(first_case(&seq).items[0].terminator, CaseTerminator::Break);
    }

    #[test]
    fn parse_case_empty() {
        let seq = parse(vec![kw("case"), w_tok("x"), kw("in"), kw("esac")])
            .unwrap()
            .unwrap();
        assert!(first_case(&seq).items.is_empty());
    }

    #[test]
    fn parse_case_unterminated_is_unterminated_case() {
        assert_eq!(parse(vec![kw("case")]), Err(ParseError::UnterminatedCase));
        assert_eq!(
            parse(vec![kw("case"), w_tok("x")]),
            Err(ParseError::UnterminatedCase)
        );
        assert_eq!(
            parse(vec![kw("case"), w_tok("x"), kw("in")]),
            Err(ParseError::UnterminatedCase)
        );
        assert_eq!(
            parse(vec![
                kw("case"), w_tok("x"), kw("in"),
                w_tok("a"), Token::Op(Operator::RParen), w_tok("echo"),
                Token::Op(Operator::DoubleSemi),
            ]),
            Err(ParseError::UnterminatedCase)
        );
    }

    #[test]
    fn parse_case_malformed_pattern_list_errors() {
        assert_eq!(
            parse(vec![
                kw("case"), w_tok("x"), kw("in"),
                w_tok("a"), w_tok("b"), Token::Op(Operator::RParen),
                w_tok("echo"), Token::Op(Operator::DoubleSemi),
                kw("esac"),
            ]),
            Err(ParseError::UnexpectedToken)
        );
    }

    #[test]
    fn parse_simple_for() {
        // for x in a b c ; do echo ; done
        let seq = parse(vec![
            kw("for"), w_tok("x"), kw("in"),
            w_tok("a"), w_tok("b"), w_tok("c"), Token::Op(Operator::Semi),
            kw("do"), w_tok("echo"), Token::Op(Operator::Semi),
            kw("done"),
        ]).unwrap().unwrap();
        let clause = first_for(&seq);
        assert_eq!(clause.var, "x");
        assert_eq!(clause.words.len(), 3);
        assert_eq!(
            clause.body.first,
            Command::Pipeline(Pipeline { commands: vec![Command::Simple(plain("echo", &[]))] })
        );
    }

    #[test]
    fn parse_for_multiline_matches_singleline() {
        let multiline = parse(vec![
            kw("for"), w_tok("x"), kw("in"), w_tok("a"), Token::Newline,
            kw("do"), w_tok("echo"), Token::Newline,
            kw("done"),
        ]).unwrap().unwrap();
        let singleline = parse(vec![
            kw("for"), w_tok("x"), kw("in"), w_tok("a"), Token::Op(Operator::Semi),
            kw("do"), w_tok("echo"), Token::Op(Operator::Semi),
            kw("done"),
        ]).unwrap().unwrap();
        assert_eq!(multiline, singleline);
    }

    #[test]
    fn parse_for_no_in_has_empty_words() {
        let seq = parse(vec![
            kw("for"), w_tok("x"), Token::Op(Operator::Semi),
            kw("do"), w_tok("echo"), Token::Op(Operator::Semi),
            kw("done"),
        ]).unwrap().unwrap();
        assert!(first_for(&seq).words.is_empty());
    }

    #[test]
    fn parse_for_empty_in_list() {
        let seq = parse(vec![
            kw("for"), w_tok("x"), kw("in"), Token::Op(Operator::Semi),
            kw("do"), w_tok("echo"), Token::Op(Operator::Semi),
            kw("done"),
        ]).unwrap().unwrap();
        assert!(first_for(&seq).words.is_empty());
    }

    #[test]
    fn parse_for_do_terminates_word_list() {
        let seq = parse(vec![
            kw("for"), w_tok("x"), kw("in"), w_tok("a"), w_tok("b"),
            kw("do"), w_tok("echo"), Token::Op(Operator::Semi),
            kw("done"),
        ]).unwrap().unwrap();
        assert_eq!(first_for(&seq).words.len(), 2);
    }

    #[test]
    fn parse_for_keyword_words_in_list() {
        let seq = parse(vec![
            kw("for"), w_tok("x"), kw("in"), w_tok("then"), w_tok("else"),
            Token::Op(Operator::Semi),
            kw("do"), w_tok("echo"), Token::Op(Operator::Semi),
            kw("done"),
        ]).unwrap().unwrap();
        assert_eq!(first_for(&seq).words.len(), 2);
    }

    #[test]
    fn parse_for_in_on_next_line() {
        let seq = parse(vec![
            kw("for"), w_tok("x"), Token::Newline,
            kw("in"), w_tok("a"), Token::Newline,
            kw("do"), w_tok("echo"), Token::Newline,
            kw("done"),
        ]).unwrap().unwrap();
        let clause = first_for(&seq);
        assert_eq!(clause.var, "x");
        assert_eq!(clause.words.len(), 1);
    }

    #[test]
    fn parse_for_invalid_variable_name_errors() {
        assert_eq!(
            parse(vec![
                kw("for"), w_tok("2x"), kw("in"), w_tok("a"), Token::Op(Operator::Semi),
                kw("do"), w_tok("echo"), Token::Op(Operator::Semi), kw("done"),
            ]),
            Err(ParseError::ForVariable)
        );
    }

    #[test]
    fn parse_for_keyword_as_variable_errors() {
        assert_eq!(
            parse(vec![
                kw("for"), kw("in"), w_tok("a"), Token::Op(Operator::Semi),
                kw("do"), w_tok("echo"), Token::Op(Operator::Semi), kw("done"),
            ]),
            Err(ParseError::ForVariable)
        );
    }

    #[test]
    fn parse_for_unterminated_is_unterminated_loop() {
        assert_eq!(
            parse(vec![kw("for"), w_tok("x"), kw("in"), w_tok("a")]),
            Err(ParseError::UnterminatedLoop)
        );
        assert_eq!(parse(vec![kw("for")]), Err(ParseError::UnterminatedLoop));
    }

    #[test]
    fn parse_for_operator_in_word_list_errors() {
        assert_eq!(
            parse(vec![
                kw("for"), w_tok("x"), kw("in"), w_tok("a"),
                Token::Op(Operator::Pipe), w_tok("b"), Token::Op(Operator::Semi),
                kw("do"), w_tok("echo"), Token::Op(Operator::Semi), kw("done"),
            ]),
            Err(ParseError::UnexpectedToken)
        );
    }

    #[test]
    fn parse_simple_if() {
        let seq = parse(vec![
            kw("if"), w_tok("a"), Token::Op(Operator::Semi),
            kw("then"), w_tok("b"), Token::Op(Operator::Semi),
            kw("fi"),
        ]).unwrap().unwrap();
        let c = first_if(&seq);
        assert_eq!(c.condition.first, Command::Pipeline(Pipeline { commands: vec![Command::Simple(plain("a", &[]))] }));
        assert_eq!(c.then_body.first, Command::Pipeline(Pipeline { commands: vec![Command::Simple(plain("b", &[]))] }));
        assert!(c.elif_branches.is_empty());
        assert!(c.else_body.is_none());
    }

    #[test]
    fn parse_if_else() {
        let seq = parse(vec![
            kw("if"), w_tok("a"), Token::Op(Operator::Semi),
            kw("then"), w_tok("b"), Token::Op(Operator::Semi),
            kw("else"), w_tok("c"), Token::Op(Operator::Semi),
            kw("fi"),
        ]).unwrap().unwrap();
        assert!(first_if(&seq).else_body.is_some());
    }

    #[test]
    fn parse_if_elif_else() {
        let seq = parse(vec![
            kw("if"), w_tok("a"), Token::Op(Operator::Semi),
            kw("then"), w_tok("b"), Token::Op(Operator::Semi),
            kw("elif"), w_tok("c"), Token::Op(Operator::Semi),
            kw("then"), w_tok("d"), Token::Op(Operator::Semi),
            kw("else"), w_tok("e"), Token::Op(Operator::Semi),
            kw("fi"),
        ]).unwrap().unwrap();
        let c = first_if(&seq);
        assert_eq!(c.elif_branches.len(), 1);
        assert!(c.else_body.is_some());
    }

    #[test]
    fn parse_if_with_andor_condition() {
        let seq = parse(vec![
            kw("if"), w_tok("a"), Token::Op(Operator::And), w_tok("b"),
            Token::Op(Operator::Semi),
            kw("then"), w_tok("c"), Token::Op(Operator::Semi),
            kw("fi"),
        ]).unwrap().unwrap();
        let c = first_if(&seq);
        assert_eq!(c.condition.rest.len(), 1);
        assert_eq!(c.condition.rest[0].0, Connector::And);
    }

    #[test]
    fn parse_if_multi_command_body() {
        let seq = parse(vec![
            kw("if"), w_tok("a"), Token::Op(Operator::Semi),
            kw("then"), w_tok("b"), Token::Op(Operator::Semi), w_tok("c"),
            Token::Op(Operator::Semi),
            kw("fi"),
        ]).unwrap().unwrap();
        assert_eq!(first_if(&seq).then_body.rest.len(), 1);
    }

    #[test]
    fn parse_if_followed_by_command() {
        let seq = parse(vec![
            kw("if"), w_tok("a"), Token::Op(Operator::Semi),
            kw("then"), w_tok("b"), Token::Op(Operator::Semi),
            kw("fi"), Token::Op(Operator::Semi), w_tok("echo"),
        ]).unwrap().unwrap();
        assert!(matches!(seq.first, Command::If(_)));
        assert_eq!(seq.rest.len(), 1);
        assert_eq!(seq.rest[0].0, Connector::Semi);
        assert!(matches!(seq.rest[0].1, Command::Pipeline(_)));
    }

    #[test]
    fn parse_if_joined_with_and() {
        let seq = parse(vec![
            kw("if"), w_tok("a"), Token::Op(Operator::Semi),
            kw("then"), w_tok("b"), Token::Op(Operator::Semi),
            kw("fi"), Token::Op(Operator::And), w_tok("echo"),
        ]).unwrap().unwrap();
        assert_eq!(seq.rest[0].0, Connector::And);
    }

    #[test]
    fn parse_nested_if() {
        let seq = parse(vec![
            kw("if"), w_tok("a"), Token::Op(Operator::Semi),
            kw("then"),
            kw("if"), w_tok("b"), Token::Op(Operator::Semi),
            kw("then"), w_tok("c"), Token::Op(Operator::Semi),
            kw("fi"), Token::Op(Operator::Semi),
            kw("fi"),
        ]).unwrap().unwrap();
        assert!(matches!(first_if(&seq).then_body.first, Command::If(_)));
    }

    #[test]
    fn parse_if_unterminated_is_error() {
        let r = parse(vec![
            kw("if"), w_tok("a"), Token::Op(Operator::Semi),
            kw("then"), w_tok("b"),
        ]);
        assert_eq!(r, Err(ParseError::UnterminatedIf));
    }

    #[test]
    fn parse_if_missing_then_is_error() {
        let r = parse(vec![
            kw("if"), w_tok("a"), Token::Op(Operator::Semi), kw("fi"),
        ]);
        assert!(matches!(r, Err(ParseError::UnexpectedKeyword(_))));
    }

    #[test]
    fn parse_bare_then_is_unexpected_keyword() {
        assert!(matches!(
            parse(vec![kw("then"), w_tok("x")]),
            Err(ParseError::UnexpectedKeyword(_))
        ));
    }

    #[test]
    fn parse_bare_fi_is_unexpected_keyword() {
        assert!(matches!(
            parse(vec![kw("fi")]),
            Err(ParseError::UnexpectedKeyword(_))
        ));
    }

    #[test]
    fn parse_if_empty_condition_is_missing_command() {
        let r = parse(vec![
            kw("if"), Token::Op(Operator::Semi),
            kw("then"), w_tok("b"), Token::Op(Operator::Semi), kw("fi"),
        ]);
        assert_eq!(r, Err(ParseError::MissingCommand));
    }

    #[test]
    fn parse_keyword_as_argument_is_literal() {
        let seq = parse(vec![w_tok("echo"), w_tok("if")]).unwrap().unwrap();
        assert_eq!(seq.first, Command::Pipeline(Pipeline {
            commands: vec![Command::Simple(plain("echo", &["if"]))],
        }));
    }

    #[test]
    fn parse_trailing_keyword_after_if_is_unexpected_keyword() {
        // `if a; then b; fi fi` — a stray `fi` after a complete `if`.
        // Must be a clean parse error, never a panic.
        let r = parse(vec![
            kw("if"), w_tok("a"), Token::Op(Operator::Semi),
            kw("then"), w_tok("b"), Token::Op(Operator::Semi),
            kw("fi"), kw("fi"),
        ]);
        assert!(matches!(r, Err(ParseError::UnexpectedKeyword(_))), "got {r:?}");
    }

    #[test]
    fn parse_if_condition_with_background_is_error() {
        // `&` is not allowed inside an `if` condition/body (v17 limitation).
        let r = parse(vec![
            kw("if"), w_tok("a"), Token::Op(Operator::Background),
            kw("then"), w_tok("b"), Token::Op(Operator::Semi), kw("fi"),
        ]);
        assert_eq!(r, Err(ParseError::UnexpectedBackground));
    }

    #[test]
    fn parse_simple_while() {
        let seq = parse(vec![
            kw("while"), w_tok("a"), Token::Op(Operator::Semi),
            kw("do"), w_tok("b"), Token::Op(Operator::Semi),
            kw("done"),
        ]).unwrap().unwrap();
        let c = first_while(&seq);
        assert!(!c.until);
        assert_eq!(c.condition.first, Command::Pipeline(Pipeline { commands: vec![Command::Simple(plain("a", &[]))] }));
        assert_eq!(c.body.first, Command::Pipeline(Pipeline { commands: vec![Command::Simple(plain("b", &[]))] }));
    }

    #[test]
    fn parse_until_sets_flag() {
        let seq = parse(vec![
            kw("until"), w_tok("a"), Token::Op(Operator::Semi),
            kw("do"), w_tok("b"), Token::Op(Operator::Semi),
            kw("done"),
        ]).unwrap().unwrap();
        assert!(first_while(&seq).until);
    }

    #[test]
    fn parse_while_andor_condition() {
        let seq = parse(vec![
            kw("while"), w_tok("a"), Token::Op(Operator::And), w_tok("b"),
            Token::Op(Operator::Semi),
            kw("do"), w_tok("c"), Token::Op(Operator::Semi),
            kw("done"),
        ]).unwrap().unwrap();
        let c = first_while(&seq);
        assert_eq!(c.condition.rest.len(), 1);
        assert_eq!(c.condition.rest[0].0, Connector::And);
    }

    #[test]
    fn parse_while_multi_command_body() {
        let seq = parse(vec![
            kw("while"), w_tok("a"), Token::Op(Operator::Semi),
            kw("do"), w_tok("b"), Token::Op(Operator::Semi), w_tok("c"),
            Token::Op(Operator::Semi),
            kw("done"),
        ]).unwrap().unwrap();
        assert_eq!(first_while(&seq).body.rest.len(), 1);
    }

    #[test]
    fn parse_while_followed_by_command() {
        let seq = parse(vec![
            kw("while"), w_tok("a"), Token::Op(Operator::Semi),
            kw("do"), w_tok("b"), Token::Op(Operator::Semi),
            kw("done"), Token::Op(Operator::Semi), w_tok("echo"),
        ]).unwrap().unwrap();
        assert!(matches!(seq.first, Command::While(_)));
        assert_eq!(seq.rest.len(), 1);
        assert!(matches!(seq.rest[0].1, Command::Pipeline(_)));
    }

    #[test]
    fn parse_nested_while() {
        let seq = parse(vec![
            kw("while"), w_tok("a"), Token::Op(Operator::Semi),
            kw("do"),
            kw("while"), w_tok("b"), Token::Op(Operator::Semi),
            kw("do"), w_tok("c"), Token::Op(Operator::Semi),
            kw("done"), Token::Op(Operator::Semi),
            kw("done"),
        ]).unwrap().unwrap();
        assert!(matches!(first_while(&seq).body.first, Command::While(_)));
    }

    #[test]
    fn parse_while_with_if_body() {
        let seq = parse(vec![
            kw("while"), w_tok("a"), Token::Op(Operator::Semi),
            kw("do"),
            kw("if"), w_tok("b"), Token::Op(Operator::Semi),
            kw("then"), w_tok("c"), Token::Op(Operator::Semi),
            kw("fi"), Token::Op(Operator::Semi),
            kw("done"),
        ]).unwrap().unwrap();
        assert!(matches!(first_while(&seq).body.first, Command::If(_)));
    }

    #[test]
    fn parse_while_unterminated_is_error() {
        let r = parse(vec![
            kw("while"), w_tok("a"), Token::Op(Operator::Semi),
            kw("do"), w_tok("b"),
        ]);
        assert_eq!(r, Err(ParseError::UnterminatedLoop));
    }

    #[test]
    fn parse_while_missing_do_is_error() {
        let r = parse(vec![
            kw("while"), w_tok("a"), Token::Op(Operator::Semi), kw("done"),
        ]);
        assert!(matches!(r, Err(ParseError::UnexpectedKeyword(_))));
    }

    #[test]
    fn parse_bare_do_is_unexpected_keyword() {
        assert!(matches!(
            parse(vec![kw("do"), w_tok("x")]),
            Err(ParseError::UnexpectedKeyword(_))
        ));
    }

    #[test]
    fn parse_bare_done_is_unexpected_keyword() {
        assert!(matches!(
            parse(vec![kw("done")]),
            Err(ParseError::UnexpectedKeyword(_))
        ));
    }

    #[test]
    fn parse_while_empty_condition_is_missing_command() {
        let r = parse(vec![
            kw("while"), Token::Op(Operator::Semi),
            kw("do"), w_tok("b"), Token::Op(Operator::Semi), kw("done"),
        ]);
        assert_eq!(r, Err(ParseError::MissingCommand));
    }

    #[test]
    fn parse_while_background_in_body_is_error() {
        let r = parse(vec![
            kw("while"), w_tok("a"), Token::Op(Operator::Semi),
            kw("do"), w_tok("b"), Token::Op(Operator::Background),
            kw("done"),
        ]);
        assert_eq!(r, Err(ParseError::UnexpectedBackground));
    }

    #[test]
    fn parse_keyword_while_as_argument_is_literal() {
        let seq = parse(vec![w_tok("echo"), w_tok("while")]).unwrap().unwrap();
        assert_eq!(seq.first, Command::Pipeline(Pipeline {
            commands: vec![Command::Simple(plain("echo", &["while"]))],
        }));
    }

    #[test]
    fn multiline_if_parses_same_as_singleline() {
        let multiline = parse(vec![
            kw("if"), w_tok("a"), Token::Newline,
            kw("then"), w_tok("b"), Token::Newline,
            kw("fi"),
        ]).unwrap().unwrap();
        let singleline = parse(vec![
            kw("if"), w_tok("a"), Token::Op(Operator::Semi),
            kw("then"), w_tok("b"), Token::Op(Operator::Semi),
            kw("fi"),
        ]).unwrap().unwrap();
        assert_eq!(multiline, singleline);
    }

    #[test]
    fn newline_after_then_is_skipped() {
        let seq = parse(vec![
            kw("if"), w_tok("a"), Token::Newline,
            kw("then"), Token::Newline,
            w_tok("b"), Token::Newline,
            kw("fi"),
        ]).unwrap().unwrap();
        let clause = first_if(&seq);
        assert_eq!(
            clause.then_body.first,
            Command::Pipeline(Pipeline { commands: vec![Command::Simple(plain("b", &[]))] })
        );
    }

    #[test]
    fn multiline_while_parses() {
        let seq = parse(vec![
            kw("while"), w_tok("a"), Token::Newline,
            kw("do"), w_tok("b"), Token::Newline,
            kw("done"),
        ]).unwrap().unwrap();
        let clause = first_while(&seq);
        assert!(!clause.until);
        assert_eq!(
            clause.body.first,
            Command::Pipeline(Pipeline { commands: vec![Command::Simple(plain("b", &[]))] })
        );
    }

    #[test]
    fn newline_separates_top_level_commands() {
        let seq = parse(vec![w_tok("a"), Token::Newline, w_tok("b")])
            .unwrap()
            .unwrap();
        assert_eq!(seq.rest.len(), 1);
        assert_eq!(seq.rest[0].0, Connector::Semi);
    }

    #[test]
    fn leading_newlines_are_skipped() {
        let seq = parse(vec![Token::Newline, Token::Newline, w_tok("a")])
            .unwrap()
            .unwrap();
        assert_eq!(seq.first, Command::Pipeline(Pipeline { commands: vec![Command::Simple(plain("a", &[]))] }));
    }

    #[test]
    fn all_newline_buffer_is_none() {
        assert_eq!(parse(vec![Token::Newline, Token::Newline]), Ok(None));
    }

    #[test]
    fn newline_after_pipe_continues_pipeline() {
        let seq = parse(vec![
            w_tok("a"), Token::Op(Operator::Pipe), Token::Newline, w_tok("b"),
        ]).unwrap().unwrap();
        let p = first_pipeline(&seq);
        assert_eq!(p.commands.len(), 2);
    }

    #[test]
    fn trailing_semicolon_then_newline_is_not_an_error() {
        let seq = parse(vec![w_tok("a"), Token::Op(Operator::Semi), Token::Newline])
            .unwrap()
            .unwrap();
        assert_eq!(seq.rest.len(), 0);
    }

    #[test]
    fn then_followed_by_semicolon_still_errors() {
        let result = parse(vec![
            kw("if"), w_tok("a"), Token::Op(Operator::Semi),
            kw("then"), Token::Op(Operator::Semi),
            w_tok("b"), Token::Op(Operator::Semi),
            kw("fi"),
        ]);
        assert_eq!(result, Err(ParseError::MissingCommand));
    }

    #[test]
    fn stray_word_after_compound_errors_without_panic() {
        let result = parse(vec![
            kw("if"), w_tok("a"), Token::Op(Operator::Semi),
            kw("then"), w_tok("b"), Token::Op(Operator::Semi),
            kw("fi"), w_tok("extra"),
        ]);
        assert_eq!(result, Err(ParseError::UnexpectedToken));
    }

    #[test]
    fn stray_close_paren_is_error() {
        assert_eq!(
            parse(vec![w_tok("echo"), Token::Op(Operator::RParen)]),
            Err(ParseError::UnexpectedToken)
        );
    }

    #[test]
    fn stray_open_paren_is_error() {
        // `echo(` with no matching `)` — looks like an incomplete function
        // definition, so FunctionBody is the right error (missing `)`).
        assert_eq!(
            parse(vec![w_tok("echo"), Token::Op(Operator::LParen)]),
            Err(ParseError::FunctionBody)
        );
    }

    #[test]
    fn stray_double_semi_is_error() {
        assert_eq!(
            parse(vec![w_tok("echo"), Token::Op(Operator::DoubleSemi)]),
            Err(ParseError::UnexpectedToken)
        );
    }

    #[test]
    fn stray_semi_amp_is_error() {
        assert_eq!(
            parse(vec![w_tok("echo"), Token::Op(Operator::SemiAmp)]),
            Err(ParseError::UnexpectedToken)
        );
    }

    #[test]
    fn stray_double_semi_amp_is_error() {
        assert_eq!(
            parse(vec![w_tok("echo"), Token::Op(Operator::DoubleSemiAmp)]),
            Err(ParseError::UnexpectedToken)
        );
    }

    #[test]
    fn if_with_no_body_at_end_of_input_is_unterminated() {
        let result = parse(vec![kw("if"), w_tok("a"), Token::Newline, kw("then")]);
        assert_eq!(result, Err(ParseError::UnterminatedIf));
    }

    #[test]
    fn while_with_no_body_at_end_of_input_is_unterminated() {
        let result = parse(vec![kw("while"), w_tok("a"), Token::Newline, kw("do")]);
        assert_eq!(result, Err(ParseError::UnterminatedLoop));
    }

    #[test]
    fn parse_brace_group_simple() {
        // { echo hi ; }
        let seq = parse(vec![
            kw("{"), w_tok("echo"), w_tok("hi"), Token::Op(Operator::Semi), kw("}"),
        ]).unwrap().unwrap();
        let body = match &seq.first {
            Command::BraceGroup(b) => b.as_ref(),
            other => panic!("expected a brace group, got {other:?}"),
        };
        assert_eq!(body.first, Command::Pipeline(Pipeline { commands: vec![Command::Simple(plain("echo", &["hi"]))] }));
    }

    #[test]
    fn parse_brace_group_multiline_matches_singleline() {
        let multi = parse(vec![
            kw("{"), Token::Newline, w_tok("echo"), Token::Newline, kw("}"),
        ]).unwrap().unwrap();
        let single = parse(vec![
            kw("{"), w_tok("echo"), Token::Op(Operator::Semi), kw("}"),
        ]).unwrap().unwrap();
        assert_eq!(multi, single);
    }

    #[test]
    fn parse_brace_group_unterminated() {
        // missing `}`
        assert_eq!(
            parse(vec![kw("{"), w_tok("echo"), Token::Op(Operator::Semi)]),
            Err(ParseError::UnterminatedBrace)
        );
    }

    fn first_function(seq: &Sequence) -> (&str, &Command) {
        match &seq.first {
            Command::FunctionDef { name, body } => (name.as_str(), body.as_ref()),
            other => panic!("expected a function def, got {other:?}"),
        }
    }

    #[test]
    fn parse_simple_function_def() {
        // foo() { echo hi; }
        let seq = parse(vec![
            w_tok("foo"), Token::Op(Operator::LParen), Token::Op(Operator::RParen),
            kw("{"), w_tok("echo"), w_tok("hi"), Token::Op(Operator::Semi), kw("}"),
        ]).unwrap().unwrap();
        let (name, body) = first_function(&seq);
        assert_eq!(name, "foo");
        assert!(matches!(body, Command::BraceGroup(_)));
    }

    #[test]
    fn parse_function_with_if_body() {
        // foo() if true; then echo; fi
        let seq = parse(vec![
            w_tok("foo"), Token::Op(Operator::LParen), Token::Op(Operator::RParen),
            kw("if"), w_tok("true"), Token::Op(Operator::Semi),
            kw("then"), w_tok("echo"), Token::Op(Operator::Semi),
            kw("fi"),
        ]).unwrap().unwrap();
        let (name, body) = first_function(&seq);
        assert_eq!(name, "foo");
        assert!(matches!(body, Command::If(_)));
    }

    #[test]
    fn parse_function_invalid_name() {
        // 1foo() { echo; }
        assert_eq!(
            parse(vec![
                w_tok("1foo"), Token::Op(Operator::LParen), Token::Op(Operator::RParen),
                kw("{"), w_tok("echo"), Token::Op(Operator::Semi), kw("}"),
            ]),
            Err(ParseError::FunctionName)
        );
    }

    #[test]
    fn parse_function_missing_close_paren() {
        // foo( { echo; }
        assert_eq!(
            parse(vec![
                w_tok("foo"), Token::Op(Operator::LParen),
                kw("{"), w_tok("echo"), Token::Op(Operator::Semi), kw("}"),
            ]),
            Err(ParseError::FunctionBody)
        );
    }

    #[test]
    fn parse_function_pipeline_body_errors() {
        // foo() echo hi  — body is a Pipeline, not a compound
        assert_eq!(
            parse(vec![
                w_tok("foo"), Token::Op(Operator::LParen), Token::Op(Operator::RParen),
                w_tok("echo"), w_tok("hi"),
            ]),
            Err(ParseError::FunctionBody)
        );
    }

    #[test]
    fn parse_function_def_without_body_is_unterminated() {
        // `foo()` then EOF — body not yet typed; classifier should treat as incomplete.
        assert_eq!(
            parse(vec![
                w_tok("foo"), Token::Op(Operator::LParen), Token::Op(Operator::RParen),
            ]),
            Err(ParseError::UnterminatedFunction)
        );
    }

    #[test]
    fn parse_function_nested_def_body_errors() {
        // foo() bar() { echo; }  — body must be a compound, not another function def
        assert_eq!(
            parse(vec![
                w_tok("foo"), Token::Op(Operator::LParen), Token::Op(Operator::RParen),
                w_tok("bar"), Token::Op(Operator::LParen), Token::Op(Operator::RParen),
                kw("{"), w_tok("echo"), Token::Op(Operator::Semi), kw("}"),
            ]),
            Err(ParseError::FunctionBody)
        );
    }

    #[test]
    fn stray_open_paren_after_pipeline_args_is_error() {
        // `echo hi (` — `(` after a pipeline arg goes via parse_pipeline,
        // not function-def detection (which only fires on the FIRST token).
        assert_eq!(
            parse(vec![w_tok("echo"), w_tok("hi"), Token::Op(Operator::LParen)]),
            Err(ParseError::UnexpectedToken)
        );
    }

    #[test]
    fn parse_function_def_followed_by_call() {
        // foo() { echo; } ; foo
        let seq = parse(vec![
            w_tok("foo"), Token::Op(Operator::LParen), Token::Op(Operator::RParen),
            kw("{"), w_tok("echo"), Token::Op(Operator::Semi), kw("}"),
            Token::Op(Operator::Semi),
            w_tok("foo"),
        ]).unwrap().unwrap();
        assert!(matches!(seq.first, Command::FunctionDef { .. }));
        assert_eq!(seq.rest.len(), 1);
        assert!(matches!(seq.rest[0].1, Command::Pipeline(_)));
    }

    #[test]
    fn parse_inline_assignments_collect_into_exec() {
        let tokens = crate::lexer::tokenize("A=1 B=2 cmd arg").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        let Command::Pipeline(p) = parsed.first else { panic!("expected Pipeline") };
        assert_eq!(p.commands.len(), 1);
        let Command::Simple(SimpleCommand::Exec(e)) = &p.commands[0] else {
            panic!("expected Simple(Exec), got {:?}", p.commands[0])
        };
        assert_eq!(e.inline_assignments.len(), 2);
        assert_eq!(e.inline_assignments[0].target.name(), "A");
        assert_eq!(e.inline_assignments[1].target.name(), "B");
        assert_eq!(e.program, ww("cmd"));
        assert_eq!(e.args, vec![ww("arg")]);
    }

    #[test]
    fn parse_assign_only_multiple_vars() {
        let tokens = crate::lexer::tokenize("A=1 B=2").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        let Command::Pipeline(p) = parsed.first else { panic!() };
        assert_eq!(p.commands.len(), 1);
        let Command::Simple(SimpleCommand::Assign(items)) = &p.commands[0] else {
            panic!("expected Simple(Assign(Vec)), got {:?}", p.commands[0])
        };
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].target.name(), "A");
        assert_eq!(items[1].target.name(), "B");
    }

    #[test]
    fn parse_assign_only_single_var_still_works() {
        let tokens = crate::lexer::tokenize("FOO=bar").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        let Command::Pipeline(p) = parsed.first else { panic!() };
        let Command::Simple(SimpleCommand::Assign(items)) = &p.commands[0] else { panic!() };
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].target.name(), "FOO");
    }

    #[test]
    fn parse_mid_command_assignment_word_stays_literal() {
        let tokens = crate::lexer::tokenize("cmd A=1").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        let Command::Pipeline(p) = parsed.first else { panic!() };
        let Command::Simple(SimpleCommand::Exec(e)) = &p.commands[0] else { panic!() };
        assert!(e.inline_assignments.is_empty());
        assert_eq!(e.program, ww("cmd"));
        assert_eq!(e.args, vec![ww("A=1")]);
    }

    #[test]
    fn parse_invalid_identifier_lhs_is_not_assignment() {
        let tokens = crate::lexer::tokenize("1FOO=bar cmd").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        let Command::Pipeline(p) = parsed.first else { panic!() };
        let Command::Simple(SimpleCommand::Exec(e)) = &p.commands[0] else { panic!() };
        assert!(e.inline_assignments.is_empty());
        assert_eq!(e.program, ww("1FOO=bar"));
        assert_eq!(e.args, vec![ww("cmd")]);
    }

    #[test]
    fn parse_assignment_before_compound_command_errors() {
        let tokens = crate::lexer::tokenize("A=1 if true; then echo hi; fi").unwrap();
        let err = parse(tokens).expect_err("expected parse error");
        // The keyword token (`if`, `then`, etc.) that follows the assignment
        // prefix is not a valid command position for a keyword, so the parser
        // returns UnexpectedKeyword rather than silently treating the compound
        // keyword as a literal argument.
        assert!(
            matches!(err, ParseError::UnexpectedKeyword(_)),
            "expected UnexpectedKeyword, got: {err:?}"
        );
    }

    // --- Here-document parser tests (v24) ---

    #[test]
    fn parse_heredoc_redirect_attaches_to_command() {
        // cmd <<EOF\nbody\nEOF → ExecCommand.stdin = Some(Redirect::Heredoc{...})
        let tokens = crate::lexer::tokenize("cmd <<EOF\nbody\nEOF\n").unwrap();
        let seq = parse(tokens).unwrap().unwrap();
        let stdin = exec_stdin(&seq);
        assert!(
            matches!(stdin, Some(Redirect::Heredoc { .. })),
            "expected Heredoc stdin, got: {stdin:?}"
        );
    }

    #[test]
    fn parse_heredoc_last_wins_over_file_redirect() {
        // cmd <file <<EOF\nbody\nEOF → stdin is the heredoc (last-wins)
        let tokens = crate::lexer::tokenize("cmd <file <<EOF\nbody\nEOF\n").unwrap();
        let seq = parse(tokens).unwrap().unwrap();
        let stdin = exec_stdin(&seq);
        assert!(
            matches!(stdin, Some(Redirect::Heredoc { .. })),
            "expected Heredoc to win over <file, got: {stdin:?}"
        );
    }

    #[test]
    fn parse_multiple_heredocs_keep_last() {
        // cmd <<A <<B\nbody_a\nA\nbody_b\nB → stdin is body_b (last heredoc wins)
        let tokens = crate::lexer::tokenize("cmd <<A <<B\nbody_a\nA\nbody_b\nB\n").unwrap();
        let seq = parse(tokens).unwrap().unwrap();
        let Command::Pipeline(p) = &seq.first else { panic!("expected Pipeline") };
        let Command::Simple(SimpleCommand::Exec(e)) = &p.commands[0] else { panic!("expected Simple(Exec)") };
        // The last heredoc (B's body) should be in stdin.
        if let Some(Redirect::Heredoc { body, .. }) = &e.stdin {
            // body_b → Literal{"body_b", quoted:false} + Literal{"\n", quoted:true}
            assert_eq!(body.0.len(), 2, "expected body_b parts, got: {:?}", body.0);
        } else {
            panic!("expected Heredoc stdin, got: {:?}", e.stdin);
        }
    }

    #[test]
    fn parse_heredoc_in_pipeline_stage() {
        // cat <<EOF | grep foo\nbody\nEOF → stage 0 has heredoc, stage 1 doesn't
        let tokens = crate::lexer::tokenize("cat <<EOF | grep foo\nbody\nEOF\n").unwrap();
        let seq = parse(tokens).unwrap().unwrap();
        let Command::Pipeline(p) = &seq.first else { panic!("expected Pipeline") };
        assert_eq!(p.commands.len(), 2, "expected 2 pipeline stages");
        let Command::Simple(SimpleCommand::Exec(stage0)) = &p.commands[0] else { panic!() };
        assert!(
            matches!(stage0.stdin, Some(Redirect::Heredoc { .. })),
            "stage 0 should have Heredoc stdin, got: {:?}", stage0.stdin
        );
        let Command::Simple(SimpleCommand::Exec(stage1)) = &p.commands[1] else { panic!() };
        assert!(
            stage1.stdin.is_none(),
            "stage 1 should have no stdin, got: {:?}", stage1.stdin
        );
    }

    // --- Pipeline compound-stage parser tests (v25 Task 2) ---

    #[test]
    fn parse_pipeline_with_if_stage() {
        let tokens = crate::lexer::tokenize("echo hi | if true; then cat; fi").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        let Command::Pipeline(p) = parsed.first else { panic!("expected Pipeline") };
        assert_eq!(p.commands.len(), 2);
        assert!(matches!(p.commands[0], Command::Simple(_)), "stage 0 should be Simple");
        assert!(matches!(p.commands[1], Command::If(_)), "stage 1 should be If");
    }

    #[test]
    fn parse_pipeline_with_function_call_stage() {
        // Function call appears as Simple at parse time — resolution is at runtime.
        let tokens = crate::lexer::tokenize("echo hi | myfunc").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        let Command::Pipeline(p) = parsed.first else { panic!("expected Pipeline") };
        assert_eq!(p.commands.len(), 2);
        assert!(matches!(&p.commands[1], Command::Simple(_)), "stage 1 should be Simple");
    }

    #[test]
    fn parse_pipeline_with_brace_group_stage() {
        let tokens = crate::lexer::tokenize("echo hi | { cat; }").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        let Command::Pipeline(p) = parsed.first else { panic!("expected Pipeline") };
        assert_eq!(p.commands.len(), 2);
        assert!(matches!(p.commands[1], Command::BraceGroup(_)), "stage 1 should be BraceGroup");
    }

    #[test]
    fn parse_pipeline_with_while_stage() {
        let tokens = crate::lexer::tokenize("seq 1 3 | while true; do cat; done").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        let Command::Pipeline(p) = parsed.first else { panic!("expected Pipeline") };
        assert!(matches!(p.commands[1], Command::While(_)), "stage 1 should be While");
    }

    #[test]
    fn parse_pipeline_with_for_stage() {
        let tokens = crate::lexer::tokenize("echo hi | for x in a b; do echo $x; done").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        let Command::Pipeline(p) = parsed.first else { panic!("expected Pipeline") };
        assert!(matches!(p.commands[1], Command::For(_)), "stage 1 should be For");
    }

    #[test]
    fn parse_pipeline_with_case_stage() {
        let tokens = crate::lexer::tokenize("echo a | case foo in a) :; ;; esac").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        let Command::Pipeline(p) = parsed.first else { panic!("expected Pipeline") };
        assert!(matches!(p.commands[1], Command::Case(_)), "stage 1 should be Case");
    }

    #[test]
    #[ignore = "TODO(M-11): subshell syntax `(list)` not yet lexed; nested multi-stage \
                pipeline-as-stage can't be triggered today. Guard exists for future correctness."]
    fn parse_pipeline_rejects_nested_multi_stage() {
        // When M-11 subshell syntax lands, `echo | (a | b)` should be a
        // parse error (Command::Pipeline with len > 1 as a stage is rejected).
        // For v25, `(a | b)` isn't lexed so this test is unreachable.
    }

    // --- Pipeline compound-as-FIRST-stage parser tests (v25 Task 2 fix) ---

    #[test]
    fn parse_pipeline_with_if_as_first_stage() {
        let tokens = crate::lexer::tokenize("if true; then echo hi; fi | cat").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        let Command::Pipeline(p) = parsed.first else {
            panic!("expected Pipeline, got {:?}", parsed.first)
        };
        assert_eq!(p.commands.len(), 2);
        assert!(matches!(p.commands[0], Command::If(_)), "stage 0 should be If");
        assert!(matches!(p.commands[1], Command::Simple(_)), "stage 1 should be Simple");
    }

    #[test]
    fn parse_pipeline_with_brace_group_as_first_stage() {
        let tokens = crate::lexer::tokenize("{ cat; } | grep foo").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        let Command::Pipeline(p) = parsed.first else {
            panic!("expected Pipeline, got {:?}", parsed.first)
        };
        assert_eq!(p.commands.len(), 2);
        assert!(matches!(p.commands[0], Command::BraceGroup(_)), "stage 0 should be BraceGroup");
        assert!(matches!(p.commands[1], Command::Simple(_)), "stage 1 should be Simple");
    }

    #[test]
    fn parse_pipeline_with_while_as_first_stage() {
        let tokens =
            crate::lexer::tokenize("while true; do echo loop; done | head -n 3").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        let Command::Pipeline(p) = parsed.first else {
            panic!("expected Pipeline, got {:?}", parsed.first)
        };
        assert_eq!(p.commands.len(), 2);
        assert!(matches!(p.commands[0], Command::While(_)), "stage 0 should be While");
    }

    #[test]
    fn parse_pipeline_with_for_as_first_stage() {
        let tokens = crate::lexer::tokenize("for x in a b; do echo $x; done | cat").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        let Command::Pipeline(p) = parsed.first else {
            panic!("expected Pipeline, got {:?}", parsed.first)
        };
        assert_eq!(p.commands.len(), 2);
        assert!(matches!(p.commands[0], Command::For(_)), "stage 0 should be For");
    }

    #[test]
    fn parse_pipeline_with_case_as_first_stage() {
        let tokens = crate::lexer::tokenize("case foo in foo) echo yes;; esac | cat").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        let Command::Pipeline(p) = parsed.first else {
            panic!("expected Pipeline, got {:?}", parsed.first)
        };
        assert_eq!(p.commands.len(), 2);
        assert!(matches!(p.commands[0], Command::Case(_)), "stage 0 should be Case");
    }

    // ---- v27 here-string parser tests ------------------------------------------

    #[test]
    fn parse_here_string_attaches_to_stdin() {
        let tokens = crate::lexer::tokenize("cat <<< hi").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        let Command::Pipeline(p) = parsed.first else { panic!() };
        let Command::Simple(SimpleCommand::Exec(e)) = &p.commands[0] else { panic!() };
        assert!(matches!(&e.stdin, Some(Redirect::HereString(_))));
    }

    #[test]
    fn parse_here_string_last_wins_over_file() {
        let tokens = crate::lexer::tokenize("cat <file <<< hi").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        let Command::Pipeline(p) = parsed.first else { panic!() };
        let Command::Simple(SimpleCommand::Exec(e)) = &p.commands[0] else { panic!() };
        assert!(matches!(&e.stdin, Some(Redirect::HereString(_))));
    }

    #[test]
    fn parse_here_string_last_wins_over_heredoc() {
        let tokens = crate::lexer::tokenize("cat <<EOF <<< override\nignored\nEOF\n").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        let Command::Pipeline(p) = parsed.first else { panic!() };
        let Command::Simple(SimpleCommand::Exec(e)) = &p.commands[0] else { panic!() };
        assert!(matches!(&e.stdin, Some(Redirect::HereString(_))));
    }

    #[test]
    fn parse_here_string_missing_word_errors() {
        let tokens = crate::lexer::tokenize("cat <<<").unwrap();
        let result = parse(tokens);
        assert!(result.is_err(), "expected parse error, got {:?}", result);
    }

    #[test]
    fn parse_here_string_in_pipeline_stage() {
        let tokens = crate::lexer::tokenize("cat <<< x | grep x").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        let Command::Pipeline(p) = parsed.first else { panic!() };
        assert_eq!(p.commands.len(), 2);
        let Command::Simple(SimpleCommand::Exec(stage0)) = &p.commands[0] else { panic!() };
        assert!(matches!(&stage0.stdin, Some(Redirect::HereString(_))));
        let Command::Simple(SimpleCommand::Exec(stage1)) = &p.commands[1] else { panic!() };
        assert!(stage1.stdin.is_none());
    }

    // ── v28 subshell parser tests ────────────────────────────────────────────

    #[test]
    fn parse_subshell_simple() {
        let tokens = crate::lexer::tokenize("(echo hi)").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        let Command::Subshell { body } = parsed.first else {
            panic!("expected Subshell, got {:?}", parsed.first)
        };
        // body is a Sequence with one command (the `echo hi` pipeline).
        assert_eq!(body.rest.len(), 0);
    }

    #[test]
    fn parse_subshell_with_sequence() {
        let tokens = crate::lexer::tokenize("(cmd1; cmd2)").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        let Command::Subshell { body } = parsed.first else { panic!() };
        assert_eq!(body.rest.len(), 1); // first + 1 more = 2 commands
    }

    #[test]
    fn parse_subshell_with_and_or() {
        let tokens = crate::lexer::tokenize("(true && echo hi)").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        let Command::Subshell { body } = parsed.first else { panic!() };
        // Body's first command + the And-connected rest.
        assert!(body.rest.iter().any(|(conn, _)| matches!(conn, Connector::And)));
    }

    #[test]
    fn parse_subshell_nested() {
        // v78: `((cmd))` (no whitespace) now lexes as `Token::ArithBlock`,
        // matching bash. Use `( (cmd) )` with whitespace between the
        // outer and inner `(` to keep the nested-subshell semantics.
        let tokens = crate::lexer::tokenize("( (echo hi) )").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        let Command::Subshell { body: outer } = parsed.first else { panic!() };
        let Command::Subshell { .. } = outer.first else {
            panic!("expected nested Subshell, got {:?}", outer.first)
        };
    }

    #[test]
    fn parse_subshell_empty_errors() {
        let tokens = crate::lexer::tokenize("()").unwrap();
        let err = parse(tokens).expect_err("expected ParseError::EmptySubshell");
        assert!(matches!(err, ParseError::EmptySubshell), "got {:?}", err);
    }

    #[test]
    fn parse_subshell_unterminated_errors() {
        let tokens = crate::lexer::tokenize("(echo hi").unwrap();
        let err = parse(tokens).expect_err("expected ParseError::UnterminatedSubshell");
        assert!(matches!(err, ParseError::UnterminatedSubshell), "got {:?}", err);
    }

    #[test]
    fn parse_subshell_as_pipeline_first_stage() {
        let tokens = crate::lexer::tokenize("(echo hi) | cat").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        let Command::Pipeline(p) = parsed.first else { panic!() };
        assert_eq!(p.commands.len(), 2);
        assert!(matches!(p.commands[0], Command::Subshell { .. }));
    }

    #[test]
    fn parse_subshell_as_pipeline_later_stage() {
        let tokens = crate::lexer::tokenize("echo hi | (cat)").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        let Command::Pipeline(p) = parsed.first else { panic!() };
        assert_eq!(p.commands.len(), 2);
        assert!(matches!(p.commands[1], Command::Subshell { .. }));
    }

    #[test]
    fn parse_subshell_does_not_conflict_with_function_def() {
        // `f() (echo hi)` is a function definition whose body is a subshell.
        // The parser must dispatch on IDENT + `(` for function-def, not LParen-alone-at-start.
        let tokens = crate::lexer::tokenize("f() (echo hi)").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        let Command::FunctionDef { name, body } = parsed.first else {
            panic!("expected FunctionDef, got {:?}", parsed.first)
        };
        assert_eq!(name, "f");
        assert!(
            matches!(*body, Command::Subshell { .. }),
            "function body should be a Subshell, got {:?}",
            body
        );
    }

    #[test]
    fn parse_subshell_with_inner_background() {
        // (cmd &) — backgrounded INSIDE the subshell body. Spec says this should
        // work. Was a Task 1 gap: parse_subshell_sequence rejected Operator::Background.
        let tokens = crate::lexer::tokenize("(echo hi &)").unwrap();
        let parsed = parse(tokens).expect("parse should succeed").expect("non-empty");
        let Command::Subshell { body } = parsed.first else {
            panic!("expected Subshell, got {:?}", parsed.first)
        };
        // The body's pipeline is backgrounded inside the subshell.
        assert!(body.background, "inner pipeline should be marked backgrounded");
    }

    #[test]
    fn parse_subshell_with_inner_background_and_sequence() {
        // (cmd1 &; cmd2) — first command backgrounded, then second runs.
        // Bash allows this; both cmd1 (bg) and cmd2 (fg) run inside the subshell.
        let tokens = crate::lexer::tokenize("(echo a &; echo b)").unwrap();
        let result = parse(tokens);
        // At minimum, this should not be a parse error. The exact AST shape
        // depends on whether the implementer collapsed `&;` into one separator
        // or treats them distinctly. Lenient assertion: parse succeeded.
        assert!(result.is_ok(), "got: {:?}", result);
    }

    #[test]
    fn parse_dup_stdout_from_fd2() {
        let tokens = crate::lexer::tokenize("cmd >&2").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        let Command::Pipeline(p) = parsed.first else { panic!() };
        let Command::Simple(SimpleCommand::Exec(e)) = &p.commands[0] else { panic!() };
        let Some(Redirect::Dup { fd, source }) = &e.stdout else { panic!("got {:?}", e.stdout) };
        assert_eq!(*fd, 1);
        // source Word's first part should be Literal "2".
        assert!(matches!(&source.0[0], WordPart::Literal { text, .. } if text == "2"));
    }

    #[test]
    fn parse_dup_stderr_from_fd1() {
        let tokens = crate::lexer::tokenize("cmd 2>&1").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        let Command::Pipeline(p) = parsed.first else { panic!() };
        let Command::Simple(SimpleCommand::Exec(e)) = &p.commands[0] else { panic!() };
        let Some(Redirect::Dup { fd, source }) = &e.stderr else { panic!("got {:?}", e.stderr) };
        assert_eq!(*fd, 2);
        assert!(matches!(&source.0[0], WordPart::Literal { text, .. } if text == "1"));
    }

    #[test]
    fn parse_and_redir_out_desugars() {
        let tokens = crate::lexer::tokenize("cmd &>file").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        let Command::Pipeline(p) = parsed.first else { panic!() };
        let Command::Simple(SimpleCommand::Exec(e)) = &p.commands[0] else { panic!() };
        // stdout = Truncate(file)
        let Some(Redirect::Truncate(file)) = &e.stdout else { panic!("got {:?}", e.stdout) };
        assert!(matches!(&file.0[0], WordPart::Literal { text, .. } if text == "file"));
        // stderr = Dup{fd:2, source:"1"}
        let Some(Redirect::Dup { fd, source }) = &e.stderr else { panic!("got {:?}", e.stderr) };
        assert_eq!(*fd, 2);
        assert!(matches!(&source.0[0], WordPart::Literal { text, .. } if text == "1"));
    }

    #[test]
    fn parse_and_redir_append_desugars() {
        let tokens = crate::lexer::tokenize("cmd &>>file").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        let Command::Pipeline(p) = parsed.first else { panic!() };
        let Command::Simple(SimpleCommand::Exec(e)) = &p.commands[0] else { panic!() };
        let Some(Redirect::Append(_)) = &e.stdout else { panic!("got {:?}", e.stdout) };
        let Some(Redirect::Dup { fd, .. }) = &e.stderr else { panic!() };
        assert_eq!(*fd, 2);
    }

    #[test]
    fn parse_dup_with_var_target() {
        // 2>&$FD — source is a Word with a Var part, not a literal.
        let tokens = crate::lexer::tokenize("cmd 2>&$FD").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        let Command::Pipeline(p) = parsed.first else { panic!() };
        let Command::Simple(SimpleCommand::Exec(e)) = &p.commands[0] else { panic!() };
        let Some(Redirect::Dup { source, .. }) = &e.stderr else { panic!() };
        assert!(source.0.iter().any(|p| matches!(p, WordPart::Var { name, .. } if name == "FD")));
    }

    #[test]
    fn parse_dup_in_pipeline_stage() {
        let tokens = crate::lexer::tokenize("cmd 2>&1 | grep").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        let Command::Pipeline(p) = parsed.first else { panic!() };
        assert_eq!(p.commands.len(), 2);
        let Command::Simple(SimpleCommand::Exec(stage0)) = &p.commands[0] else { panic!() };
        assert!(matches!(&stage0.stderr, Some(Redirect::Dup { .. })));
        let Command::Simple(SimpleCommand::Exec(stage1)) = &p.commands[1] else { panic!() };
        assert!(stage1.stderr.is_none());
    }

    #[test]
    fn parse_combined_dup_and_file_redirect() {
        let tokens = crate::lexer::tokenize("cmd >file 2>&1").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        let Command::Pipeline(p) = parsed.first else { panic!() };
        let Command::Simple(SimpleCommand::Exec(e)) = &p.commands[0] else { panic!() };
        assert!(matches!(&e.stdout, Some(Redirect::Truncate(_))));
        assert!(matches!(&e.stderr, Some(Redirect::Dup { fd: 2, .. })));
    }

    // ──────────────────────────────────────────────────────────────
    // [[ ]] parser tests (v30)
    // ──────────────────────────────────────────────────────────────

    #[test]
    fn parse_dbracket_string_eq_literal() {
        let tokens = crate::lexer::tokenize("[[ a == b ]]").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        let Command::DoubleBracket { expr, .. } = parsed.first else {
            panic!("expected DoubleBracket, got {:?}", parsed.first)
        };
        let TestExpr::Binary { op, .. } = &*expr else { panic!("expected Binary, got {:?}", expr) };
        assert!(matches!(op, TestBinaryOp::StringEq));
    }

    #[test]
    fn parse_dbracket_string_eq_single_equals() {
        // `[[ a = b ]]` is bash alias for `[[ a == b ]]`.
        let tokens = crate::lexer::tokenize("[[ a = b ]]").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty");
        let Command::DoubleBracket { expr, .. } = parsed.first else { panic!() };
        let TestExpr::Binary { op, .. } = &*expr else { panic!() };
        assert!(matches!(op, TestBinaryOp::StringEq));
    }

    #[test]
    fn parse_dbracket_string_ne() {
        let tokens = crate::lexer::tokenize("[[ x != y ]]").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty");
        let Command::DoubleBracket { expr, .. } = parsed.first else { panic!() };
        let TestExpr::Binary { op, .. } = &*expr else { panic!() };
        assert!(matches!(op, TestBinaryOp::StringNe));
    }

    #[test]
    fn parse_dbracket_string_eq_pattern() {
        // RHS is an unquoted glob — still parses as StringEq (runtime interprets glob).
        let tokens = crate::lexer::tokenize("[[ $f == *.txt ]]").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty");
        let Command::DoubleBracket { expr, .. } = parsed.first else { panic!() };
        let TestExpr::Binary { op, .. } = &*expr else { panic!() };
        assert!(matches!(op, TestBinaryOp::StringEq));
    }

    #[test]
    fn parse_dbracket_regex() {
        let tokens = crate::lexer::tokenize("[[ s =~ ^foo$ ]]").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty");
        let Command::DoubleBracket { expr, .. } = parsed.first else { panic!() };
        assert!(matches!(&*expr, TestExpr::Regex { .. }));
    }

    #[test]
    fn parse_dbracket_integer_compare() {
        let tokens = crate::lexer::tokenize("[[ 5 -eq 5 ]]").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty");
        let Command::DoubleBracket { expr, .. } = parsed.first else { panic!() };
        let TestExpr::Binary { op, .. } = &*expr else { panic!() };
        assert!(matches!(op, TestBinaryOp::IntEq));
    }

    #[test]
    fn parse_dbracket_integer_gt() {
        let tokens = crate::lexer::tokenize("[[ 5 -gt 3 ]]").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty");
        let Command::DoubleBracket { expr, .. } = parsed.first else { panic!() };
        let TestExpr::Binary { op, .. } = &*expr else { panic!() };
        assert!(matches!(op, TestBinaryOp::IntGt));
    }

    #[test]
    fn parse_dbracket_string_lex_lt() {
        // `<` is lexed as RedirIn by the lexer; parser handles it as StringLt inside [[ ]].
        let tokens = crate::lexer::tokenize("[[ a < b ]]").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty");
        let Command::DoubleBracket { expr, .. } = parsed.first else { panic!() };
        let TestExpr::Binary { op, .. } = &*expr else { panic!() };
        assert!(matches!(op, TestBinaryOp::StringLt));
    }

    #[test]
    fn parse_dbracket_unary_file() {
        let tokens = crate::lexer::tokenize("[[ -f /tmp ]]").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty");
        let Command::DoubleBracket { expr, .. } = parsed.first else { panic!() };
        let TestExpr::Unary { op, .. } = &*expr else { panic!("expected Unary, got {:?}", expr) };
        assert!(matches!(op, TestUnaryOp::IsRegFile));
    }

    #[test]
    fn parse_dbracket_unary_string_empty() {
        let tokens = crate::lexer::tokenize("[[ -z foo ]]").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty");
        let Command::DoubleBracket { expr, .. } = parsed.first else { panic!() };
        let TestExpr::Unary { op, .. } = &*expr else { panic!() };
        assert!(matches!(op, TestUnaryOp::StringEmpty));
    }

    #[test]
    fn parse_dbracket_not() {
        let tokens = crate::lexer::tokenize("[[ ! -f /tmp/x ]]").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty");
        let Command::DoubleBracket { expr, .. } = parsed.first else { panic!() };
        let TestExpr::Not(inner) = &*expr else { panic!("expected Not, got {:?}", expr) };
        assert!(matches!(&**inner, TestExpr::Unary { .. }));
    }

    #[test]
    fn parse_dbracket_and() {
        let tokens = crate::lexer::tokenize("[[ -f a && -r a ]]").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty");
        let Command::DoubleBracket { expr, .. } = parsed.first else { panic!() };
        assert!(matches!(&*expr, TestExpr::And(_, _)));
    }

    #[test]
    fn parse_dbracket_or() {
        let tokens = crate::lexer::tokenize("[[ x == a || x == b ]]").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty");
        let Command::DoubleBracket { expr, .. } = parsed.first else { panic!() };
        assert!(matches!(&*expr, TestExpr::Or(_, _)));
    }

    #[test]
    fn parse_dbracket_grouped() {
        let tokens = crate::lexer::tokenize("[[ ( a == a || b == c ) && d == d ]]").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty");
        let Command::DoubleBracket { expr, .. } = parsed.first else { panic!() };
        // Top-level should be And with the first operand being the grouped Or.
        let TestExpr::And(lhs, _) = &*expr else { panic!("expected And, got {:?}", expr) };
        assert!(matches!(&**lhs, TestExpr::Or(_, _)));
    }

    #[test]
    fn parse_dbracket_empty_errors() {
        let tokens = crate::lexer::tokenize("[[ ]]").unwrap();
        assert!(parse(tokens).is_err(), "expected parse error for empty [[]]");
    }

    #[test]
    fn parse_dbracket_unterminated_errors() {
        let tokens = crate::lexer::tokenize("[[ x == y").unwrap();
        assert!(parse(tokens).is_err(), "expected parse error for unterminated [[");
    }

    #[test]
    fn parse_dbracket_with_inline_assignment() {
        let tokens = crate::lexer::tokenize("FOO=hi [[ $FOO == hi ]]").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        let Command::DoubleBracket { expr: _, inline_assignments } = parsed.first else {
            panic!("expected DoubleBracket, got {:?}", parsed.first)
        };
        assert_eq!(inline_assignments.len(), 1);
        assert_eq!(inline_assignments[0].target.name(), "FOO");
    }

    // -----------------------------------------------------------------------
    // Regression tests: multi-assign speculative-peel iterator-drain bug
    // -----------------------------------------------------------------------

    #[test]
    fn multi_assign_then_pipeline_then_semi_then_next_cmd_preserves_next() {
        // Regression: v30's speculative-peel for FOO=hi [[ ]] introduced an
        // iterator-drain bug in the multi-assign (A=1 B=2 ...) fallback path.
        // The iter.by_ref() drain caused parse_pipeline_with_first to swallow
        // all remaining tokens into a sub-iter; anything after the pipeline
        // terminator (`;`) was silently dropped when sub went out of scope.
        // After the fix, the outer iter stays intact so parse_sequence picks
        // up `foo` after the semicolon.
        let tokens = crate::lexer::tokenize("A=1 B=2 cmd; foo").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty");
        assert_eq!(
            parsed.rest.len(), 1,
            "expected `; foo` to survive parse; got {:?}", parsed
        );
        // The first pipeline should carry both inline assignments.
        let Command::Pipeline(ref p) = parsed.first else {
            panic!("expected Pipeline, got {:?}", parsed.first)
        };
        let Command::Simple(SimpleCommand::Exec(ref e)) = p.commands[0] else {
            panic!("expected Simple(Exec), got {:?}", p.commands[0])
        };
        assert_eq!(e.inline_assignments.len(), 2);
        assert_eq!(e.inline_assignments[0].target.name(), "A");
        assert_eq!(e.inline_assignments[1].target.name(), "B");
    }

    #[test]
    fn multi_assign_then_pipeline_then_and_then_next_cmd_preserves_next() {
        // Same shape but with `&&` connector — verifies the fix is not
        // specific to the `;` terminator.
        let tokens = crate::lexer::tokenize("A=1 B=2 cmd && other").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty");
        assert_eq!(
            parsed.rest.len(), 1,
            "expected `&& other` to survive parse; got {:?}", parsed
        );
    }

    #[test]
    fn function_keyword_form_brace_body() {
        let tokens = crate::lexer::tokenize("function f { echo hi; }").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        match parsed.first {
            Command::FunctionDef { name, body } => {
                assert_eq!(name, "f");
                assert!(matches!(*body, Command::BraceGroup(_)),
                        "expected brace body, got {body:?}");
            }
            other => panic!("expected FunctionDef, got {other:?}"),
        }
    }

    #[test]
    fn function_keyword_form_with_parens() {
        let tokens = crate::lexer::tokenize("function f() { :; }").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        assert!(matches!(parsed.first, Command::FunctionDef { ref name, .. } if name == "f"),
                "got {:?}", parsed.first);
    }

    #[test]
    fn function_keyword_form_with_parens_and_spaces() {
        let tokens = crate::lexer::tokenize("function f  (  )  { :; }").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        assert!(matches!(parsed.first, Command::FunctionDef { ref name, .. } if name == "f"),
                "got {:?}", parsed.first);
    }

    #[test]
    fn function_keyword_form_subshell_body() {
        let tokens = crate::lexer::tokenize("function f() ( echo nested )").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        match parsed.first {
            Command::FunctionDef { name, body } => {
                assert_eq!(name, "f");
                assert!(matches!(*body, Command::Subshell { .. }),
                        "expected subshell body, got {body:?}");
            }
            other => panic!("expected FunctionDef, got {other:?}"),
        }
    }

    #[test]
    fn function_keyword_form_compound_body_no_braces() {
        // `function f if true; then :; fi` — no braces; if-statement body.
        let tokens = crate::lexer::tokenize("function f if true; then :; fi").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        match parsed.first {
            Command::FunctionDef { name, body } => {
                assert_eq!(name, "f");
                assert!(matches!(*body, Command::If(_)),
                        "expected if body, got {body:?}");
            }
            other => panic!("expected FunctionDef, got {other:?}"),
        }
    }

    #[test]
    fn function_keyword_form_newline_before_body() {
        let tokens = crate::lexer::tokenize("function f\n{\n:;\n}").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        assert!(matches!(parsed.first, Command::FunctionDef { ref name, .. } if name == "f"),
                "got {:?}", parsed.first);
    }

    #[test]
    fn function_keyword_no_name_errors() {
        let tokens = crate::lexer::tokenize("function { :; }").unwrap();
        let err = parse(tokens).expect_err("should error");
        assert!(matches!(err, ParseError::FunctionName), "got {err:?}");
    }

    #[test]
    fn function_keyword_keyword_name_errors() {
        // Names that are themselves reserved keywords are rejected.
        let tokens = crate::lexer::tokenize("function if { :; }").unwrap();
        let err = parse(tokens).expect_err("should error");
        assert!(matches!(err, ParseError::FunctionName), "got {err:?}");
    }

    #[test]
    fn function_keyword_missing_body_errors() {
        let tokens = crate::lexer::tokenize("function f").unwrap();
        let err = parse(tokens).expect_err("should error");
        assert!(matches!(err, ParseError::UnterminatedFunction), "got {err:?}");
    }

    #[test]
    fn function_keyword_bad_body_errors() {
        // `function f echo hi` — body is a pipeline, not a compound.
        let tokens = crate::lexer::tokenize("function f echo hi").unwrap();
        let err = parse(tokens).expect_err("should error");
        assert!(matches!(err, ParseError::FunctionBody), "got {err:?}");
    }

    #[test]
    fn function_keyword_unbalanced_parens_errors() {
        let tokens = crate::lexer::tokenize("function f ( { :; }").unwrap();
        let err = parse(tokens).expect_err("should error");
        assert!(matches!(err, ParseError::FunctionBody), "got {err:?}");
    }

    #[test]
    fn function_posix_form_double_bracket_body() {
        // Regression: POSIX form should ALSO accept [[ ]] body
        // (closes a pre-existing gap; was rejected pre-v77).
        let tokens = crate::lexer::tokenize("f() [[ -e /dev/null ]]").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        match parsed.first {
            Command::FunctionDef { name, body } => {
                assert_eq!(name, "f");
                assert!(matches!(*body, Command::DoubleBracket { .. }),
                        "expected DoubleBracket body, got {body:?}");
            }
            other => panic!("expected FunctionDef, got {other:?}"),
        }
    }

    #[test]
    fn function_body_can_be_standalone_arith() {
        let tokens = crate::lexer::tokenize("f() ((1+2))").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        match parsed.first {
            Command::FunctionDef { name, body } => {
                assert_eq!(name, "f");
                assert!(matches!(*body, Command::Arith(_)),
                        "expected arith body, got {body:?}");
            }
            other => panic!("expected FunctionDef, got {other:?}"),
        }
    }

    #[test]
    fn function_body_can_be_arith_for_loop() {
        let tokens = crate::lexer::tokenize("f() for ((i=0;i<3;i++)) do :; done").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        match parsed.first {
            Command::FunctionDef { name, body } => {
                assert_eq!(name, "f");
                assert!(matches!(*body, Command::ArithFor(_)),
                        "expected arith-for body, got {body:?}");
            }
            other => panic!("expected FunctionDef, got {other:?}"),
        }
    }

    #[test]
    fn function_keyword_form_double_bracket_body() {
        // Wrapped in a brace group: the body IS BraceGroup; inside the
        // brace group is a DoubleBracket. Verify the function parses.
        let tokens = crate::lexer::tokenize("function f { [[ -e /dev/null ]]; }").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        assert!(matches!(parsed.first, Command::FunctionDef { ref name, .. } if name == "f"),
                "got {:?}", parsed.first);
    }

    #[test]
    fn function_as_assignment_var_still_works() {
        // `function=value` must still parse as a normal assignment,
        // NOT trigger the function-keyword path. The lexer's
        // assignment-prefix detection fires before keyword
        // classification, so the token reaches the parser as a
        // Word with an AssignPrefix part.
        let tokens = crate::lexer::tokenize("function=value").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        // At minimum it must NOT parse as a FunctionDef.
        assert!(
            !matches!(parsed.first, Command::FunctionDef { .. }),
            "function=value must not parse as a FunctionDef: {:?}",
            parsed.first,
        );
        // Specifically, it should parse as a Pipeline → Simple →
        // Assign with target Bare("function").
        let Command::Pipeline(p) = &parsed.first else {
            panic!("expected Pipeline, got {:?}", parsed.first);
        };
        let Command::Simple(SimpleCommand::Assign(assigns)) = &p.commands[0] else {
            panic!("expected Simple/Assign, got {:?}", p.commands[0]);
        };
        assert_eq!(assigns.len(), 1);
        match &assigns[0].target {
            AssignTarget::Bare(name) => assert_eq!(name, "function"),
            other => panic!("expected Bare target, got {other:?}"),
        }
    }

    #[test]
    fn function_in_arg_position_still_works() {
        // `echo function` — `function` is in argument position, not
        // command position, so it must NOT trigger the keyword arm.
        let tokens = crate::lexer::tokenize("echo function").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        assert!(
            !matches!(parsed.first, Command::FunctionDef { .. }),
            "echo function must not parse as a FunctionDef: {:?}",
            parsed.first,
        );
    }

    // ----- v78: arith block + C-style for-loop parser tests -----

    #[test]
    fn parse_standalone_arith_command() {
        let tokens = crate::lexer::tokenize("((1+2))").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        assert!(matches!(parsed.first, Command::Arith(_)),
                "got {:?}", parsed.first);
    }

    #[test]
    fn parse_arith_for_full_header() {
        let tokens = crate::lexer::tokenize("for ((i=0;i<10;i++)) do :; done").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        match parsed.first {
            Command::ArithFor(clause) => {
                assert!(clause.init.is_some());
                assert!(clause.cond.is_some());
                assert!(clause.step.is_some());
            }
            other => panic!("expected ArithFor, got {other:?}"),
        }
    }

    #[test]
    fn parse_arith_for_all_empty_sections() {
        let tokens = crate::lexer::tokenize("for ((;;)) do break; done").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        match parsed.first {
            Command::ArithFor(clause) => {
                assert!(clause.init.is_none());
                assert!(clause.cond.is_none());
                assert!(clause.step.is_none());
            }
            other => panic!("expected ArithFor, got {other:?}"),
        }
    }

    #[test]
    fn parse_arith_for_only_init() {
        let tokens = crate::lexer::tokenize("for ((i=0;;)) do break; done").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        match parsed.first {
            Command::ArithFor(clause) => {
                assert!(clause.init.is_some());
                assert!(clause.cond.is_none());
                assert!(clause.step.is_none());
            }
            other => panic!("expected ArithFor, got {other:?}"),
        }
    }

    #[test]
    fn parse_arith_for_only_cond() {
        let tokens = crate::lexer::tokenize("for ((;i<10;)) do break; done").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        match parsed.first {
            Command::ArithFor(clause) => {
                assert!(clause.init.is_none());
                assert!(clause.cond.is_some());
                assert!(clause.step.is_none());
            }
            other => panic!("expected ArithFor, got {other:?}"),
        }
    }

    #[test]
    fn parse_arith_for_only_step() {
        let tokens = crate::lexer::tokenize("for ((;;i++)) do break; done").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        match parsed.first {
            Command::ArithFor(clause) => {
                assert!(clause.init.is_none());
                assert!(clause.cond.is_none());
                assert!(clause.step.is_some());
            }
            other => panic!("expected ArithFor, got {other:?}"),
        }
    }

    #[test]
    fn parse_arith_for_newline_before_do() {
        let tokens = crate::lexer::tokenize("for ((;;))\ndo break; done").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        assert!(matches!(parsed.first, Command::ArithFor(_)),
                "got {:?}", parsed.first);
    }

    #[test]
    fn parse_arith_for_semicolon_before_do() {
        let tokens = crate::lexer::tokenize("for ((;;)); do break; done").unwrap();
        let parsed = parse(tokens).unwrap().expect("non-empty parse");
        assert!(matches!(parsed.first, Command::ArithFor(_)),
                "got {:?}", parsed.first);
    }

    #[test]
    fn parse_arith_for_two_sections_errors() {
        // `for ((i=0;i<10))` — only one `;`, two sections.
        let tokens = crate::lexer::tokenize("for ((i=0;i<10)) do :; done").unwrap();
        let err = parse(tokens).expect_err("should error");
        assert!(matches!(err, ParseError::ArithForHeader(_)), "got {err:?}");
    }

    #[test]
    fn parse_arith_for_bad_arith_in_section_errors() {
        // `for ((i=+;;))` — `i=+` is not a valid arith expression.
        let tokens = crate::lexer::tokenize("for ((i=+;;)) do :; done").unwrap();
        let err = parse(tokens).expect_err("should error");
        assert!(matches!(err, ParseError::ArithBlock(_)), "got {err:?}");
    }

    #[test]
    fn parse_arith_for_missing_do_errors() {
        let tokens = crate::lexer::tokenize("for ((;;)) :; done").unwrap();
        let err = parse(tokens).expect_err("should error");
        assert!(matches!(err, ParseError::UnterminatedLoop), "got {err:?}");
    }

    #[test]
    fn parse_standalone_arith_with_bad_expr_errors() {
        let tokens = crate::lexer::tokenize("((1++))").unwrap();
        let err = parse(tokens).expect_err("should error");
        assert!(matches!(err, ParseError::ArithBlock(_)), "got {err:?}");
    }
}
