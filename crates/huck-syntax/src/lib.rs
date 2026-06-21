//! `huck-syntax` — huck's Shell-free frontend.
//!
//! Contains the lexer, the command AST + parser, brace expansion, and the
//! AST->source generator. This crate MUST NOT depend on the `huck` runtime
//! crate (the dependency direction is enforced by Cargo: a cycle won't compile).

pub mod brace_expand;
pub mod command;
pub mod errors;
pub mod generate;
pub mod lexer;
pub mod util;

pub use errors::{lex_error_message, parse_error_message};
pub use util::escape_double_quote_value;
