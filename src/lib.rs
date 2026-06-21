//! huck — a POSIX-ish shell, exposed as a library so the lex→parse→AST
//! frontend and the leaf utilities (glob matching, brace/arith expansion,
//! the `test` evaluator) can be reused, and so the whole crate is
//! library-testable. The binary (`src/main.rs`) is a thin shim over
//! [`shell::run`].
//!
//! The Shell-free frontend (`lexer`, `command` AST+parser, `brace_expand`,
//! `generate`) lives in the sibling **`huck-syntax`** workspace crate and is
//! re-exported at this crate's root, so `crate::lexer::`/`crate::command::`
//! paths resolve here unchanged. The remaining runtime modules are published
//! as-is; the public surface may be curated further in later iterations.

pub mod alias_expand;
pub mod arith;
pub mod builtins;
pub mod completion;
pub mod completion_builtins;
pub mod completion_spec;
pub mod continuation;
pub mod executor;
pub mod expand;
pub mod glob_match;
pub mod history;
pub mod job_spec;
pub mod jobs;
pub mod param_expansion;
pub mod prompt;
pub mod procsub;
pub mod readline_bind;
pub mod shell;
pub mod shell_state;
pub mod test_builtin;
pub mod traps;

// Frontend modules live in the `huck-syntax` crate; re-export at the crate root
// so existing `crate::lexer::`/`crate::command::`/`crate::generate::` paths and
// the relocated helpers resolve unchanged across the runtime.
pub use huck_syntax::{brace_expand, command, generate, lexer};
pub use huck_syntax::{escape_double_quote_value, lex_error_message, parse_error_message};
