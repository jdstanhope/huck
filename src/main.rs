mod builtins;
mod command;
mod executor;
mod lexer;
mod shell;

fn main() {
    std::process::exit(shell::run());
}
