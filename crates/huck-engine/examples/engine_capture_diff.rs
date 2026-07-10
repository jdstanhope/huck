//! Driver for the `engine_capture_diff_check.sh` bash-diff harness.
//!
//! Reads two args from argv: the mode (`split` | `merged`) and the fragment.
//! Runs the fragment through [`Engine`] with the matching mode and prints:
//!   `STDOUT:<n>\n<bytes>STDERR:<n>\n<bytes>EXIT:<code>\n`
//! The harness diffs huck's output against the equivalent bash run.

use huck_engine::Engine;
use std::io::Write;

fn main() {
    let mut args = std::env::args().skip(1);
    let mode = args.next().expect("mode arg");
    let fragment = args.next().expect("fragment arg");

    let mut e = Engine::new();
    let out = if mode == "merged" {
        e.prepare(&fragment).merge_stderr().capture()
    } else {
        e.prepare(&fragment).capture()
    };

    let stdout = std::io::stdout();
    let mut h = stdout.lock();
    writeln!(h, "STDOUT:{}", out.stdout.len()).unwrap();
    h.write_all(out.stdout.as_bytes()).unwrap();
    writeln!(h, "STDERR:{}", out.stderr.len()).unwrap();
    h.write_all(out.stderr.as_bytes()).unwrap();
    writeln!(h, "EXIT:{}", out.exit_code).unwrap();
}
