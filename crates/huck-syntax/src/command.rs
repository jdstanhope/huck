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
    DoubleBracketOpen,  // [[
    DoubleBracketClose, // ]]
    Function,
    Select,
    Coproc,
}

/// Returns the keyword a token represents, or `None`. A token is a
/// keyword only when it is a `Word` of exactly one part — an *unquoted*
/// `Literal` whose text equals the keyword.
fn keyword_of(token: &TokenKind) -> Option<Keyword> {
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
pub fn try_split_assignment(word: crate::lexer::Word) -> Result<Assignment, crate::lexer::Word> {
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
    value_parts.push(WordPart::Literal {
        text: rest_of_first,
        quoted: false,
    });
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
pub fn is_assignment_word(w: &crate::lexer::Word) -> bool {
    use crate::lexer::WordPart;
    if matches!(w.0.first(), Some(WordPart::AssignPrefix { .. })) {
        return true;
    }
    let text = match w.0.first() {
        Some(WordPart::Literal {
            text,
            quoted: false,
        }) => text,
        _ => return false,
    };
    let Some(eq) = text.find('=') else {
        return false;
    };
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
    Word(vec![WordPart::Literal {
        text: s.to_string(),
        quoted: false,
    }])
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum RedirectSlot {
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
    Heredoc {
        body: Word,
        expand: bool,
        strip_tabs: bool,
    },
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
    ReadOnly,  // <     default fd 0
    Truncate,  // >     default fd 1
    Append,    // >>    default fd 1
    Clobber,   // >|    default fd 1
    ReadWrite, // <>    default fd 0
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum RedirOp {
    File {
        mode: FileMode,
        target: Word,
    },
    /// `>&w` (output=true) / `<&w` (output=false). `source` is an fd-number
    /// word; a `-` source is normalized to `Close` by the parser.
    Dup {
        source: Word,
        output: bool,
    },
    /// `N>&-` / `N<&-`.
    Close,
    /// `[n]>&digit-` (output) / `[n]<&digit-` (input): dup `source` onto the
    /// target fd, THEN close `source` (bash's "move fd"). `output` picks the
    /// directional default target fd (1 for `>&`, 0 for `<&`) and the `>&`/`<&`
    /// rendering.
    Move {
        source: Word,
        output: bool,
    },
    Heredoc {
        body: Word,
        expand: bool,
        strip_tabs: bool,
    },
    HereString(Word),
}

impl RedirOp {
    /// The fd this op targets when `RedirFd::Default` (no explicit prefix).
    pub fn default_fd(&self) -> u16 {
        match self {
            RedirOp::File {
                mode: FileMode::ReadOnly | FileMode::ReadWrite,
                ..
            } => 0,
            RedirOp::File { .. } => 1,
            RedirOp::Dup { output: true, .. } => 1,
            RedirOp::Dup { output: false, .. } => 0,
            RedirOp::Move { output: true, .. } => 1,
            RedirOp::Move { output: false, .. } => 0,
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
/// `RedirOp::Close`, `<&` (Dup output:false), and ReadWrite are DROPPED here;
/// these slots feed only the 0/1/2 pipe/stdio wiring — the pipeline path
/// separately replays the FULL ordered redirect list for external stages via
/// `build_child_redir_plan`/`ChildRedirPlan` (v292), so ordering and fd>2
/// heredocs are handled there, not by this helper.
///
/// The single-command builtin and external paths do NOT use this helper
/// anymore — they apply `cmd.redirects` in source order (so L-08 is fixed
/// there too).
pub fn slots_for_simple_path(
    redirs: &[Redirection],
) -> (
    Option<RedirectSlot>,
    Option<RedirectSlot>,
    Option<RedirectSlot>,
) {
    let (mut sin, mut sout, mut serr) = (None, None, None);
    for r in redirs {
        let Some(fd) = r.target_fd() else { continue };
        let legacy = match &r.op {
            RedirOp::File {
                mode: FileMode::ReadOnly,
                target,
            } => Some(RedirectSlot::Read(target.clone())),
            RedirOp::File {
                mode: FileMode::Truncate,
                target,
            } => Some(RedirectSlot::Truncate(target.clone())),
            RedirOp::File {
                mode: FileMode::Append,
                target,
            } => Some(RedirectSlot::Append(target.clone())),
            RedirOp::File {
                mode: FileMode::Clobber,
                target,
            } => Some(RedirectSlot::Clobber(target.clone())),
            RedirOp::File {
                mode: FileMode::ReadWrite,
                ..
            } => None,
            RedirOp::Dup {
                source,
                output: true,
            } => Some(RedirectSlot::Dup {
                fd: fd as i32,
                source: source.clone(),
            }),
            RedirOp::Dup { output: false, .. } | RedirOp::Close | RedirOp::Move { .. } => None,
            RedirOp::Heredoc {
                body,
                expand,
                strip_tabs,
            } => Some(RedirectSlot::Heredoc {
                body: body.clone(),
                expand: *expand,
                strip_tabs: *strip_tabs,
            }),
            RedirOp::HereString(w) => Some(RedirectSlot::HereString(w.clone())),
        };
        // Only fill a slot when the op direction matches the fd:
        //   stdin  (0): input ops only (ReadOnly, Heredoc, HereString)
        //   stdout (1) / stderr (2): output ops only (Truncate/Append/Clobber, Dup{output:true})
        // Cross-type combos (e.g. Read→fd1, Truncate→fd0) are dropped — they
        // would cause resolve()'s unreachable!() assertions to fire. They
        // become fully functional when the applier tasks are migrated.
        match fd {
            0 => {
                if matches!(
                    &r.op,
                    RedirOp::File {
                        mode: FileMode::ReadOnly,
                        ..
                    } | RedirOp::Heredoc { .. }
                        | RedirOp::HereString(_)
                ) {
                    sin = legacy;
                }
            }
            1 => {
                if matches!(
                    &r.op,
                    RedirOp::File {
                        mode: FileMode::Truncate | FileMode::Append | FileMode::Clobber,
                        ..
                    } | RedirOp::Dup { output: true, .. }
                ) {
                    sout = legacy;
                }
            }
            2 => {
                if matches!(
                    &r.op,
                    RedirOp::File {
                        mode: FileMode::Truncate | FileMode::Append | FileMode::Clobber,
                        ..
                    } | RedirOp::Dup { output: true, .. }
                ) {
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
            && let WordPart::Literal {
                text,
                quoted: false,
            } = &self.program.0[0]
        {
            return Some(text.clone());
        }
        None
    }

    /// The 0/1/2 redirect slots derived from `redirects` for the PIPELINE-STAGE
    /// fast-path (v156). The single-command builtin/external paths no longer use
    /// these — they apply `redirects` in source order. Last-wins, source-order
    /// NOT preserved (see `slots_for_simple_path`).
    pub fn slot_stdin(&self) -> Option<RedirectSlot> {
        slots_for_simple_path(&self.redirects).0
    }
    pub fn slot_stdout(&self) -> Option<RedirectSlot> {
        slots_for_simple_path(&self.redirects).1
    }
    pub fn slot_stderr(&self) -> Option<RedirectSlot> {
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
    FileExists,     // -e
    IsRegFile,      // -f
    IsDir,          // -d
    IsReadable,     // -r
    IsWritable,     // -w
    IsExecutable,   // -x
    IsNonEmpty,     // -s (non-empty file)
    IsSymlink,      // -L
    StringNonEmpty, // -n
    StringEmpty,    // -z
    VarSet,         // -v  (variable is set)
    /// `[[ -o NAME ]]` — true iff the `set -o` option NAME is enabled.
    OptEnabled, // -o
    IsFifo,         // -p
    IsSocket,       // -S
    IsBlockDev,     // -b
    IsCharDev,      // -c
    OwnedByEuid,    // -O
    OwnedByEgid,    // -G
    NewerThanRead,  // -N
    IsSticky,       // -k
    IsSetuid,       // -u
    IsSetgid,       // -g
    IsTerminal,     // -t
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
    Unary {
        op: TestUnaryOp,
        operand: Word,
    },
    Binary {
        op: TestBinaryOp,
        lhs: Word,
        rhs: Word,
    },
    Regex {
        lhs: Word,
        pattern: Word,
    },
    Not(Box<TestExpr>),
    And(Box<TestExpr>, Box<TestExpr>),
    Or(Box<TestExpr>, Box<TestExpr>),
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
    Subshell {
        body: Box<Sequence>,
    }, // NEW (v28): `(list)` subshell
    FunctionDef {
        name: String,
        body: Box<Command>,
        /// The `f() {`/`function f {` definition line (1-based, LOCAL to the
        /// parse — same convention as other `line: u32` fields, absolutized
        /// via `Shell::line_base()` by the engine). 0 when synthesized/unknown
        /// (mirrors `zero_lines_in_command`'s convention for other commands).
        /// v329 (#274): lets the engine fire the DEBUG trap on function ENTRY
        /// with `$LINENO` = this line, matching bash's function-tracing entry
        /// fire.
        line: u32,
    },
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
    Coproc {
        name: String,
        body: Box<Command>,
    },
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
    /// Source line of the `for` keyword (v325 #261), for stamping
    /// `$LINENO` before each per-iteration DEBUG-trap header fire. 0 when
    /// stripped by `zero_lines_in_command` (e.g. inside `$(...)`).
    pub line: u32,
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
    /// Source line of the `select` keyword (v325 #261), for stamping
    /// `$LINENO` before each per-iteration DEBUG-trap header fire. 0 when
    /// stripped by `zero_lines_in_command`.
    pub line: u32,
}

/// NEW (v78): a C-style `for ((init; cond; step)) do BODY done` clause.
/// Each header section is optional; an empty cond is treated as always-true.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ArithForClause {
    pub init: Option<crate::lexer::Word>,
    pub cond: Option<crate::lexer::Word>,
    pub step: Option<crate::lexer::Word>,
    pub body: Sequence,
    /// Source line of the `for` keyword (v325 #261), for stamping
    /// `$LINENO` before each init/cond/step DEBUG-trap header fire. 0 when
    /// stripped by `zero_lines_in_command`.
    pub line: u32,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct CaseClause {
    /// The word being matched — unexpanded.
    pub subject: Word,
    /// The clauses, in source order. May be empty.
    pub items: Vec<CaseItem>,
    /// Source line of the `case` keyword (v325 #261), for stamping
    /// `$LINENO` before the entry DEBUG-trap header fire. 0 when stripped
    /// by `zero_lines_in_command`.
    pub line: u32,
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

/// v314 (#211): what was found at the point a parse expectation failed —
/// either a concrete token, or end-of-input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Found {
    Token(crate::lexer::TokenKind),
    Eof,
}

/// v314 (#211): the opening delimiter (if any) an `ExpectFailure` is
/// unmatched against, for rendering bash's "unexpected EOF while looking
/// for matching `X'" shape downstream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delim {
    Paren,        // ( subshell
    Brace,        // { group
    DQuote,       // "
    SQuote,       // '
    Backtick,     // `
    DollarParen,  // $(
    DollarDParen, // $((
    DollarBrace,  // ${
    DBracket,     // [[
}

/// v314 (#211): captures the context of a parse expectation failure —
/// what was found, what (if anything) it was supposed to close, and where
/// in the input it occurred. Rendered into bash's near-token /
/// unexpected-EOF message shapes downstream (Task 3's engine renderer).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpectFailure {
    pub found: Found,
    pub matching: Option<Delim>,
    pub pos: usize,
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
    EmptySubshell,               // NEW (v28): `()` — empty subshell body
    UnterminatedSubshell,        // NEW (v28): `(cmd` with no closing `)`
    EmptyDoubleBracket,          // NEW (v30): `[[ ]]` — no expression
    UnterminatedDoubleBracket,   // NEW (v30): `[[ x == y` — missing `]]`
    TestExprBadOperator(String), // NEW (v30): unrecognised operator inside `[[ ]]`
    TestExprMissingOperand,      // NEW (v30): e.g. `[[ -f ]]` or `[[ x == ]]`
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
    /// v314 (#211): a syntax error carrying the offending token / EOF context,
    /// rendered into bash's near-token / unexpected-EOF shapes downstream.
    Unexpected(ExpectFailure),
    /// v316 (#213): a syntax error re-parsing a backtick command-substitution
    /// body. `inner` = the body error (already body-relative), `body` = the
    /// cooked backtick body (for the echo + body-local line), `err_pos` = the
    /// body-relative error offset. Rendered with `command substitution:`.
    InCommandSub {
        inner: Box<ParseError>,
        body: String,
        err_pos: usize,
    },
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
    let WordPart::Literal {
        text,
        quoted: false,
    } = &word.0[0]
    else {
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
            op: RedirOp::File {
                mode: FileMode::ReadOnly,
                target,
            },
        }],
        Operator::RedirOut => vec![Redirection {
            fd: plain_fd(),
            op: RedirOp::File {
                mode: FileMode::Truncate,
                target,
            },
        }],
        Operator::RedirAppend => vec![Redirection {
            fd: plain_fd(),
            op: RedirOp::File {
                mode: FileMode::Append,
                target,
            },
        }],
        Operator::RedirClobber => vec![Redirection {
            fd: plain_fd(),
            op: RedirOp::File {
                mode: FileMode::Clobber,
                target,
            },
        }],
        Operator::RedirReadWrite => vec![Redirection {
            fd: plain_fd(),
            op: RedirOp::File {
                mode: FileMode::ReadWrite,
                target,
            },
        }],
        Operator::RedirErr => vec![Redirection {
            fd: err_fd(),
            op: RedirOp::File {
                mode: FileMode::Truncate,
                target,
            },
        }],
        Operator::RedirErrAppend => vec![Redirection {
            fd: err_fd(),
            op: RedirOp::File {
                mode: FileMode::Append,
                target,
            },
        }],
        Operator::RedirErrClobber => vec![Redirection {
            fd: err_fd(),
            op: RedirOp::File {
                mode: FileMode::Clobber,
                target,
            },
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
                op: RedirOp::File {
                    mode: FileMode::Truncate,
                    target,
                },
            },
            Redirection {
                fd: RedirFd::Number(2),
                op: RedirOp::Dup {
                    source: lit_word("1"),
                    output: true,
                },
            },
        ],
        Operator::AndRedirAppend => vec![
            Redirection {
                fd: plain_fd(),
                op: RedirOp::File {
                    mode: FileMode::Append,
                    target,
                },
            },
            Redirection {
                fd: RedirFd::Number(2),
                op: RedirOp::Dup {
                    source: lit_word("1"),
                    output: true,
                },
            },
        ],
        // is_redirect_op gates the callers; no other operator reaches here.
        _ => unreachable!("build_redirections called with a non-redirect operator"),
    }
}

/// `>&w`/`<&w`: `-` closes; `<digits>-` moves (dup then close source);
/// otherwise a Dup.
pub(crate) fn dup_op(source: Word, output: bool) -> RedirOp {
    match word_literal_text(&source) {
        Some("-") => RedirOp::Close,
        Some(t) if is_move_operand(t) => RedirOp::Move {
            source: lit_word(&t[..t.len() - 1]),
            output,
        },
        _ => RedirOp::Dup { source, output },
    }
}

/// True for a bash move-fd operand: one or more ASCII digits then a single
/// trailing `-` (e.g. `5-`, `10-`). `-` alone is Close (handled by the caller).
fn is_move_operand(t: &str) -> bool {
    matches!(t.strip_suffix('-'), Some(digits) if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
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

// ──────────────────────────────────────────────────────────────
// [[ ]] extended test — Pratt-style parser (v30)
// ──────────────────────────────────────────────────────────────

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
        WordPart::Literal {
            text,
            quoted: false,
        } => Some(text.as_str()),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::WordPart;

    fn ww(s: &str) -> Word {
        Word(vec![WordPart::Literal {
            text: s.to_string(),
            quoted: false,
        }])
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
        assert_eq!(
            RedirOp::File {
                mode: FileMode::ReadOnly,
                target: w.clone()
            }
            .default_fd(),
            0
        );
        assert_eq!(
            RedirOp::File {
                mode: FileMode::Truncate,
                target: w.clone()
            }
            .default_fd(),
            1
        );
        assert_eq!(
            RedirOp::File {
                mode: FileMode::ReadWrite,
                target: w.clone()
            }
            .default_fd(),
            0
        );
        assert_eq!(
            RedirOp::Dup {
                source: ww("1"),
                output: true
            }
            .default_fd(),
            1
        );
        let r = Redirection {
            fd: RedirFd::Number(3),
            op: RedirOp::Close,
        };
        assert_eq!(r.target_fd(), Some(3));
        let v = Redirection {
            fd: RedirFd::Var("x".into()),
            op: RedirOp::Close,
        };
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
            op: RedirOp::File {
                mode: FileMode::ReadOnly,
                target: ww("f"),
            },
        };
        let (sin, sout, serr) = slots_for_simple_path(&[r_read_on_fd1]);
        assert!(sin.is_none(), "Read on fd1 must not fill stdin");
        assert!(sout.is_none(), "Read on fd1 must not fill stdout");
        assert!(serr.is_none(), "Read on fd1 must not fill stderr");

        // Truncate-op on fd 0 (stdin): must not fill any slot.
        let r_trunc_on_fd0 = Redirection {
            fd: RedirFd::Number(0),
            op: RedirOp::File {
                mode: FileMode::Truncate,
                target: ww("f"),
            },
        };
        let (sin, sout, serr) = slots_for_simple_path(&[r_trunc_on_fd0]);
        assert!(sin.is_none(), "Truncate on fd0 must not fill stdin");
        assert!(sout.is_none(), "Truncate on fd0 must not fill stdout");
        assert!(serr.is_none(), "Truncate on fd0 must not fill stderr");

        // Dup{output:true} on fd 0: must not fill any slot.
        let r_dup_out_on_fd0 = Redirection {
            fd: RedirFd::Number(0),
            op: RedirOp::Dup {
                source: ww("1"),
                output: true,
            },
        };
        let (sin, sout, serr) = slots_for_simple_path(&[r_dup_out_on_fd0]);
        assert!(sin.is_none(), "Dup(output) on fd0 must not fill stdin");
        assert!(sout.is_none(), "Dup(output) on fd0 must not fill stdout");
        assert!(serr.is_none(), "Dup(output) on fd0 must not fill stderr");

        // Read-op on fd 2 (stderr): must not fill any slot.
        let r_read_on_fd2 = Redirection {
            fd: RedirFd::Number(2),
            op: RedirOp::File {
                mode: FileMode::ReadOnly,
                target: ww("f"),
            },
        };
        let (sin, sout, serr) = slots_for_simple_path(&[r_read_on_fd2]);
        assert!(sin.is_none(), "Read on fd2 must not fill stdin");
        assert!(sout.is_none(), "Read on fd2 must not fill stdout");
        assert!(serr.is_none(), "Read on fd2 must not fill stderr");

        // Sanity: direction-matched ops still fill slots correctly.
        let r_read_fd0 = Redirection {
            fd: RedirFd::Number(0),
            op: RedirOp::File {
                mode: FileMode::ReadOnly,
                target: ww("f"),
            },
        };
        let r_trunc_fd1 = Redirection {
            fd: RedirFd::Number(1),
            op: RedirOp::File {
                mode: FileMode::Truncate,
                target: ww("g"),
            },
        };
        let r_trunc_fd2 = Redirection {
            fd: RedirFd::Number(2),
            op: RedirOp::File {
                mode: FileMode::Truncate,
                target: ww("h"),
            },
        };
        let (sin, sout, serr) = slots_for_simple_path(&[r_read_fd0, r_trunc_fd1, r_trunc_fd2]);
        assert!(sin.is_some(), "Read on fd0 should fill stdin");
        assert!(sout.is_some(), "Truncate on fd1 should fill stdout");
        assert!(serr.is_some(), "Truncate on fd2 should fill stderr");
    }

    // ── coproc parser tests (v157 task 2) ─────────────────────────────────

    #[test]
    fn dup_op_classifies_close_move_dup() {
        assert!(matches!(dup_op(lit_word("-"), true), RedirOp::Close));
        assert!(matches!(
            dup_op(lit_word("5-"), false),
            RedirOp::Move { output: false, .. }
        ));
        assert!(matches!(
            dup_op(lit_word("10-"), true),
            RedirOp::Move { output: true, .. }
        ));
        assert!(matches!(
            dup_op(lit_word("5"), true),
            RedirOp::Dup { output: true, .. }
        ));
        // A non-numeric source ending in `-` stays a Dup (bash: move needs digits).
        assert!(matches!(dup_op(lit_word("x-"), true), RedirOp::Dup { .. }));
    }

    // ── v314 (#211) ExpectFailure capture types ─────────────────────────────

    #[test]
    fn expect_failure_roundtrips() {
        use crate::lexer::{Operator, TokenKind};
        let e = ParseError::Unexpected(ExpectFailure {
            found: Found::Token(TokenKind::Op(Operator::RParen)),
            matching: None,
            pos: 5,
        });
        assert!(matches!(e, ParseError::Unexpected(ref f) if f.pos == 5));
    }
}
