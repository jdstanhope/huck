//! Self-consistency driver for v207 streaming. Runs the same fragment twice —
//! once with no callback (`Output.stdout`), once with a string-accumulating
//! `on_stdout_line` callback — and emits both, so the bash harness can verify
//! they agree.
//!
//! Argv: `<mode> <fragment>` where mode is `cap` or `stream`.
//!
//! Output format (matches the v205 capture protocol, stdout only):
//!   `STDOUT:<n>\n<bytes>EXIT:<code>\n`

use huck_engine::Engine;
use std::io::Write;

fn main() {
    let mut args = std::env::args().skip(1);
    let mode = args.next().expect("mode arg (cap | stream)");
    let fragment = args.next().expect("fragment arg");

    let mut e = Engine::new();
    let (stdout_bytes, exit_code) = match mode.as_str() {
        "cap" => {
            let out = e.exec(&fragment).capture();
            (out.stdout.into_bytes(), out.exit_code)
        }
        "stream" => {
            let mut acc = String::new();
            let out = e
                .exec(&fragment)
                .on_stdout_line(|line| {
                    acc.push_str(line);
                    acc.push('\n');
                })
                .capture();
            // NOTE: a trailing partial line (no \n at EOF) gets a synthetic \n
            // appended here; test fragments end in \n so this edge doesn't fire.
            (acc.into_bytes(), out.exit_code)
        }
        _ => panic!("unknown mode: {mode}"),
    };

    let stdout = std::io::stdout();
    let mut h = stdout.lock();
    writeln!(h, "STDOUT:{}", stdout_bytes.len()).unwrap();
    h.write_all(&stdout_bytes).unwrap();
    writeln!(h, "EXIT:{}", exit_code).unwrap();
}
