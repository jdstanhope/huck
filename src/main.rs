mod alias_expand;
mod arith;
mod brace_expand;
mod builtins;
mod command;
mod completion;
mod completion_builtins;
mod completion_spec;
mod continuation;
mod executor;
mod expand;
mod generate;
mod glob_match;
mod history;
mod job_spec;
mod jobs;
mod lexer;
mod param_expansion;
mod prompt;
mod procsub;
mod shell;
mod shell_state;
mod test_builtin;
mod traps;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(shell::run(&args));
}

/// Shared test-only synchronization primitives. Tests across multiple
/// modules mutate process-global state (CWD, env, FDs); without a shared
/// lock they race under `cargo test`'s default parallel runner. The
/// pattern is `let _g = test_support::CWD_LOCK.lock().unwrap();` at the
/// top of any test that calls `std::env::set_current_dir`.
/// (Placed after `main` so it is the last item — a `#[cfg(test)]` module
/// followed by other items trips `clippy::items_after_test_module`.)
#[cfg(test)]
pub(crate) mod test_support {
    use std::sync::Mutex;
    pub(crate) static CWD_LOCK: Mutex<()> = Mutex::new(());
}
