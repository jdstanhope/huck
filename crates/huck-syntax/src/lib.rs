//! `huck-syntax` — huck's Shell-free frontend.
//!
//! Contains the lexer, the command AST + parser, brace expansion, and the
//! AST->source generator. This crate MUST NOT depend on the `huck` runtime
//! crate (the dependency direction is enforced by Cargo: a cycle won't compile).
