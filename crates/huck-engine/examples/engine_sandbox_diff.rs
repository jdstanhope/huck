//! Driver for the `engine_sandbox_diff_check.sh` bash-diff harness.
//!
//! Argv: `<mode> <fragment>` where mode is:
//!   - `bare`           — `.capture()` only.
//!   - `restricted`     — `.restricted(true).capture()`.
//!   - `cwd:<path>`     — `.cwd(<path>).capture()`.
//!   - `cwd:<path>:r`   — `.cwd(<path>).restricted(true).capture()`.
//!
//! Output format (same as engine_capture_diff):
//!   `STDOUT:<n>\n<bytes>STDERR:<n>\n<bytes>EXIT:<code>\n`

use huck_engine::Engine;
use std::io::Write;
use std::path::PathBuf;

fn main() {
    let mut args = std::env::args().skip(1);
    let mode = args.next().expect("mode arg");
    let fragment = args.next().expect("fragment arg");

    let mut e = Engine::new();
    let out = match mode.as_str() {
        "bare" => e.prepare(&fragment).capture(),
        "restricted" => e.prepare(&fragment).restricted(true).capture(),
        m if m.starts_with("cwd:") => {
            let body = &m[4..];
            let (path, restricted) = if let Some(stripped) = body.strip_suffix(":r") {
                (PathBuf::from(stripped), true)
            } else {
                (PathBuf::from(body), false)
            };
            let mut b = e.prepare(&fragment).cwd(path);
            if restricted {
                b = b.restricted(true);
            }
            b.capture()
        }
        _ => panic!("unknown mode: {mode}"),
    };

    let stdout = std::io::stdout();
    let mut h = stdout.lock();
    writeln!(h, "STDOUT:{}", out.stdout.len()).unwrap();
    h.write_all(out.stdout.as_bytes()).unwrap();
    writeln!(h, "STDERR:{}", out.stderr.len()).unwrap();
    h.write_all(out.stderr.as_bytes()).unwrap();
    writeln!(h, "EXIT:{}", out.exit_code).unwrap();
}
