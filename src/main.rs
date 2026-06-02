mod alias_expand;
mod arith;
mod brace_expand;
mod builtins;
mod command;
mod completion;
mod completion_spec;
mod continuation;
mod executor;
mod expand;
mod history;
mod job_spec;
mod jobs;
mod lexer;
mod param_expansion;
mod prompt;
mod shell;
mod shell_state;
mod test_builtin;
mod traps;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(shell::run(&args));
}
