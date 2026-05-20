mod arith;
mod builtins;
mod command;
mod completion;
mod executor;
mod expand;
mod history;
mod job_spec;
mod jobs;
mod lexer;
mod param_expansion;
mod shell;
mod shell_state;

fn main() {
    std::process::exit(shell::run());
}
