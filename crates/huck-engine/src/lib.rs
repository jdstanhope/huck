//! `huck-engine` — huck's terminal-free execution core.
//!
//! Parses (via `huck-syntax`), expands, and executes shell scripts/commands
//! with NO terminal/line-editor dependency. MUST NOT depend on `rustyline` —
//! the REPL + line-editor adapters live in `huck-cli`.

#[macro_use]
mod macros;
pub use macros::bash_io_error;

pub(crate) mod arith;
pub(crate) mod array_transforms;
#[doc(hidden)]
pub mod builtins;
pub(crate) mod callbacks_thread_local;
pub(crate) mod child_fd;
#[doc(hidden)]
pub mod completion;
pub(crate) mod completion_builtins;
pub(crate) mod completion_spec;
#[doc(hidden)]
pub mod continuation;
pub(crate) mod cwd_scope;
pub mod engine;
pub(crate) mod err_thread_local;
pub mod error_emit;
pub mod exec_builder;
pub(crate) mod executor;
pub(crate) mod expand;
pub(crate) mod glob_match;
#[doc(hidden)]
pub mod history;
pub(crate) mod job_spec;
#[doc(hidden)]
pub mod jobs;
pub(crate) mod line_buf;
pub(crate) mod param_expansion;
pub(crate) mod procsub;
#[doc(hidden)]
pub mod prompt;
#[doc(hidden)]
pub mod readline_bind;
pub(crate) mod restricted;
#[doc(hidden)]
pub mod shell;
#[doc(hidden)]
pub mod shell_state;
pub(crate) mod stdin_pipe;
pub(crate) mod stream_loop;
pub(crate) mod test_builtin;
pub(crate) mod timeout;
#[doc(hidden)]
pub mod traps;
pub(crate) mod wait_loop;

#[cfg(test)]
pub mod test_support;

pub use completion::{Candidate, CandidateKind};
pub use engine::{Completion, Engine, EngineBuilder, Output};
pub use error_emit::{Diag, emit_cli_error, emit_error, emit_error_to, emit_syntax_error};
pub use exec_builder::ExecBuilder;
pub use executor::{StderrSink, StdoutSink};

// Re-export the frontend so `huck_engine::lexer::`/`::command::` resolve downstream.
pub use huck_syntax::escape_double_quote_value;
pub use huck_syntax::{brace_expand, command, generate, lexer, parser};
