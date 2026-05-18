mod builtins;
mod command;
mod executor;
mod expand;
mod job_spec;
mod jobs;
mod lexer;
mod shell;
mod shell_state;

fn main() {
    std::process::exit(shell::run());
}
