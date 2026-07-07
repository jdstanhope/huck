use crate::lexer::{Lexer, Operator, TokenKind, Word, WordPart};

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
    Select,
    Coproc,
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
            Keyword::Select => "select",
            Keyword::Coproc => "coproc",
        }
    }
}

/// Returns the keyword a token represents, or `None`. A token is a
/// keyword only when it is a `Word` of exactly one part — an *unquoted*
/// `Literal` whose text equals the keyword.
fn keyword_of(token: &TokenKind) -> Option<Keyword> {
    let TokenKind::Word(Word(parts)) = token else { return None };
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
        "select" => Some(Keyword::Select),
        "coproc" => Some(Keyword::Coproc),
        _ => None,
    }
}

/// If `word` is an assignment-shaped Word (either an `AssignPrefix`-led
/// structured form for `name+=…` / `name[…]=…` / `name[…]+=…`, or a
/// leading `Literal` of the form `NAME=…` for bare scalar `name=…`),
/// returns the structured `Assignment`. The value is moved (not cloned)
/// from the input parts. Otherwise returns `Err(word)` handing the
/// original back unchanged.
pub fn try_split_assignment(
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

/// Peek variant of [`try_split_assignment`] that does not consume the
/// input. Returns `Some(Assignment)` if `word` has assignment shape
/// (the relevant parts are cloned), else `None`.
///
/// Use this when you have a `&Word` reference and need to detect
/// assignment shape without taking ownership. For the consuming form
/// (which avoids the clone when you can hand over the word), see
/// [`try_split_assignment`].
pub fn try_split_assignment_ref(word: &crate::lexer::Word) -> Option<Assignment> {
    try_split_assignment(word.clone()).ok()
}

/// Returns `true` if `w` looks like an assignment word without
/// consuming or cloning it. Mirrors the shape check in `try_split_assignment`
/// so the caller can decide whether to take ownership before calling the
/// real splitter. Detects both the structured `AssignPrefix` form and the
/// legacy bare `Literal("NAME=…")` form.
pub fn is_assignment_word(w: &crate::lexer::Word) -> bool {
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
pub(crate) fn lit_word(s: &str) -> Word {
    Word(vec![WordPart::Literal { text: s.to_string(), quoted: false }])
}

fn finalize_stage(
    program: crate::lexer::Word,
    args: Vec<crate::lexer::Word>,
    redirects: Vec<Redirection>,
    line: u32,
) -> SimpleCommand {

    // Walk [program, args…] peeling leading assignments. Stops at the first
    // word that isn't a valid `NAME=value` (per try_split_assignment).
    // We peek by reference first (cheap) and only take ownership when the
    // word is confirmed to be assignment-shaped, avoiding a deep clone on
    // every non-assignment first word.
    let mut inline: Vec<Assignment> = Vec::new();
    let mut word_iter = std::iter::once(program).chain(args).peekable();
    while let Some(w) = word_iter.peek() {
        if !is_assignment_word(w) {
            break;
        }
        let owned = word_iter.next().expect("just peeked Some");
        match try_split_assignment(owned) {
            Ok(a) => inline.push(a),
            Err(_) => unreachable!("is_assignment_word confirmed assignment shape"),
        }
    }
    let remaining: Vec<Word> = word_iter.collect();

    if remaining.is_empty() && redirects.is_empty() && !inline.is_empty() {
        return SimpleCommand::Assign(inline, line);
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
            redirects,
            line,
        });
    }
    let mut remaining = remaining.into_iter();
    let program = remaining.next().expect("non-empty after peel");
    let args: Vec<Word> = remaining.collect();
    SimpleCommand::Exec(ExecCommand {
        inline_assignments: inline,
        program,
        args,
        redirects,
        line,
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
    /// `>|file` — force truncate, overriding `noclobber` (`set -C`).
    Clobber(Word),
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

/// One redirection, applied in source order. Replaces the old fixed
/// stdin/stdout/stderr slots so fd>2 and source-ordering (`2>&1 >file`) work.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Redirection {
    pub fd: RedirFd,
    pub op: RedirOp,
}

/// The target file descriptor of a redirection.
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum RedirFd {
    /// No explicit prefix: resolves to 0 for input ops, 1 for output ops.
    Default,
    /// `3>` / `2<&` — an explicit numeric fd.
    Number(u16),
    /// `{name}>` — allocate a free fd (>=10) at apply time and assign $name.
    Var(String),
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum FileMode {
    ReadOnly,   // <     default fd 0
    Truncate,   // >     default fd 1
    Append,     // >>    default fd 1
    Clobber,    // >|    default fd 1
    ReadWrite,  // <>    default fd 0
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum RedirOp {
    File { mode: FileMode, target: Word },
    /// `>&w` (output=true) / `<&w` (output=false). `source` is an fd-number
    /// word; a `-` source is normalized to `Close` by the parser.
    Dup { source: Word, output: bool },
    /// `N>&-` / `N<&-`.
    Close,
    Heredoc { body: Word, expand: bool, strip_tabs: bool },
    HereString(Word),
}

impl RedirOp {
    /// The fd this op targets when `RedirFd::Default` (no explicit prefix).
    pub fn default_fd(&self) -> u16 {
        match self {
            RedirOp::File { mode: FileMode::ReadOnly | FileMode::ReadWrite, .. } => 0,
            RedirOp::File { .. } => 1,
            RedirOp::Dup { output: true, .. } => 1,
            RedirOp::Dup { output: false, .. } => 0,
            RedirOp::Close => 0,
            RedirOp::Heredoc { .. } | RedirOp::HereString(_) => 0,
        }
    }
}

impl Redirection {
    /// The concrete numeric target fd for non-`Var` redirections (`Var` is
    /// resolved at apply time). Used by the ordered applier and the
    /// pipeline-stage slot fast-path.
    pub fn target_fd(&self) -> Option<u16> {
        match &self.fd {
            RedirFd::Default => Some(self.op.default_fd()),
            RedirFd::Number(n) => Some(*n),
            RedirFd::Var(_) => None,
        }
    }
}

/// PERMANENT internal helper (v156): collapse the ordered redirect list to the
/// fixed 0/1/2 slots for the paths that still consume 0/1/2 separately rather
/// than via the unified ordered applier — the PIPELINE-STAGE executor
/// (`run_multi_stage` / `run_background_sequence` / `spawn_external_with_fds`)
/// and AST→source regeneration (`generate.rs`). fd>2 targets, `RedirFd::Var`,
/// `RedirOp::Close`, `<&` (Dup output:false), and ReadWrite are DROPPED here
/// (the pipeline path picks the fd>2/dup-in/close/ReadWrite set up additively via
/// `slots_for_simple_path` + `build_child_extra_ops`). LAST-WINS per slot.
///
/// RESIDUAL LIMITATION (v156 task 7 fallback): because this is last-wins and the
/// extra set is applied additively, SOURCE ORDERING between a 0/1/2 redirect and
/// another redirect (`2>&1 >file` / `>file 3>&1`) is NOT preserved for pipeline
/// stages, and an fd>2 heredoc on a pipeline-stage external is dropped. The
/// single-command builtin and external paths do NOT use this helper anymore —
/// they apply `cmd.redirects` in source order (so L-08 is fixed there).
pub fn slots_for_simple_path(redirs: &[Redirection]) -> (Option<Redirect>, Option<Redirect>, Option<Redirect>) {
    let (mut sin, mut sout, mut serr) = (None, None, None);
    for r in redirs {
        let Some(fd) = r.target_fd() else { continue };
        let legacy = match &r.op {
            RedirOp::File { mode: FileMode::ReadOnly, target } => Some(Redirect::Read(target.clone())),
            RedirOp::File { mode: FileMode::Truncate, target } => Some(Redirect::Truncate(target.clone())),
            RedirOp::File { mode: FileMode::Append, target } => Some(Redirect::Append(target.clone())),
            RedirOp::File { mode: FileMode::Clobber, target } => Some(Redirect::Clobber(target.clone())),
            RedirOp::File { mode: FileMode::ReadWrite, .. } => None,
            RedirOp::Dup { source, output: true } => Some(Redirect::Dup { fd: fd as i32, source: source.clone() }),
            RedirOp::Dup { output: false, .. } | RedirOp::Close => None,
            RedirOp::Heredoc { body, expand, strip_tabs } => Some(Redirect::Heredoc { body: body.clone(), expand: *expand, strip_tabs: *strip_tabs }),
            RedirOp::HereString(w) => Some(Redirect::HereString(w.clone())),
        };
        // Only fill a slot when the op direction matches the fd:
        //   stdin  (0): input ops only (ReadOnly, Heredoc, HereString)
        //   stdout (1) / stderr (2): output ops only (Truncate/Append/Clobber, Dup{output:true})
        // Cross-type combos (e.g. Read→fd1, Truncate→fd0) are dropped — they
        // would cause resolve()'s unreachable!() assertions to fire. They
        // become fully functional when the applier tasks are migrated.
        match fd {
            0 => {
                if matches!(&r.op,
                    RedirOp::File { mode: FileMode::ReadOnly, .. }
                    | RedirOp::Heredoc { .. }
                    | RedirOp::HereString(_))
                {
                    sin = legacy;
                }
            }
            1 => {
                if matches!(&r.op,
                    RedirOp::File { mode: FileMode::Truncate | FileMode::Append | FileMode::Clobber, .. }
                    | RedirOp::Dup { output: true, .. })
                {
                    sout = legacy;
                }
            }
            2 => {
                if matches!(&r.op,
                    RedirOp::File { mode: FileMode::Truncate | FileMode::Append | FileMode::Clobber, .. }
                    | RedirOp::Dup { output: true, .. })
                {
                    serr = legacy;
                }
            }
            _ => {}
        }
    }
    (sin, sout, serr)
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
    /// Redirections in source order (`>a 2>&1` differs from `2>&1 >a`).
    /// Replaces the old fixed stdin/stdout/stderr slots (v156). Single-command
    /// builtin/external/exec paths apply this list in source order; the
    /// pipeline-stage path still collapses it to 0/1/2 via `slots_for_simple_path`
    /// (last-wins — source order not preserved there).
    pub redirects: Vec<Redirection>,
    /// 1-based source line of the command's first token (0 = unknown).
    /// Set at parse time; the executor uses it for $LINENO.
    pub line: u32,
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

    /// The 0/1/2 redirect slots derived from `redirects` for the PIPELINE-STAGE
    /// fast-path (v156). The single-command builtin/external paths no longer use
    /// these — they apply `redirects` in source order. Last-wins, source-order
    /// NOT preserved (see `slots_for_simple_path`).
    pub fn slot_stdin(&self) -> Option<Redirect> {
        slots_for_simple_path(&self.redirects).0
    }
    pub fn slot_stdout(&self) -> Option<Redirect> {
        slots_for_simple_path(&self.redirects).1
    }
    pub fn slot_stderr(&self) -> Option<Redirect> {
        slots_for_simple_path(&self.redirects).2
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum SimpleCommand {
    /// `A=1 B=2 …` with no following command — every assignment
    /// persists in the shell. Single-element vec is the v22-style
    /// single-assignment case. The `u32` is the 1-based source line (0 = unknown).
    Assign(Vec<Assignment>, u32),
    Exec(ExecCommand),
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Pipeline {
    /// True if the pipeline is prefixed with `!` (negate the exit status).
    pub negate: bool,
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
    /// `&` as a list separator: backgrounds the preceding and-or group and
    /// continues the list (v98).
    Amp,
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
    VarSet,          // -v  (variable is set)
    /// `[[ -o NAME ]]` — true iff the `set -o` option NAME is enabled.
    OptEnabled,      // -o
    IsFifo,          // -p
    IsSocket,        // -S
    IsBlockDev,      // -b
    IsCharDev,       // -c
    OwnedByEuid,     // -O
    OwnedByEgid,     // -G
    NewerThanRead,   // -N
    IsSticky,        // -k
    IsSetuid,        // -u
    IsSetgid,        // -g
    IsTerminal,      // -t
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
    NewerThan, // -nt
    OlderThan, // -ot
    SameFile,  // -ef
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
#[non_exhaustive]
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
    Arith(crate::lexer::Word),
    /// NEW (v78): C-style `for ((init; cond; step)) do BODY done`.
    ArithFor(Box<ArithForClause>),
    /// NEW (v81): `select NAME [in WORDS]; do BODY; done`.
    Select(Box<SelectClause>),
    /// A compound command with trailing redirections applied to its whole
    /// execution: `{ …; } >f`, `while … done <<EOF`, etc. (v97)
    Redirected {
        inner: Box<Command>,
        /// Trailing redirects in source order (v156), applied by the executor's
        /// ordered `with_redirect_scope` applier (source order preserved).
        redirects: Vec<Redirection>,
    },
    /// `coproc [NAME] command` (v157). `name` is "COPROC" when anonymous. The
    /// body runs asynchronously with its stdin/stdout wired to two pipes the
    /// shell holds as NAME[0] (read) / NAME[1] (write).
    Coproc { name: String, body: Box<Command> },
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
    /// The raw loop variable name; identifier-validated at runtime (`run_for`).
    pub var: String,
    /// The unexpanded `in` word list. Empty for the no-`in` form.
    pub words: Vec<Word>,
    /// True when an explicit `in WORDS` clause was present. The no-`in`
    /// form (`has_in == false`) iterates the positional params (Task 3).
    pub has_in: bool,
    /// The do…done body.
    pub body: Sequence,
}

/// NEW (v81): a `select NAME [in WORDS]; do BODY; done` clause.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct SelectClause {
    /// Loop variable name — a validated identifier.
    pub var: String,
    /// None => no `in` clause (iterate the positional params "$@").
    /// Some(words) => explicit `in WORDS` (Some(vec![]) = empty `in`).
    pub words: Option<Vec<Word>>,
    pub body: Sequence,
}

/// NEW (v78): a C-style `for ((init; cond; step)) do BODY done` clause.
/// Each header section is optional; an empty cond is treated as always-true.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ArithForClause {
    pub init: Option<crate::lexer::Word>,
    pub cond: Option<crate::lexer::Word>,
    pub step: Option<crate::lexer::Word>,
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

#[derive(Debug, PartialEq, Eq, Clone)]
#[non_exhaustive]
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
    /// NEW (v239): a lex error surfaced while the parser pulled tokens live
    Lex(Box<crate::lexer::LexError>),
    /// A nested expansion the parser-driven path does not handle yet
    /// (`$(…)` / `$((…))` / backtick inside a `${…}` operand). v241 boundary.
    UnsupportedExpansion,
    /// A command-level construct the parser-driven flat command parser does not
    /// model yet (subshell, arith command, compound command, heredoc, …). v242 boundary.
    UnsupportedCommand,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&crate::errors::parse_error_message_impl(self))
    }
}

impl std::error::Error for ParseError {}

impl From<crate::lexer::LexError> for ParseError {
    fn from(e: crate::lexer::LexError) -> Self {
        ParseError::Lex(Box::new(e))
    }
}

fn parse_cursor(iter: &mut Lexer) -> Result<Option<Sequence>, ParseError> {
    skip_newlines(iter)?;
    if iter.peek_kind()?.is_none() {
        return Ok(None);
    }
    let seq = parse_sequence(iter, &[])?;
    if iter.peek_kind()?.is_some() {
        // A stray terminator (`;;`/`;&`/`;;&`) left after the top-level
        // sequence — `parse_sequence` peek-breaks on those (see below).
        return Err(ParseError::UnexpectedToken);
    }
    Ok(Some(seq))
}

pub fn parse(iter: &mut Lexer) -> Result<Option<Sequence>, ParseError> {
    parse_cursor(iter)
}

/// Parse ONE top-level command unit from a pre-tokenized stream, stopping at
/// (and consuming) the next top-level newline or EOF. Skips leading blank-line
/// newlines. Returns `Ok(None)` when only newlines / EOF remain. Used by the
/// non-interactive script reader.
pub fn parse_one_unit(iter: &mut Lexer) -> Result<Option<Sequence>, ParseError> {
    while matches!(iter.peek_kind()?, Some(TokenKind::Newline)) {
        iter.next_kind()?;
    }
    if iter.peek_kind()?.is_none() {
        return Ok(None);
    }
    let seq = parse_sequence_opts(iter, &[], true)?;
    Ok(Some(seq))
}

/// Parses one sequence ELEMENT: a command, plus — if a `|` immediately
/// follows — the rest of the pipeline (the command is the first stage).
/// `parse_command` already consumes a pipeline when the first stage is a
/// SIMPLE command; this helper adds the wrap for a COMPOUND/subshell first
/// stage (which `parse_command` returns without checking for a trailing `|`).
/// Returns `raw` unchanged when no `|` follows — so non-pipeline elements are
/// byte-identical to before.
fn parse_command_then_pipeline(
    iter: &mut Lexer,
) -> Result<Command, ParseError> {
    let raw = parse_command(iter)?;
    if matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::Pipe))) {
        // The bang wrapper in `parse_command` may have produced a 1-element
        // negated Pipeline around a compound/subshell first stage (e.g.
        // `! ( false ) | cat`). Since `!` negates the WHOLE pipeline, hoist
        // its `negate` flag to the outer pipeline and unwrap the inner stage.
        let (negate, first_stage) = match raw {
            Command::Pipeline(p) if p.negate && p.commands.len() == 1 => {
                (true, p.commands.into_iter().next().unwrap())
            }
            other => (false, other),
        };
        let mut stages = vec![first_stage];
        iter.next_kind()?; // consume `|`
        skip_newlines(iter)?;
        let mut more = true;
        while more {
            let (cmd, next_pipe) = parse_next_stage(iter)?;
            stages.push(cmd);
            if next_pipe {
                // Simple stage already consumed its own `|`; continue.
            } else if matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::Pipe))) {
                iter.next_kind()?;
                skip_newlines(iter)?;
            } else {
                more = false;
            }
        }
        Ok(Command::Pipeline(Pipeline { negate, commands: stages }))
    } else {
        Ok(raw)
    }
}

/// Parses commands joined by `;` / `&&` / `||` (and an optional trailing
/// `&` at top level only). Stops — without consuming — when the next
/// token is a keyword in `stop_at`. `stop_at` is empty only at the top
/// level; a non-empty `stop_at` means we are inside a compound command.
fn parse_sequence(
    iter: &mut Lexer,
    stop_at: &[Keyword],
) -> Result<Sequence, ParseError> {
    parse_sequence_opts(iter, stop_at, false)
}

/// The shared body of [`parse_sequence`]. When `stop_at_top_newline` is set, a
/// top-level `TokenKind::Newline` terminates the command unit (used by
/// [`parse_one_unit`] for the non-interactive script reader); otherwise a
/// top-level newline is a Semi-like continue connector (the historical
/// behavior — all existing callers go through the `false` wrapper).
fn parse_sequence_opts(
    iter: &mut Lexer,
    stop_at: &[Keyword],
    stop_at_top_newline: bool,
) -> Result<Sequence, ParseError> {
    let first = parse_command_then_pipeline(iter)?;
    let mut rest = Vec::new();
    let mut background = false;

    loop {
        match iter.peek_kind()? {
            None => break,
            Some(TokenKind::Op(
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
        let token = iter.next_kind()?.unwrap();
        match token {
            TokenKind::Op(Operator::Background) => {
                // `&` is a list separator (v98): it backgrounds the preceding
                // and-or group. Skip any trailing Newline tokens that the
                // heredoc body collector emits at the end of the input (e.g.
                // when the buffer is "cat <<EOF &\nbody\nEOF" the lexer emits a
                // Newline after the heredoc body). Newlines after `&` are not
                // meaningful as separators.
                skip_newlines(iter)?;
                match iter.peek_kind()? {
                    // Nothing meaningful follows -> trailing `&`: background the
                    // whole (final group of the) sequence.
                    None => {
                        background = true;
                        break;
                    }
                    // A `stop_at` keyword (e.g. `done`/`fi`/`then`) terminates a
                    // compound body -> trailing `&` for the last group.
                    Some(tok)
                        if keyword_of(tok)
                            .map(|k| stop_at.contains(&k))
                            .unwrap_or(false) =>
                    {
                        background = true;
                        break;
                    }
                    // A case-clause terminator ends the clause body -> trailing
                    // `&` for the last group.
                    Some(TokenKind::Op(
                        Operator::DoubleSemi | Operator::SemiAmp | Operator::DoubleSemiAmp,
                    )) => {
                        background = true;
                        break;
                    }
                    // Another `&` (`cmd & &`) has no preceding command -> invalid.
                    Some(TokenKind::Op(Operator::Background)) => {
                        return Err(ParseError::UnexpectedBackground);
                    }
                    // A command follows -> `&` is a separator: background the
                    // preceding group, continue the list.
                    Some(_) => {
                        rest.push((Connector::Amp, parse_command_then_pipeline(iter)?));
                    }
                }
            }
            TokenKind::Op(Operator::Semi) | TokenKind::Newline => {
                if stop_at_top_newline && matches!(token, TokenKind::Newline) {
                    // Unit mode: a top-level newline ends the command unit
                    // (already consumed by iter.next_kind()? above). `;`, `&&`,
                    // `||`, `&`, and compound-internal newlines are unaffected.
                    break;
                }
                skip_newlines(iter)?;
                match iter.peek_kind()? {
                    None => break,
                    Some(TokenKind::Op(
                        Operator::DoubleSemi | Operator::SemiAmp | Operator::DoubleSemiAmp,
                    )) => break,
                    Some(tok) => {
                        if keyword_of(tok).map(|k| stop_at.contains(&k)).unwrap_or(false) {
                            break;
                        }
                    }
                }
                rest.push((Connector::Semi, parse_command_then_pipeline(iter)?));
            }
            TokenKind::Op(Operator::And) => {
                skip_newlines(iter)?;
                rest.push((Connector::And, parse_command_then_pipeline(iter)?));
            }
            TokenKind::Op(Operator::Or) => {
                skip_newlines(iter)?;
                rest.push((Connector::Or, parse_command_then_pipeline(iter)?));
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

/// Pipeline negation wrapper: consumes a run of standalone `!` words at command
/// position (odd count → negate), parses the inner command, and attaches the
/// negation flag. Detected only here (command position), so `[ ! -e x ]` keeps
/// `!` as an argument of `[`.
fn parse_command(
    iter: &mut Lexer,
) -> Result<Command, ParseError> {
    let mut bangs = 0usize;
    while iter.peek_kind()?.map(is_bang_word).unwrap_or(false) {
        iter.next_kind()?; // consume `!`
        bangs += 1;
    }
    if bangs == 0 {
        return parse_command_inner(iter);
    }
    let inner = parse_command_inner(iter)?;
    let negate = bangs % 2 == 1;
    Ok(match inner {
        Command::Pipeline(mut p) => {
            p.negate = negate;
            Command::Pipeline(p)
        }
        // A compound (if/while/for/case/select/{}/subshell/[[ ]]): wrap in a
        // 1-element pipeline so the negation applies to its status.
        other => Command::Pipeline(Pipeline { negate, commands: vec![other] }),
    })
}

/// Parses a single sequence element: a subshell, compound command, or pipeline.
fn parse_command_inner(
    iter: &mut Lexer,
) -> Result<Command, ParseError> {
    skip_newlines(iter)?;
    iter.expand_command_alias()?;

    // Standalone arith block: `((expr))` at command position.
    if matches!(iter.peek_kind()?, Some(TokenKind::ArithBlock(..))) {
        let Some(TokenKind::ArithBlock(text, opts)) = iter.next_kind()? else {
            unreachable!("matches! guard above guarantees ArithBlock")
        };
        let body = crate::lexer::arith_string_to_word(&text, opts)
            .map_err(|e| ParseError::ArithBlock(crate::lex_error_message(&e)))?;
        return maybe_wrap_redirects(Command::Arith(body), iter);
    }

    match iter.peek_kind()?.and_then(keyword_of) {
        Some(Keyword::If) => maybe_wrap_redirects(Command::If(Box::new(parse_if(iter)?)), iter),
        Some(Keyword::While) | Some(Keyword::Until) => {
            maybe_wrap_redirects(Command::While(Box::new(parse_while(iter)?)), iter)
        }
        Some(Keyword::For) => {
            let cmd = parse_for_command(iter)?;
            maybe_wrap_redirects(cmd, iter)
        }
        Some(Keyword::Select) => {
            iter.next_kind()?; // consume `select`
            let cmd = parse_select_command(iter)?;
            maybe_wrap_redirects(cmd, iter)
        }
        Some(Keyword::Case) => maybe_wrap_redirects(Command::Case(Box::new(parse_case(iter)?)), iter),
        Some(Keyword::LBrace) => {
            maybe_wrap_redirects(Command::BraceGroup(Box::new(parse_brace_group(iter)?)), iter)
        }
        Some(Keyword::DoubleBracketOpen) => {
            let cmd = parse_double_bracket(iter)?;
            maybe_wrap_redirects(cmd, iter)
        }
        Some(Keyword::Function) => parse_function_keyword_def(iter),
        Some(Keyword::Coproc) => {
            iter.next_kind()?; // consume `coproc`
            let cmd = parse_coproc_command(iter)?;
            maybe_wrap_redirects(cmd, iter)
        }
        Some(other) => Err(ParseError::UnexpectedKeyword(other.name().to_string())),
        None => {
            // Check for bare `(` at command-start → subshell `(list)`.
            // This check MUST come BEFORE the IDENT+LParen function-def path,
            // but in practice they don't overlap: function-def starts with
            // a Word token, not an Op(LParen). Still, the comment clarifies
            // intent.
            if matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::LParen))) {
                let cmd = parse_subshell(iter)?;
                return maybe_wrap_redirects(cmd, iter);
            }
            // Non-keyword, non-LParen: may be a function definition
            // `name() compound`, or a plain pipeline. Need two-token lookahead.
            if matches!(iter.peek_kind()?, Some(TokenKind::Word(_))) {
                // Capture the line of the first word BEFORE consuming it.
                let word_line = iter.current_line()?;
                // Consume the word; peek for `(`.
                let Some(TokenKind::Word(w)) = iter.next_kind()? else { unreachable!() };
                if matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::LParen))) {
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
                    while let Some(TokenKind::Word(nw)) = iter.peek_kind()? {
                        if !is_assignment_word(nw) {
                            break;
                        }
                        let Some(TokenKind::Word(nw)) = iter.next_kind()? else { unreachable!() };
                        let nw_clone = nw.clone();
                        match try_split_assignment(nw) {
                            Ok(a) => assigns.push(a),
                            Err(_) => unreachable!("is_assignment_word confirmed"),
                        }
                        extra_clones.push(nw_clone);
                    }
                    // If `[[` follows, dispatch as DoubleBracket with assigns.
                    if iter.peek_kind()?.and_then(keyword_of) == Some(Keyword::DoubleBracketOpen) {
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
                            parse_pipeline_with_first(Some(w_clone), vec![], iter, word_line)?
                        ));
                    }
                    // Multi-assign case (e.g. `A=1 B=2 cmd`): extra assignment
                    // words were consumed from `iter` during speculative peeling.
                    // Re-inject them as prefix_tokens so that `iter` is NOT
                    // drained — the outer parse_sequence needs `iter` intact to
                    // pick up any trailing `;`/`&&`/`||` separators after this
                    // pipeline ends.
                    let prefix: Vec<TokenKind> =
                        extra_clones.into_iter().map(TokenKind::Word).collect();
                    return Ok(Command::Pipeline(
                        parse_pipeline_with_first(Some(w_clone), prefix, iter, word_line)?
                    ));
                }
                // Not a function def — pipeline with `w` as the first word.
                Ok(Command::Pipeline(parse_pipeline_with_first(Some(w), vec![], iter, word_line)?))
            } else {
                Ok(Command::Pipeline(parse_pipeline(iter)?))
            }
        }
    }
}

/// True if `body` is one of the compound-command shapes that's
/// allowed as a function body in both POSIX `name() body` form and
/// the bash `function NAME body` form.
pub(crate) fn is_function_body_shape(body: &Command) -> bool {
    // A redirected compound (`{ … } >file`) is a valid function body — the
    // redirect attaches to the definition and is applied (with call-time
    // filename expansion) on every call. The Redirected body is stored and
    // re-executed per call, giving bash's semantics with no executor change
    // (M-09b). A Redirected wrapping a non-compound is still rejected.
    if let Command::Redirected { inner, .. } = body {
        return is_function_body_shape(inner);
    }
    matches!(
        body,
        Command::If(_)
            | Command::While(_)
            | Command::For(_)
            | Command::Select(_)
            | Command::Case(_)
            | Command::BraceGroup(_)
            | Command::Subshell { .. }
            | Command::DoubleBracket { .. }
            | Command::Arith(_)
            | Command::ArithFor(_)
    )
}

/// After the function name and optional `()` have been consumed, skips
/// newlines, guards against end-of-input, parses the body command,
/// validates its shape, and returns the complete `FunctionDef` command.
/// Shared by `parse_function_def` and `parse_function_keyword_def`.
fn finish_function_body(name: String, iter: &mut Lexer) -> Result<Command, ParseError> {
    skip_newlines(iter)?;
    if iter.peek_kind()?.is_none() {
        return Err(ParseError::UnterminatedFunction);
    }
    let body = parse_command(iter)?;
    if !is_function_body_shape(&body) {
        return Err(ParseError::FunctionBody);
    }
    Ok(Command::FunctionDef { name, body: Box::new(body) })
}

/// Parses `name() compound-command`. The caller has consumed the name
/// (`name_word`) and verified the next token is `(`.
fn parse_function_def(
    name_word: Word,
    iter: &mut Lexer,
) -> Result<Command, ParseError> {
    let name = valid_function_name_text(&name_word).ok_or(ParseError::FunctionName)?;
    // Consume `(`.
    iter.next_kind()?;
    // Expect `)`.
    match iter.next_kind()? {
        Some(TokenKind::Op(Operator::RParen)) => {}
        _ => return Err(ParseError::FunctionBody),
    }
    finish_function_body(name, iter)
}

/// Parses `function NAME [()] compound-command`. The caller has
/// verified the next token is the `function` keyword (still in the
/// iterator). Consumes the keyword, the name, optional `()`, and
/// the compound body.
fn parse_function_keyword_def(
    iter: &mut Lexer,
) -> Result<Command, ParseError> {
    // Consume `function` keyword.
    iter.next_kind()?;

    // Read the name. Must be a Word that's a valid POSIX identifier
    // and not a reserved keyword.
    let name_word = match iter.next_kind()? {
        Some(TokenKind::Word(w)) => w,
        _ => return Err(ParseError::FunctionName),
    };
    let name = valid_function_name_text(&name_word).ok_or(ParseError::FunctionName)?;

    // Optionally consume `()`.
    if matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::LParen))) {
        iter.next_kind()?; // consume `(`
        match iter.next_kind()? {
            Some(TokenKind::Op(Operator::RParen)) => {}
            _ => return Err(ParseError::FunctionBody),
        }
    }

    // Allow newlines between name (or `()`) and the body.
    finish_function_body(name, iter)
}

/// Consumes a run of `Newline` tokens. Newlines are soft separators —
/// they are skipped wherever a command is expected but not yet present.
fn skip_newlines(iter: &mut Lexer) -> Result<(), ParseError> {
    while matches!(iter.peek_kind()?, Some(TokenKind::Newline)) {
        iter.next_kind()?;
    }
    Ok(())
}

/// Consumes one token and checks it is the expected keyword.
fn expect_keyword(
    iter: &mut Lexer,
    expected: Keyword,
    on_missing: ParseError,
) -> Result<(), ParseError> {
    match iter.next_kind()? {
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
fn parse_compound_section(
    iter: &mut Lexer,
    stop_at: &[Keyword],
    unterminated: ParseError,
) -> Result<Sequence, ParseError> {
    match parse_sequence(iter, stop_at) {
        Err(ParseError::MissingCommand) if iter.peek_kind()?.is_none() => Err(unterminated),
        other => other,
    }
}

/// Parses `if LIST; then LIST; [elif LIST; then LIST;]... [else LIST;] fi`.
fn parse_if(
    iter: &mut Lexer,
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
    while iter.peek_kind()?.and_then(keyword_of) == Some(Keyword::Elif) {
        iter.next_kind()?; // consume `elif`
        let condition = parse_compound_section(iter, &[Keyword::Then], ParseError::UnterminatedIf)?;
        expect_keyword(iter, Keyword::Then, ParseError::UnterminatedIf)?;
        let body = parse_compound_section(
            iter,
            &[Keyword::Elif, Keyword::Else, Keyword::Fi],
            ParseError::UnterminatedIf,
        )?;
        elif_branches.push(ElifBranch { condition, body });
    }

    let else_body = if iter.peek_kind()?.and_then(keyword_of) == Some(Keyword::Else) {
        iter.next_kind()?; // consume `else`
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
pub(crate) fn valid_identifier_text(word: &Word) -> Option<String> {
    if word.0.len() != 1 {
        return None;
    }
    let WordPart::Literal { text, quoted: false } = &word.0[0] else {
        return None;
    };
    // Reject reserved keywords. Build a single-Word token to reuse keyword_of.
    let tok = TokenKind::Word(Word(vec![WordPart::Literal {
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

/// Returns the function name if `word` is a single, unquoted, non-empty
/// `Literal` that is not a reserved keyword. Unlike `valid_identifier_text`, this
/// does NOT restrict the character set: bash accepts almost any single word as a
/// function name (`foo-bar`, `a.b`, `2foo`, …), and the lexer already guarantees a
/// single `Literal` has no metacharacters or whitespace, so the trailing `()`
/// (or the `function` keyword) — not the name's spelling — is what makes it a
/// definition. The keyword guard keeps `if() { :; }` a syntax error like bash.
pub(crate) fn valid_function_name_text(word: &Word) -> Option<String> {
    if word.0.len() != 1 {
        return None;
    }
    let WordPart::Literal { text, quoted: false } = &word.0[0] else {
        return None;
    };
    if text.is_empty() {
        return None;
    }
    let tok = TokenKind::Word(Word(vec![WordPart::Literal {
        text: text.clone(),
        quoted: false,
    }]));
    if keyword_of(&tok).is_some() {
        return None;
    }
    Some(text.clone())
}

/// Returns the raw loop-variable name if `token` is a single, unquoted,
/// non-empty `Literal` `Word`. bash accepts ANY word as the `for` variable at
/// parse time (including reserved words like `if`, and non-identifiers like
/// `a-b`); the identifier rule is enforced at RUNTIME (`run_for`). So this does
/// NOT apply the keyword / charset checks of `valid_identifier_text`.
fn for_variable_name(token: &TokenKind) -> Option<String> {
    let TokenKind::Word(w) = token else { return None };
    if w.0.len() != 1 {
        return None;
    }
    let WordPart::Literal { text, quoted: false } = &w.0[0] else {
        return None;
    };
    if text.is_empty() {
        return None;
    }
    Some(text.clone())
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
    Option<crate::lexer::Word>,
    Option<crate::lexer::Word>,
    Option<crate::lexer::Word>,
);

/// Splits an arith-for header into three optional arith expressions.
/// Empty sections (e.g., the cond in `((;;))`) yield `None`. Returns
/// `ArithForHeader` if the header doesn't split into exactly 3 sections.
fn parse_arith_for_header(
    text: &str,
    opts: crate::lexer::LexerOptions,
) -> Result<ArithForHeaderTriple, ParseError> {
    let sections = split_top_level_semi(text);
    if sections.len() != 3 {
        return Err(ParseError::ArithForHeader(format!(
            "expected 3 sections separated by `;`, got {}",
            sections.len()
        )));
    }
    let parse_section = |s: &str| -> Result<Option<crate::lexer::Word>, ParseError> {
        let trimmed = s.trim();
        if trimmed.is_empty() {
            Ok(None)
        } else {
            crate::lexer::arith_string_to_word(trimmed, opts)
                .map(Some)
                .map_err(|e| ParseError::ArithBlock(crate::lex_error_message(&e)))
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
/// `TokenKind::ArithBlock`. This function consumes the ArithBlock, the
/// separators before `do`, the `do` keyword, the body, and the `done`
/// keyword.
fn parse_arith_for_clause(
    iter: &mut Lexer,
) -> Result<ArithForClause, ParseError> {
    let (header_text, arith_opts) = match iter.next_kind()? {
        Some(TokenKind::ArithBlock(text, opts)) => (text, opts),
        _ => unreachable!("caller verified peek"),
    };
    let (init, cond, step) = parse_arith_for_header(&header_text, arith_opts)?;

    // Skip `;` and newline separators between the header and `do`.
    while matches!(
        iter.peek_kind()?,
        Some(TokenKind::Op(Operator::Semi)) | Some(TokenKind::Newline)
    ) {
        iter.next_kind()?;
    }
    expect_keyword(iter, Keyword::Do, ParseError::UnterminatedLoop)?;

    let body = parse_compound_section(iter, &[Keyword::Done], ParseError::UnterminatedLoop)?;
    expect_keyword(iter, Keyword::Done, ParseError::UnterminatedLoop)?;

    Ok(ArithForClause { init, cond, step, body })
}

/// Dispatches `for` to either the POSIX form (`for VAR in WORDS; do ...`)
/// or the bash arith form (`for ((init;cond;step)) do ...`). Consumes
/// the `for` keyword itself.
fn parse_for_command(
    iter: &mut Lexer,
) -> Result<Command, ParseError> {
    expect_keyword(iter, Keyword::For, ParseError::UnterminatedLoop)?;

    // Peek the next token to choose the variant. Skip newlines first so
    // `for\n((...))` works the same as `for ((...))`.
    while matches!(iter.peek_kind()?, Some(TokenKind::Newline)) {
        iter.next_kind()?;
    }

    if matches!(iter.peek_kind()?, Some(TokenKind::ArithBlock(..))) {
        return Ok(Command::ArithFor(Box::new(parse_arith_for_clause(iter)?)));
    }

    // v184: an arith-for header `((init;cond;step))` lexes as `TokenKind::ArithBlock`
    // only when its `((` closes with a matching `))`. An *unterminated* `((`
    // (e.g. the REPL line `for ((;;`) now falls back to two `LParen` tokens, so
    // here we see `for` immediately followed by `(` `(`. In bash, `for` may only
    // be followed by `(` as a C-style header — any other paren use is a syntax
    // error — so two consecutive `(` here mean an arith-for header that hasn't
    // closed yet. Report it as UnterminatedLoop (the v19 classifier maps that to
    // "read more"), matching bash which prompts `>` for an unclosed `for ((`.
    if matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::LParen)))
        && matches!(iter.peek2_kind()?, Some(TokenKind::Op(Operator::LParen)))
    {
        return Err(ParseError::UnterminatedLoop);
    }

    Ok(Command::For(Box::new(parse_for_after_keyword(iter)?)))
}

/// Skips `;`/newline separators before `do`, then consumes `do`, the
/// loop body, and `done`. Returns the parsed body `Sequence`. Shared
/// by `parse_for_after_keyword` and `parse_select_command`.
fn parse_do_body_done(iter: &mut Lexer) -> Result<Sequence, ParseError> {
    while matches!(
        iter.peek_kind()?,
        Some(TokenKind::Op(Operator::Semi)) | Some(TokenKind::Newline)
    ) {
        iter.next_kind()?;
    }
    expect_keyword(iter, Keyword::Do, ParseError::UnterminatedLoop)?;
    let body = parse_compound_section(iter, &[Keyword::Done], ParseError::UnterminatedLoop)?;
    expect_keyword(iter, Keyword::Done, ParseError::UnterminatedLoop)?;
    Ok(body)
}

/// Parses `for NAME [in WORD...] sep do LIST done` AFTER the `for`
/// keyword has already been consumed by the caller.
fn parse_for_after_keyword(
    iter: &mut Lexer,
) -> Result<ForClause, ParseError> {
    // Loop variable. End-of-input means the command is incomplete (the
    // v19 classifier maps UnterminatedLoop to "read more"); a present
    // but invalid token is a genuine error.
    let var = match iter.next_kind()? {
        None => return Err(ParseError::UnterminatedLoop),
        Some(tok) => for_variable_name(&tok).ok_or(ParseError::ForVariable)?,
    };

    // POSIX allows a linebreak between the variable and `in`.
    skip_newlines(iter)?;

    // Optional `in` plus the word list.
    let mut words: Vec<Word> = Vec::new();
    let has_in = if iter.peek_kind()?.and_then(keyword_of) == Some(Keyword::In) {
        iter.next_kind()?; // consume `in`
        loop {
            match iter.peek_kind()? {
                None | Some(TokenKind::Newline) | Some(TokenKind::Op(Operator::Semi)) => break,
                Some(tok) => {
                    if keyword_of(tok) == Some(Keyword::Do) {
                        break;
                    }
                    match iter.next_kind()? {
                        Some(TokenKind::Word(w)) => words.push(w),
                        Some(TokenKind::Op(_)) => return Err(ParseError::UnexpectedToken),
                        _ => unreachable!("peek already ruled out Newline/Semi/None here"),
                    }
                }
            }
        }
        true
    } else {
        false
    };

    let body = parse_do_body_done(iter)?;
    Ok(ForClause { var, words, has_in, body })
}

/// Parses `select NAME [in WORD...] sep do LIST done`. The `select` keyword
/// has already been consumed by the caller. Mirrors `parse_for_after_keyword`
/// but builds a `SelectClause` with `words: Option<Vec<Word>>` to distinguish
/// the no-`in` form (None) from an explicit `in` clause (Some, possibly empty).
fn parse_select_command(
    iter: &mut Lexer,
) -> Result<Command, ParseError> {
    // Loop variable. End-of-input means the command is incomplete.
    let var = match iter.next_kind()? {
        None => return Err(ParseError::UnterminatedLoop),
        Some(tok) => for_variable_name(&tok).ok_or(ParseError::ForVariable)?,
    };

    // POSIX allows a linebreak between the variable and `in`.
    skip_newlines(iter)?;

    // Optional `in` plus the word list.
    let words: Option<Vec<Word>> = if iter.peek_kind()?.and_then(keyword_of) == Some(Keyword::In) {
        iter.next_kind()?; // consume `in`
        let mut list: Vec<Word> = Vec::new();
        loop {
            match iter.peek_kind()? {
                None | Some(TokenKind::Newline) | Some(TokenKind::Op(Operator::Semi)) => break,
                Some(tok) => {
                    if keyword_of(tok) == Some(Keyword::Do) {
                        break;
                    }
                    match iter.next_kind()? {
                        Some(TokenKind::Word(w)) => list.push(w),
                        Some(TokenKind::Op(_)) => return Err(ParseError::UnexpectedToken),
                        _ => unreachable!("peek already ruled out Newline/Semi/None here"),
                    }
                }
            }
        }
        Some(list)
    } else {
        None
    };

    let body = parse_do_body_done(iter)?;
    Ok(Command::Select(Box::new(SelectClause { var, words, body })))
}

/// Parses a `coproc` body after the `coproc` keyword has been consumed.
///
/// Named form (`coproc NAME compound`): consumed only when the token after
/// the plain-word NAME starts a compound command.  Otherwise anonymous:
/// name = "COPROC" and the body is the next ordinary command (simple or
/// compound).
fn parse_coproc_command(iter: &mut Lexer) -> Result<Command, ParseError> {
    // Named form: a valid-identifier Word followed by a compound-command opener.
    let is_named = matches!(iter.peek_kind()?, Some(TokenKind::Word(w))
        if valid_identifier_text(w).is_some())
        && is_compound_opener(iter.peek2_kind()?);

    if is_named {
        // Consume the NAME word (we already verified it's a valid identifier).
        let name = match iter.next_kind()? {
            Some(TokenKind::Word(w)) => valid_identifier_text(&w)
                .expect("verified above"),
            _ => unreachable!("peek matched TokenKind::Word"),
        };
        let body = parse_command_inner(iter)?;
        return Ok(Command::Coproc { name, body: Box::new(body) });
    }

    // Anonymous: parse the body as an ordinary command.
    let body = parse_command_inner(iter)?;
    Ok(Command::Coproc { name: "COPROC".to_string(), body: Box::new(body) })
}

/// True if `tok` is the first token of a compound command
/// (`{`, `(`, if/while/until/for/case/select, `[[`, `((`).
pub(crate) fn is_compound_opener(tok: Option<&TokenKind>) -> bool {
    match tok {
        Some(TokenKind::Op(Operator::LParen)) => true,
        Some(TokenKind::ArithBlock(..)) => true,
        Some(t) => matches!(
            keyword_of(t),
            Some(Keyword::LBrace)
                | Some(Keyword::If)
                | Some(Keyword::While)
                | Some(Keyword::Until)
                | Some(Keyword::For)
                | Some(Keyword::Case)
                | Some(Keyword::Select)
                | Some(Keyword::DoubleBracketOpen)
        ),
        None => false,
    }
}

/// Parses `case WORD in [clause]... esac`. The caller has peeked `case`.
fn parse_case(
    iter: &mut Lexer,
) -> Result<CaseClause, ParseError> {
    expect_keyword(iter, Keyword::Case, ParseError::UnterminatedCase)?;
    skip_newlines(iter)?;

    let subject = match iter.next_kind()? {
        None => return Err(ParseError::UnterminatedCase),
        Some(TokenKind::Word(w)) => w,
        Some(_) => return Err(ParseError::UnexpectedToken),
    };

    skip_newlines(iter)?;
    expect_keyword(iter, Keyword::In, ParseError::UnterminatedCase)?;
    skip_newlines(iter)?;

    let mut items: Vec<CaseItem> = Vec::new();
    while iter.peek_kind()?.and_then(keyword_of) != Some(Keyword::Esac) {
        if iter.peek_kind()?.is_none() {
            return Err(ParseError::UnterminatedCase);
        }
        items.push(parse_case_item(iter)?);
        skip_newlines(iter)?;
    }
    expect_keyword(iter, Keyword::Esac, ParseError::UnterminatedCase)?;
    Ok(CaseClause { subject, items })
}

/// Parses one `[(] pattern [| pattern]... ) [body] [terminator]` clause.
fn parse_case_item(
    iter: &mut Lexer,
) -> Result<CaseItem, ParseError> {
    // Optional leading `(`.
    if matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::LParen))) {
        iter.next_kind()?;
    }

    // Pattern list — Word (`|` Word)* `)`, non-empty.
    let mut patterns: Vec<Word> = Vec::new();
    loop {
        skip_newlines(iter)?;
        match iter.next_kind()? {
            None => return Err(ParseError::UnterminatedCase),
            Some(TokenKind::Word(w)) => patterns.push(w),
            Some(_) => return Err(ParseError::UnexpectedToken),
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

    // Body — empty if the next token is a terminator or `esac`.
    skip_newlines(iter)?;
    let body = match iter.peek_kind()? {
        None => return Err(ParseError::UnterminatedCase),
        Some(TokenKind::Op(
            Operator::DoubleSemi | Operator::SemiAmp | Operator::DoubleSemiAmp,
        )) => None,
        Some(tok) if keyword_of(tok) == Some(Keyword::Esac) => None,
        Some(_) => Some(parse_sequence(iter, &[Keyword::Esac])?),
    };

    // Terminator — an absent one (next token is `esac` or end) is `Break`.
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

/// Parses `{ LIST }`. The caller has peeked the leading `{`.
fn parse_brace_group(
    iter: &mut Lexer,
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
fn parse_subshell(
    iter: &mut Lexer,
) -> Result<Command, ParseError> {
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

    // Parse the inner sequence using parse_subshell_sequence, which mirrors
    // parse_sequence but terminates on `)` instead of on stop-keywords.
    let body = parse_subshell_sequence(iter)?;
    Ok(Command::Subshell { body: Box::new(body) })
}

/// Parses a sequence of commands terminated by `)`. Mirrors `parse_sequence`
/// but:
/// - breaks on `TokenKind::Op(Operator::RParen)` (consuming it) instead of keywords.
/// - returns `Err(UnterminatedSubshell)` if the token stream ends before `)`.
fn parse_subshell_sequence(
    iter: &mut Lexer,
) -> Result<Sequence, ParseError> {
    // Parse first command. It may itself be a subshell, compound command, etc.
    // — and, if followed by `|`, the rest of the pipeline.
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
                // and acts as a separator. Skip any redundant `;` or newlines
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
                // next command and continue. The `&` backgrounds the preceding
                // group (v98); only the trailing `&` before `)` sets background.
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

/// Parses `while LIST; do LIST; done` or `until LIST; do LIST; done`.
/// The caller has already peeked the leading `while`/`until`.
fn parse_while(
    iter: &mut Lexer,
) -> Result<WhileClause, ParseError> {
    let until = match iter.next_kind()?.as_ref().and_then(keyword_of) {
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

/// True for the operators that introduce a redirection (and thus a trailing
/// target word). Excludes pipeline/grouping operators (`|`, `(`, `)`, etc.).
pub(crate) fn is_redirect_op(op: &Operator) -> bool {
    matches!(
        op,
        Operator::RedirIn
            | Operator::RedirOut
            | Operator::RedirAppend
            | Operator::RedirErr
            | Operator::RedirErrAppend
            | Operator::HereString
            | Operator::DupOut
            | Operator::DupErr
            | Operator::DupIn
            | Operator::RedirReadWrite
            | Operator::AndRedirOut
            | Operator::AndRedirAppend
            | Operator::RedirClobber
            | Operator::RedirErrClobber
    )
}

/// Builds the ordered `Redirection`(s) for a redirect operator + target word.
///
/// `fd_prefix` is `Some` when an explicit `TokenKind::RedirFd` preceded the
/// operator (`3>`, `{fd}>`), else `None`. Most operators map to a single
/// `Redirection` with `fd = fd_prefix.unwrap_or(Default)`. The stderr-default
/// operators (`2>`/`2>>`/`2>&`/`2>|`) default their fd to `Number(2)` when no
/// explicit prefix is given. `&>`/`&>>` desugar to TWO redirections
/// (file-to-stdout + `2>&1`).
pub(crate) fn build_redirections(
    op: Operator,
    target: Word,
    fd_prefix: Option<RedirFd>,
) -> Vec<Redirection> {
    // Default fd for the stderr-family operators when unprefixed.
    let err_fd = || fd_prefix.clone().unwrap_or(RedirFd::Number(2));
    let plain_fd = || fd_prefix.clone().unwrap_or(RedirFd::Default);
    match op {
        Operator::RedirIn => vec![Redirection {
            fd: plain_fd(),
            op: RedirOp::File { mode: FileMode::ReadOnly, target },
        }],
        Operator::RedirOut => vec![Redirection {
            fd: plain_fd(),
            op: RedirOp::File { mode: FileMode::Truncate, target },
        }],
        Operator::RedirAppend => vec![Redirection {
            fd: plain_fd(),
            op: RedirOp::File { mode: FileMode::Append, target },
        }],
        Operator::RedirClobber => vec![Redirection {
            fd: plain_fd(),
            op: RedirOp::File { mode: FileMode::Clobber, target },
        }],
        Operator::RedirReadWrite => vec![Redirection {
            fd: plain_fd(),
            op: RedirOp::File { mode: FileMode::ReadWrite, target },
        }],
        Operator::RedirErr => vec![Redirection {
            fd: err_fd(),
            op: RedirOp::File { mode: FileMode::Truncate, target },
        }],
        Operator::RedirErrAppend => vec![Redirection {
            fd: err_fd(),
            op: RedirOp::File { mode: FileMode::Append, target },
        }],
        Operator::RedirErrClobber => vec![Redirection {
            fd: err_fd(),
            op: RedirOp::File { mode: FileMode::Clobber, target },
        }],
        Operator::HereString => vec![Redirection {
            fd: plain_fd(),
            op: RedirOp::HereString(target),
        }],
        Operator::DupOut => {
            let op = dup_op(target, true);
            // When the source is `-` (Close), use fd 1 as the directional
            // default (output dup targets stdout). `plain_fd()` would fall
            // back to `Default` which resolves to 0 — the wrong fd.
            let fd = if matches!(op, RedirOp::Close) {
                fd_prefix.clone().unwrap_or(RedirFd::Number(1))
            } else {
                plain_fd()
            };
            vec![Redirection { fd, op }]
        }
        Operator::DupErr => {
            let op = dup_op(target, true);
            // DupErr already uses err_fd() which defaults to Number(2) —
            // correct for both Dup and Close.
            vec![Redirection { fd: err_fd(), op }]
        }
        Operator::DupIn => {
            let op = dup_op(target, false);
            // When the source is `-` (Close), use fd 0 as the directional
            // default (input dup targets stdin). `plain_fd()` would fall
            // back to `Default` which also resolves to 0, so this is
            // technically a no-op for DupIn — but we make it explicit for
            // symmetry and to avoid relying on the Default->0 fallback.
            let fd = if matches!(op, RedirOp::Close) {
                fd_prefix.clone().unwrap_or(RedirFd::Number(0))
            } else {
                plain_fd()
            };
            vec![Redirection { fd, op }]
        }
        Operator::AndRedirOut => vec![
            Redirection {
                fd: plain_fd(),
                op: RedirOp::File { mode: FileMode::Truncate, target },
            },
            Redirection {
                fd: RedirFd::Number(2),
                op: RedirOp::Dup { source: lit_word("1"), output: true },
            },
        ],
        Operator::AndRedirAppend => vec![
            Redirection {
                fd: plain_fd(),
                op: RedirOp::File { mode: FileMode::Append, target },
            },
            Redirection {
                fd: RedirFd::Number(2),
                op: RedirOp::Dup { source: lit_word("1"), output: true },
            },
        ],
        // is_redirect_op gates the callers; no other operator reaches here.
        _ => unreachable!("build_redirections called with a non-redirect operator"),
    }
}

/// `>&w`/`<&w`: a `-` source word closes the fd; otherwise a Dup.
pub(crate) fn dup_op(source: Word, output: bool) -> RedirOp {
    if word_literal_text(&source) == Some("-") {
        RedirOp::Close
    } else {
        RedirOp::Dup { source, output }
    }
}

/// True iff the next token begins a redirection: a `TokenKind::RedirFd` prefix,
/// a `TokenKind::Heredoc`, or a redirect operator.
pub(crate) fn next_is_redirect(iter: &mut Lexer) -> Result<bool, ParseError> {
    Ok(match iter.peek_kind()? {
        Some(TokenKind::RedirFd(_)) => true,
        Some(TokenKind::Heredoc { .. }) => true,
        Some(TokenKind::Op(op)) => is_redirect_op(op),
        _ => false,
    })
}

/// Consumes a run of trailing redirect tokens (an optional `TokenKind::RedirFd`
/// prefix, then a redirect operator + target word, or a `TokenKind::Heredoc`)
/// from `iter`, building an ORDERED list of `Redirection`s in source order
/// (no last-wins merge — ordering matters for `2>&1 >f` vs `>f 2>&1`).
/// Stops at the first non-redirect token.
fn parse_trailing_redirects(
    iter: &mut Lexer,
) -> Result<Vec<Redirection>, ParseError> {
    let mut redirs: Vec<Redirection> = Vec::new();
    loop {
        // Optional explicit fd-prefix (`3>`, `{fd}>`, `3<<EOF`).
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
                let Some(TokenKind::Heredoc { body, expand, strip_tabs }) = iter.next_kind()? else {
                    unreachable!("peek confirmed Heredoc")
                };
                redirs.push(Redirection {
                    fd: fd_prefix.unwrap_or(RedirFd::Number(0)),
                    op: RedirOp::Heredoc { body, expand, strip_tabs },
                });
            }
            Some(TokenKind::Op(op)) if is_redirect_op(op) => {
                let op = *op;
                iter.next_kind()?;
                let target = match iter.next_kind()? {
                    Some(TokenKind::Word(word)) => word,
                    Some(TokenKind::Op(_)) => return Err(ParseError::RedirectTargetIsOperator),
                    Some(TokenKind::Newline) | None => return Err(ParseError::MissingRedirectTarget),
                    Some(TokenKind::Heredoc { .. }) => return Err(ParseError::RedirectTargetIsOperator),
                    Some(TokenKind::RedirFd(_)) => return Err(ParseError::RedirectTargetIsOperator),
                    Some(TokenKind::ArithBlock(..)) => return Err(ParseError::RedirectTargetIsOperator),
                    // Phase C atom variants (dormant in v241 — never emitted in Command mode)
                    Some(_) => return Err(ParseError::RedirectTargetIsOperator),
                };
                redirs.extend(build_redirections(op, target, fd_prefix));
            }
            _ => {
                // A bare fd-prefix with no following redirect operator should
                // not happen (the lexer only emits RedirFd glued to an op), but
                // guard defensively: a dangling prefix means a missing target.
                if fd_prefix.is_some() {
                    return Err(ParseError::MissingRedirectTarget);
                }
                break;
            }
        }
    }
    Ok(redirs)
}

/// Wraps a freshly-parsed compound command in `Command::Redirected` when one
/// or more redirects immediately follow its terminator; otherwise returns the
/// command unchanged. Applied to compound arms only (simple commands consume
/// their own redirects inline in `parse_simple_stage`).
fn maybe_wrap_redirects(
    cmd: Command,
    iter: &mut Lexer,
) -> Result<Command, ParseError> {
    let redirects = parse_trailing_redirects(iter)?;
    if !redirects.is_empty() {
        Ok(Command::Redirected { inner: Box::new(cmd), redirects })
    } else {
        Ok(cmd)
    }
}

/// Parses a simple-command stage — accumulates program/args/redirects
/// tokens until a pipeline terminator (`|`, `;`, `&&`, `||`, newline,
/// etc.) is reached. Returns `Command::Simple(...)` and a flag indicating
/// whether a `|` was consumed (meaning more stages follow).
///
/// The first word may be supplied as `first` (already consumed by the
/// caller for function-def lookahead). When `first` is `None` the
/// function reads the initial word from the iterator.
/// `first_line` is the 1-based source line of the command's first token
/// (captured by the caller before consuming that token).
fn parse_simple_stage(
    first: Option<Word>,
    prefix_tokens: Vec<TokenKind>,
    iter: &mut Lexer,
    first_line: u32,
) -> Result<(Command, bool), ParseError> {
    let mut program: Option<Word> = first;
    let mut args: Vec<Word> = Vec::new();
    let mut redirects: Vec<Redirection> = Vec::new();
    let mut pipe_follows = false;

    // Drain prefix_tokens first (extra assignment words re-injected by the
    // multi-assign speculative-peel path). These are always TokenKind::Word items
    // so we process them directly without going through the pipeline-terminator
    // peek-break that guards the main loop.
    for tok in prefix_tokens {
        match tok {
            TokenKind::Word(word) => {
                if program.is_none() {
                    program = Some(word);
                } else {
                    args.push(word);
                }
            }
            // prefix_tokens only ever contains TokenKind::Word items in the
            // current caller; guard other variants to prevent silent misbehaviour.
            _ => unreachable!("prefix_tokens should only contain TokenKind::Word"),
        }
    }

    loop {
        // Trailing-blank alias chain: if the last command-position expansion
        // ended with a blank, the next argument word is eligible for alias
        // expansion too. take_trailing_eligible() returns true at most once
        // per expansion (it resets the flag), so this is a no-op in the
        // overwhelmingly common non-alias or non-trailing-blank case.
        if iter.take_trailing_eligible() {
            iter.expand_command_alias()?;
        }
        let Some(token) = iter.peek_kind()? else { break };
        if matches!(
            token,
            TokenKind::Op(
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
            ) | TokenKind::Newline
        ) {
            break;
        }
        // Redirect tokens (an fd-prefix, a heredoc, or a redirect operator) —
        // delegate to the shared `parse_trailing_redirects` helper, appending
        // its ordered Redirections to this stage's. This keeps the simple- and
        // compound-command redirect semantics identical.
        if next_is_redirect(iter)? {
            redirects.extend(parse_trailing_redirects(iter)?);
            continue;
        }
        let token = iter.next_kind()?.unwrap();
        match token {
            TokenKind::Word(word) => {
                if program.is_none() {
                    program = Some(word);
                } else {
                    args.push(word);
                }
            }
            TokenKind::Newline => {
                // Unreachable: the peek-break above stops the loop on a
                // Newline before it is ever consumed here.
                unreachable!("Newline terminates the stage via the peek-break above");
            }
            TokenKind::Op(Operator::Pipe) => {
                pipe_follows = true;
                skip_newlines(iter)?;
                break;
            }
            TokenKind::Op(Operator::LParen) => {
                // A `(` mid-argument (e.g. `cmd (args)`) is a syntax error.
                // Note: `(` at command-start is dispatched by parse_command
                // before parse_simple_stage is called.
                return Err(ParseError::UnexpectedToken);
            }
            TokenKind::ArithBlock(..) => {
                // `((...))` mid-argument (e.g. `cmd ((1+2))`) is a syntax
                // error. The standalone arith block at command-start is
                // dispatched by parse_command before parse_simple_stage runs.
                return Err(ParseError::UnexpectedToken);
            }
            TokenKind::Op(Operator::RParen) => {
                // Unreachable: the peek-break above stops on RParen.
                unreachable!("RParen terminates the stage via the peek-break above");
            }
            TokenKind::Heredoc { .. } => {
                // Unreachable: the redirect-delegation branch above consumes
                // heredoc tokens before this match.
                unreachable!("Heredoc consumed by parse_trailing_redirects branch");
            }
            TokenKind::Op(_) => {
                // A non-terminator, non-redirect operator at this point is a
                // syntax error (redirect ops are consumed by the delegation
                // branch; terminators break via the peek-break above).
                return Err(ParseError::UnexpectedToken);
            }
            TokenKind::RedirFd(_) => {
                // Unreachable: next_is_redirect consumes RedirFd prefixes via
                // the delegation branch above before this match.
                unreachable!("RedirFd consumed by parse_trailing_redirects branch");
            }
            // Phase C atom variants (dormant in v241 — never emitted in Command mode)
            _ => unreachable!("Phase C atom reached Command-mode simple-command parser"),
        }
    }

    let prog = match program {
        Some(p) => p,
        None => {
            // No program word and (since args are only pushed after a program
            // word is seen) no arguments. If redirections are present, this is a
            // bare redirect-only command (`>file`, `2>err`, `<in`): bash performs
            // the redirections for their side effects — `>file` truncates/creates
            // it — and exits 0 (M-123). Build the empty-program ExecCommand the
            // executor already handles for the `VAR=val 2>err` sibling case. With
            // no redirections either, the command is genuinely empty: keep
            // MissingCommand so the caller can treat an exhausted iterator as a
            // line continuation and a pending one as a real "expected a command".
            if redirects.is_empty() {
                return Err(ParseError::MissingCommand);
            }
            let cmd = Command::Simple(SimpleCommand::Exec(ExecCommand {
                inline_assignments: Vec::new(),
                program: Word(Vec::new()),
                args: Vec::new(),
                redirects,
                line: first_line,
            }));
            return Ok((cmd, pipe_follows));
        }
    };
    let cmd = Command::Simple(finalize_stage(prog, args, redirects, first_line));
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
fn parse_next_stage(
    iter: &mut Lexer,
) -> Result<(Command, bool), ParseError> {
    iter.expand_command_alias()?;
    // Standalone arith block: `((expr))` at pipeline-stage position
    // (e.g., `((x++)) | cat`). Mirrors the dispatch in parse_command.
    if matches!(iter.peek_kind()?, Some(TokenKind::ArithBlock(..))) {
        let Some(TokenKind::ArithBlock(text, opts)) = iter.next_kind()? else {
            unreachable!("matches! guard above guarantees ArithBlock")
        };
        let body = crate::lexer::arith_string_to_word(&text, opts)
            .map_err(|e| ParseError::ArithBlock(crate::lex_error_message(&e)))?;
        return Ok((maybe_wrap_redirects(Command::Arith(body), iter)?, false));
    }

    match iter.peek_kind()?.and_then(keyword_of) {
        Some(Keyword::If) => Ok((
            maybe_wrap_redirects(Command::If(Box::new(parse_if(iter)?)), iter)?,
            false,
        )),
        Some(Keyword::While) | Some(Keyword::Until) => Ok((
            maybe_wrap_redirects(Command::While(Box::new(parse_while(iter)?)), iter)?,
            false,
        )),
        Some(Keyword::For) => {
            let cmd = parse_for_command(iter)?;
            Ok((maybe_wrap_redirects(cmd, iter)?, false))
        }
        Some(Keyword::Select) => {
            iter.next_kind()?; // consume `select`
            let cmd = parse_select_command(iter)?;
            Ok((maybe_wrap_redirects(cmd, iter)?, false))
        }
        Some(Keyword::Case) => Ok((
            maybe_wrap_redirects(Command::Case(Box::new(parse_case(iter)?)), iter)?,
            false,
        )),
        Some(Keyword::LBrace) => Ok((
            maybe_wrap_redirects(Command::BraceGroup(Box::new(parse_brace_group(iter)?)), iter)?,
            false,
        )),
        Some(Keyword::DoubleBracketOpen) => {
            let cmd = parse_double_bracket(iter)?;
            Ok((maybe_wrap_redirects(cmd, iter)?, false))
        }
        // `coproc` is invalid as a pipeline stage (it's a top-level command).
        Some(Keyword::Coproc) => Err(ParseError::UnexpectedKeyword("coproc".to_string())),
        Some(other) => Err(ParseError::UnexpectedKeyword(other.name().to_string())),
        None => {
            // Bare `(` at pipeline-stage position → subshell.
            if matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::LParen))) {
                let cmd = parse_subshell(iter)?;
                return Ok((maybe_wrap_redirects(cmd, iter)?, false));
            }
            // Non-keyword: may be a function definition `name() compound` or
            // a plain simple stage. Need two-token lookahead.
            if matches!(iter.peek_kind()?, Some(TokenKind::Word(_))) {
                // Capture line before consuming the first word.
                let word_line = iter.current_line()?;
                let Some(TokenKind::Word(w)) = iter.next_kind()? else { unreachable!() };
                if matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::LParen))) {
                    let cmd = parse_function_def(w, iter)?;
                    return Ok((cmd, false));
                }
                // Not a function def — simple stage with `w` as the first word.
                parse_simple_stage(Some(w), vec![], iter, word_line)
            } else {
                let first_line = iter.current_line()?;
                parse_simple_stage(None, vec![], iter, first_line)
            }
        }
    }
}

fn parse_pipeline_with_first(
    first: Option<Word>,
    prefix_tokens: Vec<TokenKind>,
    iter: &mut Lexer,
    first_line: u32,
) -> Result<Pipeline, ParseError> {
    let mut commands: Vec<Command> = Vec::new();

    // Parse the first stage as a simple stage (the caller has already
    // consumed the first word for function-def lookahead purposes, so we
    // pass it along). The first stage is always a simple command because
    // `parse_command` dispatches compound commands before calling us, so
    // we only arrive here for simple-command pipelines.
    let (first_cmd, mut pipe_follows) = parse_simple_stage(first, prefix_tokens, iter, first_line)?;
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
            if matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::Pipe))) {
                iter.next_kind()?; // consume `|`
                skip_newlines(iter)?;
                // pipe_follows remains true — loop continues.
            } else {
                pipe_follows = false;
            }
        }
        // else: simple stage already consumed `|`; pipe_follows remains true.
    }

    Ok(Pipeline { negate: false, commands })
}

fn parse_pipeline(
    iter: &mut Lexer,
) -> Result<Pipeline, ParseError> {
    let first_line = iter.current_line()?;
    parse_pipeline_with_first(None, vec![], iter, first_line)
}

// ──────────────────────────────────────────────────────────────
// [[ ]] extended test — Pratt-style parser (v30)
// ──────────────────────────────────────────────────────────────

/// Returns `true` if the next token signals the end of a `[[ ]]` expression
/// body (i.e. we should stop trying to parse more at this precedence level).
/// This means we've hit `]]` (DoubleBracketClose keyword), `)` (RParen for
/// grouped sub-expressions), or end of input.
fn is_test_expr_stop(iter: &mut Lexer) -> Result<bool, ParseError> {
    Ok(match iter.peek_kind()? {
        None => true,
        Some(tok) => keyword_of(tok) == Some(Keyword::DoubleBracketClose)
            || matches!(tok, TokenKind::Op(Operator::RParen)),
    })
}

/// Peeks (consumes nothing) and reports whether the next token is a recognized
/// `[[ ]]` binary operator. Used by `parse_test_atom` to distinguish a binary
/// test (`lhs OP rhs`) from a bare-word test (`[[ word ]]` ≡ `[[ -n word ]]`).
///
/// KEEP THIS OPERATOR SET IN SYNC with the operator match arms in
/// `parse_test_atom` below. `<` / `>` arrive as `Op(RedirIn)` / `Op(RedirOut)`;
/// every other operator arrives as a `Word` because the lexer has no dedicated
/// token for it.
pub(crate) fn next_is_test_binary_operator(
    iter: &mut Lexer,
) -> Result<bool, ParseError> {
    Ok(match iter.peek_kind()? {
        Some(TokenKind::Op(Operator::RedirIn)) | Some(TokenKind::Op(Operator::RedirOut)) => true,
        Some(TokenKind::Word(w)) => matches!(
            word_literal_text(w),
            Some("==" | "=" | "!=" | "=~" | "-eq" | "-ne" | "-lt" | "-gt"
                | "-le" | "-ge" | "-nt" | "-ot" | "-ef")
        ),
        _ => false,
    })
}

/// Skips zero or more `TokenKind::Newline` tokens inside a `[[ … ]]` expression.
/// Bash treats newlines as whitespace anywhere inside `[[ ]]`.
pub(crate) fn skip_test_newlines(iter: &mut Lexer) -> Result<(), ParseError> {
    while matches!(iter.peek_kind()?, Some(TokenKind::Newline)) {
        iter.next_kind()?;
    }
    Ok(())
}

/// Returns the Word's single unquoted Literal text, if it is exactly that shape.
/// Used to identify operator words like `==`, `!=`, `-eq`, etc.
pub fn word_literal_text(w: &Word) -> Option<&str> {
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
pub(crate) fn try_unary_op(w: &Word) -> Option<TestUnaryOp> {
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
        "-v" => Some(TestUnaryOp::VarSet),
        "-o" => Some(TestUnaryOp::OptEnabled),
        "-p" => Some(TestUnaryOp::IsFifo),
        "-S" => Some(TestUnaryOp::IsSocket),
        "-b" => Some(TestUnaryOp::IsBlockDev),
        "-c" => Some(TestUnaryOp::IsCharDev),
        "-O" => Some(TestUnaryOp::OwnedByEuid),
        "-G" => Some(TestUnaryOp::OwnedByEgid),
        "-N" => Some(TestUnaryOp::NewerThanRead),
        "-k" => Some(TestUnaryOp::IsSticky),
        "-u" => Some(TestUnaryOp::IsSetuid),
        "-g" => Some(TestUnaryOp::IsSetgid),
        "-t" => Some(TestUnaryOp::IsTerminal),
        _ => None,
    }
}


/// Returns true if `w` is the literal word `!` (unquoted).
pub(crate) fn is_bang_word(tok: &TokenKind) -> bool {
    match tok {
        TokenKind::Word(w) => word_literal_text(w) == Some("!"),
        _ => false,
    }
}

/// Reads the next token and asserts it is a Word. Returns the Word; on
/// end-of-input (inside an unclosed `[[`) returns
/// `ParseError::UnterminatedDoubleBracket` so the REPL requests a
/// continuation line, and on a present-but-wrong token (`]]`/an operator,
/// i.e. a genuine missing operand) returns `ParseError::TestExprMissingOperand`.
fn next_test_word(
    iter: &mut Lexer,
) -> Result<Word, ParseError> {
    match iter.peek_kind()? {
        None => return Err(ParseError::UnterminatedDoubleBracket),
        Some(tok) => {
            if keyword_of(tok) == Some(Keyword::DoubleBracketClose)
                || matches!(tok, TokenKind::Op(_))
            {
                return Err(ParseError::TestExprMissingOperand);
            }
        }
    }
    match iter.next_kind()? {
        Some(TokenKind::Word(w)) => Ok(w),
        _ => Err(ParseError::TestExprMissingOperand),
    }
}

/// Lowest precedence: `||`.
fn parse_test_or(
    iter: &mut Lexer,
) -> Result<TestExpr, ParseError> {
    let mut lhs = parse_test_and(iter)?;
    skip_test_newlines(iter)?;
    while matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::Or))) {
        iter.next_kind()?; // consume `||`
        skip_test_newlines(iter)?;
        let rhs = parse_test_and(iter)?;
        lhs = TestExpr::Or(Box::new(lhs), Box::new(rhs));
        skip_test_newlines(iter)?;
    }
    Ok(lhs)
}

/// Next precedence: `&&`.
fn parse_test_and(
    iter: &mut Lexer,
) -> Result<TestExpr, ParseError> {
    let mut lhs = parse_test_not(iter)?;
    skip_test_newlines(iter)?;
    while matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::And))) {
        iter.next_kind()?; // consume `&&`
        skip_test_newlines(iter)?;
        let rhs = parse_test_not(iter)?;
        lhs = TestExpr::And(Box::new(lhs), Box::new(rhs));
        skip_test_newlines(iter)?;
    }
    Ok(lhs)
}

/// Next precedence: `!` (right-associative).
fn parse_test_not(
    iter: &mut Lexer,
) -> Result<TestExpr, ParseError> {
    if iter.peek_kind()?.map(is_bang_word).unwrap_or(false) {
        iter.next_kind()?; // consume `!`
        let inner = parse_test_not(iter)?;
        return Ok(TestExpr::Not(Box::new(inner)));
    }
    parse_test_primary(iter)
}

/// Highest precedence: `( expr )` grouping or a single test atom.
fn parse_test_primary(
    iter: &mut Lexer,
) -> Result<TestExpr, ParseError> {
    // Grouping: `( expr )`.
    if matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::LParen))) {
        iter.next_kind()?; // consume `(`
        let inner = parse_test_or(iter)?;
        // Expect `)`.
        match iter.next_kind()? {
            Some(TokenKind::Op(Operator::RParen)) => {}
            None => return Err(ParseError::UnterminatedDoubleBracket),
            _ => return Err(ParseError::TestExprMissingOperand),
        }
        return Ok(inner);
    }
    parse_test_atom(iter)
}

/// Parses a single test — either a unary test (`-f path`) or a
/// binary/regex test (`lhs op rhs`).
fn parse_test_atom(
    iter: &mut Lexer,
) -> Result<TestExpr, ParseError> {
    // End of input mid-expression → unterminated (request continuation).
    if iter.peek_kind()?.is_none() {
        return Err(ParseError::UnterminatedDoubleBracket);
    }
    // A present terminator (`]]` / `)`) with nothing before it → empty body.
    if is_test_expr_stop(iter)? {
        return Err(ParseError::EmptyDoubleBracket);
    }

    // Peek at the first word to check if it is a unary op.
    let first_word = match iter.peek_kind()? {
        Some(TokenKind::Word(w)) => w.clone(),
        _ => return Err(ParseError::TestExprMissingOperand),
    };

    // Unary op path: `-f`, `-d`, `-n`, `-z`, …
    if let Some(op) = try_unary_op(&first_word) {
        iter.next_kind()?; // consume the op word
        let operand = next_test_word(iter)?;
        return Ok(TestExpr::Unary { op, operand });
    }

    // Binary / regex path: lhs op rhs.
    // Consume the LHS word (first_word peeked above).
    iter.next_kind()?;
    let lhs = first_word;

    // Bash: `[[ word ]]` ≡ `[[ -n word ]]`. When no binary operator follows the
    // operand (next token is `]]` / `)` / `&&` / `||` / end-of-input), the lhs
    // alone is a non-empty-string test. See `next_is_test_binary_operator` —
    // keep its operator set in sync with the match arms below.
    if !next_is_test_binary_operator(iter)? {
        return Ok(TestExpr::Unary { op: TestUnaryOp::StringNonEmpty, operand: lhs });
    }

    // Peek at the operator token.  Operators like `==`, `!=`, `=~`, `<`, `>`,
    // `-eq`, etc. arrive as Word tokens because the lexer doesn't have them as
    // separate operators.  `<` and `>` ARE lexer operator tokens (RedirIn /
    // RedirOut) — handle those too.
    let op_token = iter.next_kind()?;
    match op_token {
        None => Err(ParseError::UnterminatedDoubleBracket),
        // `<` and `>` are lexed as RedirIn / RedirOut even inside `[[ ]]`.
        Some(TokenKind::Op(Operator::RedirIn)) => {
            let rhs = next_test_word(iter)?;
            Ok(TestExpr::Binary { op: TestBinaryOp::StringLt, lhs, rhs })
        }
        Some(TokenKind::Op(Operator::RedirOut)) => {
            let rhs = next_test_word(iter)?;
            Ok(TestExpr::Binary { op: TestBinaryOp::StringGt, lhs, rhs })
        }
        Some(TokenKind::Word(op_word)) => {
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
                "-nt" => {
                    let rhs = next_test_word(iter)?;
                    Ok(TestExpr::Binary { op: TestBinaryOp::NewerThan, lhs, rhs })
                }
                "-ot" => {
                    let rhs = next_test_word(iter)?;
                    Ok(TestExpr::Binary { op: TestBinaryOp::OlderThan, lhs, rhs })
                }
                "-ef" => {
                    let rhs = next_test_word(iter)?;
                    Ok(TestExpr::Binary { op: TestBinaryOp::SameFile, lhs, rhs })
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
fn parse_double_bracket(
    iter: &mut Lexer,
) -> Result<Command, ParseError> {
    parse_double_bracket_with_assigns(iter, Vec::new())
}

fn parse_double_bracket_with_assigns(
    iter: &mut Lexer,
    inline_assignments: Vec<Assignment>,
) -> Result<Command, ParseError> {
    // Consume `[[`.
    iter.next_kind()?;

    // Skip newlines after `[[` (bash allows `[[\n expr ]]`).
    skip_test_newlines(iter)?;

    // Immediately hit `]]` — empty body.
    if iter.peek_kind()?.and_then(keyword_of) == Some(Keyword::DoubleBracketClose) {
        return Err(ParseError::EmptyDoubleBracket);
    }

    // End of input — unterminated.
    if iter.peek_kind()?.is_none() {
        return Err(ParseError::UnterminatedDoubleBracket);
    }

    let expr = parse_test_or(iter)
        .map_err(|e| match e {
            // Propagate unterminated from deeper levels.
            ParseError::UnterminatedDoubleBracket => ParseError::UnterminatedDoubleBracket,
            other => other,
        })?;

    // Skip newlines before `]]` (bash allows `expr\n]]`).
    skip_test_newlines(iter)?;

    // Consume `]]`.
    match iter.next_kind()? {
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



    fn ww(s: &str) -> Word {
        Word(vec![WordPart::Literal { text: s.to_string(), quoted: false }])
    }






























































































































































    // --- Here-document parser tests (v24) ---





    // --- Pipeline compound-stage parser tests (v25 Task 2) ---







    #[test]
    #[ignore = "TODO(M-11): subshell syntax `(list)` not yet lexed; nested multi-stage \
                pipeline-as-stage can't be triggered today. Guard exists for future correctness."]
    fn parse_pipeline_rejects_nested_multi_stage() {
        // When M-11 subshell syntax lands, `echo | (a | b)` should be a
        // parse error (Command::Pipeline with len > 1 as a stage is rejected).
        // For v25, `(a | b)` isn't lexed so this test is unreachable.
    }

    // --- Pipeline compound-as-FIRST-stage parser tests (v25 Task 2 fix) ---






    // ---- v27 here-string parser tests ------------------------------------------






    // ── v28 subshell parser tests ────────────────────────────────────────────



















    // ── v156: ordered redirect-list parser tests ─────────────────────────────






    // ──────────────────────────────────────────────────────────────
    // [[ ]] parser tests (v30)
    // ──────────────────────────────────────────────────────────────























    // -----------------------------------------------------------------------
    // Regression tests: multi-assign speculative-peel iterator-drain bug
    // -----------------------------------------------------------------------




















    // ----- v78: arith block + C-style for-loop parser tests -----



























    #[test]
    fn redirop_default_fds() {
        let w = ww("f");
        assert_eq!(RedirOp::File { mode: FileMode::ReadOnly, target: w.clone() }.default_fd(), 0);
        assert_eq!(RedirOp::File { mode: FileMode::Truncate, target: w.clone() }.default_fd(), 1);
        assert_eq!(RedirOp::File { mode: FileMode::ReadWrite, target: w.clone() }.default_fd(), 0);
        assert_eq!(RedirOp::Dup { source: ww("1"), output: true }.default_fd(), 1);
        let r = Redirection { fd: RedirFd::Number(3), op: RedirOp::Close };
        assert_eq!(r.target_fd(), Some(3));
        let v = Redirection { fd: RedirFd::Var("x".into()), op: RedirOp::Close };
        assert_eq!(v.target_fd(), None);
    }

    /// Regression: cross-type low-fd redirects (e.g. `1<file`, `0>file`, `0>&1`)
    /// must be dropped by slots_for_simple_path rather than placed into the wrong slot
    /// (which would cause resolve()'s unreachable!() assertions to fire).
    #[test]
    fn slots_for_simple_path_drops_cross_type_low_fd() {
        // Read-op on fd 1 (stdout): must not fill any slot.
        let r_read_on_fd1 = Redirection {
            fd: RedirFd::Number(1),
            op: RedirOp::File { mode: FileMode::ReadOnly, target: ww("f") },
        };
        let (sin, sout, serr) = slots_for_simple_path(&[r_read_on_fd1]);
        assert!(sin.is_none(), "Read on fd1 must not fill stdin");
        assert!(sout.is_none(), "Read on fd1 must not fill stdout");
        assert!(serr.is_none(), "Read on fd1 must not fill stderr");

        // Truncate-op on fd 0 (stdin): must not fill any slot.
        let r_trunc_on_fd0 = Redirection {
            fd: RedirFd::Number(0),
            op: RedirOp::File { mode: FileMode::Truncate, target: ww("f") },
        };
        let (sin, sout, serr) = slots_for_simple_path(&[r_trunc_on_fd0]);
        assert!(sin.is_none(), "Truncate on fd0 must not fill stdin");
        assert!(sout.is_none(), "Truncate on fd0 must not fill stdout");
        assert!(serr.is_none(), "Truncate on fd0 must not fill stderr");

        // Dup{output:true} on fd 0: must not fill any slot.
        let r_dup_out_on_fd0 = Redirection {
            fd: RedirFd::Number(0),
            op: RedirOp::Dup { source: ww("1"), output: true },
        };
        let (sin, sout, serr) = slots_for_simple_path(&[r_dup_out_on_fd0]);
        assert!(sin.is_none(), "Dup(output) on fd0 must not fill stdin");
        assert!(sout.is_none(), "Dup(output) on fd0 must not fill stdout");
        assert!(serr.is_none(), "Dup(output) on fd0 must not fill stderr");

        // Read-op on fd 2 (stderr): must not fill any slot.
        let r_read_on_fd2 = Redirection {
            fd: RedirFd::Number(2),
            op: RedirOp::File { mode: FileMode::ReadOnly, target: ww("f") },
        };
        let (sin, sout, serr) = slots_for_simple_path(&[r_read_on_fd2]);
        assert!(sin.is_none(), "Read on fd2 must not fill stdin");
        assert!(sout.is_none(), "Read on fd2 must not fill stdout");
        assert!(serr.is_none(), "Read on fd2 must not fill stderr");

        // Sanity: direction-matched ops still fill slots correctly.
        let r_read_fd0 = Redirection {
            fd: RedirFd::Number(0),
            op: RedirOp::File { mode: FileMode::ReadOnly, target: ww("f") },
        };
        let r_trunc_fd1 = Redirection {
            fd: RedirFd::Number(1),
            op: RedirOp::File { mode: FileMode::Truncate, target: ww("g") },
        };
        let r_trunc_fd2 = Redirection {
            fd: RedirFd::Number(2),
            op: RedirOp::File { mode: FileMode::Truncate, target: ww("h") },
        };
        let (sin, sout, serr) = slots_for_simple_path(&[r_read_fd0, r_trunc_fd1, r_trunc_fd2]);
        assert!(sin.is_some(), "Read on fd0 should fill stdin");
        assert!(sout.is_some(), "Truncate on fd1 should fill stdout");
        assert!(serr.is_some(), "Truncate on fd2 should fill stderr");
    }

    // ── coproc parser tests (v157 task 2) ─────────────────────────────────








    #[test]
    fn try_split_assignment_ref_parity_with_consuming_form() {
        let scalar = Word(vec![WordPart::Literal { text: "name=hello".into(), quoted: false }]);

        // Peek then consume — both should agree on outcome.
        let peek = try_split_assignment_ref(&scalar);
        let consume = try_split_assignment(scalar.clone()).ok();
        assert_eq!(peek, consume);
        assert!(peek.is_some());

        // Negative: word that's not an assignment.
        let plain = Word(vec![WordPart::Literal { text: "echo".into(), quoted: false }]);
        assert!(try_split_assignment_ref(&plain).is_none());
    }
}
