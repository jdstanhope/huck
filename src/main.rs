//! huck — thin binary shim. All logic lives in `huck-cli` (REPL) over
//! `huck-engine` (execution) over `huck-syntax` (frontend).
fn main() {
    // `args_os`, not `args`: the UTF-8 iterator unwraps internally, so one
    // non-UTF-8 byte in argv panicked the shell before it ran a line (#553).
    // Lossy here; carrying the raw bytes needs a byte-string value type
    // through `Shell`, which is the rest of that issue.
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    std::process::exit(huck_cli::run(&args, env!("CARGO_PKG_VERSION")));
}
