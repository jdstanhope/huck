//! `huck-engine` — huck's terminal-free execution core.
//!
//! Parses (via `huck-syntax`), expands, and executes shell scripts/commands
//! with NO terminal/line-editor dependency. MUST NOT depend on `rustyline` —
//! the REPL + line-editor adapters live in `huck-cli`.
