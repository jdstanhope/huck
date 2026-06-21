//! `huck-cli` — huck's interactive REPL + rustyline adapters (see crate docs).
mod completion_helper;
mod readline_apply;
mod repl;

pub use repl::run;
