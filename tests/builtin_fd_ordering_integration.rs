//! End-to-end tests for v156 task 7: arbitrary-fd / source-ordered redirections
//! on BARE BUILTINS (the in-process path). Task 7 migrated the builtin path off
//! the last-wins 0/1/2 bridge onto the single ordered `RedirectScope` applier, so
//! `echo x 2>&1 >file` is now source-ordered exactly like compounds/externals
//! (L-08 fully fixed for bare builtins). Each test compares `huck -c <script>` to
//! `bash -c <script>` for byte-identical stdout.

use std::process::Command;

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

/// Run `prog -c script` and return its stdout.
fn run_c(prog: &str, script: &str) -> String {
    let out = Command::new(prog)
        .arg("-c")
        .arg(script)
        .output()
        .expect("spawn");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Assert huck's stdout matches bash's for the same script. Scripts here funnel
/// the observable result onto stdout (e.g. by `cat`-ing a redirect target file)
/// so that stderr-vs-file routing is captured by a stdout-only comparison.
fn assert_matches_bash(script: &str) {
    let bash_out = run_c("bash", script);
    let huck_out = run_c(&huck_binary(), script);
    assert_eq!(
        huck_out, bash_out,
        "stdout differs for script: {script}\n bash: {bash_out:?}\n huck: {huck_out:?}"
    );
}

/// L-08 for a bare builtin: `2>&1 >FILE` must send stderr to the terminal and
/// stdout to FILE (source order), so FILE contains ONLY the builtin's stdout.
/// `printf "%d\n" OUT` writes `0` to stdout and an "invalid number" diagnostic to
/// stderr; with `2>&1 >FILE` the file must hold only `0` (the error went to the
/// terminal, not the file). Pre-task-7 (last-wins bridge) both streams landed in
/// the file. We compare the FILE content (huck vs bash) — its stdout-only nature
/// makes this exact regardless of the differing diagnostic text.
#[test]
fn l08_builtin_2dup1_then_redirect_file() {
    let f = format!("/tmp/huck_l08b_{}", std::process::id());
    // Run the noisy `printf` inside a group whose own fd 1 is /dev/null, so the
    // `2>&1`-captured diagnostic (which differs in text: `huck:` vs `bash:`) is
    // discarded and does NOT leak into our stdout comparison. Then report ONLY
    // the file content. The discriminator is the file: it must contain ONLY `0`
    // (stdout went to FILE, stderr to the group's fd 1 = /dev/null) — NOT the
    // diagnostic. Pre-task-7 the last-wins bridge put BOTH into the file.
    let script = format!(
        "{{ printf '%d\\n' OUT 2>&1 >{f}; }} >/dev/null; printf 'FILE=[%s]' \"$(cat {f})\"; rm -f {f}"
    );
    assert_matches_bash(&script);
}

/// Reverse order `>FILE 2>&1`: both stdout and stderr go to FILE. We compare only
/// the `0` line by counting it, since the diagnostic text differs (`huck:` vs
/// `bash: line 1:`). Here we just assert the stdout `0` reached the file in both.
#[test]
fn l08_builtin_redirect_file_then_2dup1_stdout_in_file() {
    let f = format!("/tmp/huck_l08c_{}", std::process::id());
    // Extract just the numeric stdout line from the file (grep -c) so the
    // differing diagnostic text doesn't affect the comparison.
    let script = format!(
        "printf '%d\\n' OUT >{f} 2>&1; printf 'N=[%s]' \"$(grep -c '^0$' {f})\"; rm -f {f}"
    );
    assert_matches_bash(&script);
}

/// `echo` bare builtin with `2>&1 >FILE`: echo writes no stderr, so the file
/// holds its line either way — this guards the common case stays correct.
#[test]
fn echo_builtin_2dup1_then_file() {
    let f = format!("/tmp/huck_l08e_{}", std::process::id());
    let script = format!("echo hello 2>&1 >{f}; cat {f}; rm -f {f}");
    assert_matches_bash(&script);
}

/// `read` builtin's stdin redirect (`< FILE`) must actually feed fd 0 it reads.
#[test]
fn read_builtin_stdin_from_file() {
    let f = format!("/tmp/huck_l08r_{}", std::process::id());
    let script = format!(
        "printf 'alpha beta\\n' >{f}; read a b <{f}; printf '[%s|%s]' \"$a\" \"$b\"; rm -f {f}"
    );
    assert_matches_bash(&script);
}

/// `read` builtin from a here-string.
#[test]
fn read_builtin_herestring() {
    assert_matches_bash("read a b <<< 'one two'; printf '[%s|%s]' \"$a\" \"$b\"");
}

/// A bare builtin writing to a scratch fd>2 in-process (`echo >&3` with `3>FILE`).
#[test]
fn builtin_writes_to_extra_fd() {
    let f = format!("/tmp/huck_l08x_{}", std::process::id());
    let script = format!("echo viafd3 >&3 3>{f}; cat {f}; rm -f {f}");
    assert_matches_bash(&script);
}

/// Capture mode still captures a bare builtin's stdout when there is no stdout
/// redirect: `r=$(echo captured)`.
#[test]
fn capture_builtin_stdout_no_redirect() {
    assert_matches_bash("r=$(echo captured); printf '[%s]' \"$r\"");
}

/// `>&-` (close stdout) on a bare builtin in capture mode discards its output,
/// matching bash (the closed fd 1 wins over the capture buffer).
#[test]
fn builtin_close_stdout_discards_in_capture() {
    assert_matches_bash("r=$(echo GONE >&-); printf '[%s]' \"$r\"");
}
