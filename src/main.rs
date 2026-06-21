//! huck — thin binary shim. All logic lives in `huck-cli` (REPL) over
//! `huck-engine` (execution) over `huck-syntax` (frontend).
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(huck_cli::run(&args));
}
