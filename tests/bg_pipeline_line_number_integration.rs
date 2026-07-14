//! v294 (#69, background half): a redirect-open failure on a BACKGROUND
//! pipeline stage (`… &`, routed through `run_background_sequence`) reports the
//! stage's physical line number (`<script>: line N: …`), matching bash.
//!
//! The error is emitted synchronously in the parent while lowering the stage's
//! redirects (`build_child_redir_plan` opens files before the child forks), so
//! a subprocess `huck <file>` run captures it deterministically even though the
//! job itself detaches. A bare `pipeline &` (no trailing `wait`/`;`) is required
//! to hit `run_background_sequence`; adding `wait` would route through a
//! different dispatch path.
use std::io::Write;
use std::process::Command;

fn huck_bin() -> &'static str {
    env!("CARGO_BIN_EXE_huck")
}

/// Writes `body` to a temp file, runs `huck <file>` with stdin severed (so a
/// backgrounded first stage cannot inherit the harness's stdin), returns stderr.
fn run_script_stderr(body: &str) -> String {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.sh");
    std::fs::File::create(&path)
        .unwrap()
        .write_all(body.as_bytes())
        .unwrap();
    let out = Command::new(huck_bin())
        .arg(&path)
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn bg_pipeline_stage_redirect_error_reports_line() {
    // The redirect-open failure is on line 3 (the backgrounded pipeline).
    let se = run_script_stderr(": one\n: two\ncat </no/such/dir/nope | wc -l >out &\n");
    assert!(
        se.contains("line 3:"),
        "expected 'line 3:' from a bg stage redirect error, got: {se:?}"
    );
    assert!(
        se.contains("/no/such/dir/nope"),
        "expected the failing path in the message, got: {se:?}"
    );
    assert!(
        se.contains("No such file or directory"),
        "expected the open-failure reason, got: {se:?}"
    );
}
