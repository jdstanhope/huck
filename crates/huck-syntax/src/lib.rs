//! # huck-syntax
//!
//! Shell-free frontend for the [huck](https://github.com/jdstanhope/huck)
//! POSIX-ish shell: lexer, command-AST parser, brace expansion, and
//! source generator. Re-usable as a standalone library for shell
//! parsing, linting, and tooling.
//!
//! ## Pipeline
//!
//! ```text
//! source bytes  ->  tokenize  ->  parse        ->  walk / regenerate
//! &str              Vec<Token>    Option<Sequence>  Command tree
//! ```
//!
//! ## Quick example
//!
//! ```rust
//! use huck_syntax::parser::parse_sequence;
//! use huck_syntax::lexer::{Lexer, LexerOptions};
//!
//! let src = "echo hello";
//! let mut lx = Lexer::new(src, &Default::default(), LexerOptions::default());
//! let seq = parse_sequence(&mut lx).expect("parse").expect("non-empty");
//! // The Sequence has a first Command and no trailing commands.
//! assert!(seq.rest.is_empty());
//! assert!(!seq.background);
//! ```
//!
//! For richer examples — token dumping, AST walking, assignment
//! extraction — see the `examples/` directory:
//!
//! - `cargo run --example tokenize_dump -p huck-syntax`
//! - `cargo run --example list_assignments -p huck-syntax`
//!
//! ## Crate layout
//!
//! - [`lexer`] — bytes -> tokens + `Word` AST.
//! - [`command`] — tokens -> command AST (`Sequence` / `Command`).
//! - [`generate`] — AST -> source bytes (canonical round-trip).
//! - [`brace_expand`] — standalone brace expansion (`a{1,2}b` -> words).
//! - [`errors`] — human-readable error message rendering. The
//!   `Display` impls on `LexError` / `ParseError` / `BraceError` are
//!   the canonical surface; the free functions kept here are
//!   convenience wrappers.
//!
//! ## Stability
//!
//! The AST enums (`Token`, `WordPart`, `ParamModifier`, `TransformOp`,
//! `Command`, `ParseError`, `LexError`, `BraceError`) are marked
//! `#[non_exhaustive]`. Downstream consumers MUST use `_ =>` arms
//! when matching, so new variants in future huck releases are not
//! SemVer-breaking.
//!
//! This crate has NO dependency on huck's runtime; it is buildable
//! and consumable on its own. The dependency direction is enforced
//! by Cargo (no cycle).

pub mod brace_expand;
pub mod command;
pub mod errors;
pub mod generate;
pub mod lexer;
pub mod parser;
pub mod util;

// --- curated root re-exports ----------------------------------------
// External consumers can `use huck_syntax::{Word, parse}` instead of
// hunting through six modules. Module paths remain valid for the few
// types not re-exported here.

// Note: `brace_expand::expand` is intentionally NOT re-exported at the
// root — it would collide with the module name (function + module in the
// same namespace produces a doc warning and is confusing). Users call
// `huck_syntax::brace_expand::expand(...)` explicitly.
pub use brace_expand::BraceError;
pub use command::{
    ArithForClause, AssignTarget, Assignment, CaseClause, CaseItem, CaseTerminator, Command,
    Connector, ElifBranch, ExecCommand, FileMode, ForClause, IfClause, ParseError, Pipeline,
    RedirFd, RedirOp, RedirectSlot, Redirection, SelectClause, Sequence, SimpleCommand,
    TestBinaryOp, TestExpr, TestUnaryOp, WhileClause, try_split_assignment,
};
pub use generate::{command_to_source, function_to_source};
pub use lexer::{
    ArrayLiteralElement, CaseDirection, LexError, Lexer, LexerOptions, ParamModifier, ProcDir,
    QuoteStyle, Span, SubscriptKind, SubstAnchor, TildeSpec, Token, TokenKind, TransformOp, Word,
    WordPart,
};
pub use parser::{parse, parse_sequence};
pub use util::escape_double_quote_value;
