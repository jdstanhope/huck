//! Serial isolation for a redirect that reads back through the real fd 1 (#458).
//!
//! `{ echo HI; } > file` dup2s the process-global fd 1 onto the target file for
//! the duration of the group. Under the parallel `--lib` harness, libtest's own
//! progress output for a concurrently finishing test lands on that same
//! descriptor and ends up INSIDE the file.
//!
//! The test used to live in `executor/tests.rs` and tolerate the noise by
//! asserting only that some line equalled `HI`. That cannot work: libtest emits
//! `test NAME ... ` and `ok\n` as separate writes, so the payload can be glued
//! onto one mid-line and no line equals `HI` —
//!
//! ```text
//! got "ok\ntest executor::tests::default_readonly_for_var_is_not_fatal ... okHI\n\n"
//! ```
//!
//! Line-granular tolerance cannot survive interleaving that splits inside a
//! line, so the fix is isolation, not a better assertion. Being the sole
//! `#[test]` in this binary, no sibling output exists to leak while fd 1 is
//! swapped — which is what lets the assertion below be EXACT. (Precedent: #90 /
//! #184 / #297; `capture_tempfile_serial.rs`, `forking_execution_serial.rs`.)

use huck_engine::Engine;

/// Smoke-test for `with_redirect_scope`: a brace group redirected to a file
/// writes its output there, and nothing else does.
#[test]
fn compound_stdout_redirect_writes_to_file() {
    let dir = std::env::temp_dir().join(format!("huck_redir_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    let p = dir.join("out.txt");
    let _ = std::fs::remove_file(&p);

    let mut e = Engine::new();
    let code = e.run(&format!("{{ echo HI; }} > {}\n", p.display()));
    assert_eq!(code, 0, "the redirected group should succeed");

    let content = std::fs::read_to_string(&p).expect("redirect target file should exist");
    // Exact, not `lines().any(...)`: with no concurrent writer on fd 1 the file
    // holds the group's output and nothing else. A regression that lets other
    // bytes reach the descriptor now fails here instead of being tolerated.
    assert_eq!(content, "HI\n");

    let _ = std::fs::remove_file(&p);
    let _ = std::fs::remove_dir(&dir);
}
