//! Thin binary shim. All shell logic lives in the `huck` library crate
//! (`src/lib.rs`); this just parses argv and hands off to [`huck::shell::run`].

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(huck::shell::run(&args));
}
