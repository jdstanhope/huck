//! `huck-engine` — huck's terminal-free execution core.
//!
//! Parses (via `huck-syntax`), expands, and executes shell scripts/commands
//! with NO terminal/line-editor dependency. MUST NOT depend on `rustyline` —
//! the REPL + line-editor adapters live in `huck-cli`.

pub mod alias_expand;
pub mod arith;
pub mod builtins;
pub mod completion;
pub mod completion_builtins;
pub mod completion_spec;
pub mod continuation;
pub mod engine;
pub mod executor;
pub mod expand;
pub mod glob_match;
pub mod history;
pub mod job_spec;
pub mod jobs;
pub mod param_expansion;
pub mod procsub;
pub mod prompt;
pub mod readline_bind;
pub mod shell;
pub mod shell_state;
pub mod test_builtin;
pub mod traps;

#[cfg(test)]
pub mod test_support;

pub use engine::{Engine, EngineBuilder, Output};

// Re-export the frontend so `huck_engine::lexer::`/`::command::` resolve downstream.
pub use huck_syntax::{brace_expand, command, generate, lexer};
pub use huck_syntax::{escape_double_quote_value, lex_error_message, parse_error_message};
