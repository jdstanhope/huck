mod builtins;
mod command;
mod executor;
mod expand;
mod lexer;
mod shell;
mod shell_state;

fn main() {
    std::process::exit(shell::run());
}
