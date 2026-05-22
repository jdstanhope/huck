mod arith;
mod builtins;
mod command;
mod completion;
mod continuation;
mod executor;
mod expand;
mod history;
mod job_spec;
mod jobs;
mod lexer;
mod param_expansion;
mod shell;
mod shell_state;
mod test_builtin;

fn main() {
    std::process::exit(shell::run());
}
