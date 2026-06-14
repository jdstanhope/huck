//! End-to-end tests for v156 task 5: the bash 4.1+ `{var}>file` named-fd form.
//! A `{var}` redirection allocates a free fd >= 10, wires the redirect onto it,
//! and (for in-process commands) assigns the allocated number to `$var` so it
//! PERSISTS after the command. The allocated fd is non-CLOEXEC so an external
//! child inherits it open.
//!
//! Each test compares `huck -c <script>` to `bash -c <script>`. The literal fd
//! NUMBER can differ between shells (both >= 10), so number-bearing assertions
//! use `-ge 10` rather than the literal; file content is byte-compared.
//!
//! `exec {var}>file` (persistent fd) is v156 task 6 — these tests use only the
//! non-exec forms.

use std::process::Command;

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

/// Run `prog -c script` and return (stdout, exit_code).
fn run_c(prog: &str, script: &str) -> (String, i32) {
    let out = Command::new(prog).arg("-c").arg(script).output().expect("spawn");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

/// Assert huck's stdout matches bash's for the same script.
fn assert_matches_bash(script: &str) {
    let (bash_out, _) = run_c("bash", script);
    let (huck_out, _) = run_c(&huck_binary(), script);
    assert_eq!(
        huck_out, bash_out,
        "stdout differs for script: {script}\n bash: {bash_out:?}\n huck: {huck_out:?}"
    );
}

#[test]
fn in_process_compound_named_fd_writes_file_and_persists_var() {
    // `{ echo hi >&$fd; } {fd}>file` writes `hi` to the file, and $fd PERSISTS
    // after the command as a number >= 10. The number can differ between shells,
    // so the script normalizes it to a deterministic token via `-ge 10`.
    let f = format!("/tmp/huck_named_v_{}", std::process::id());
    let script = format!(
        "{{ echo hi >&$fd; }} {{fd}}>{f}; cat {f}; \
         if [ \"$fd\" -ge 10 ]; then echo 'fd-ge-10'; else echo 'fd-bad'; fi; rm -f {f}"
    );
    assert_matches_bash(&script);
}

#[test]
fn in_process_builtin_named_fd_persists_var() {
    // A bare in-process builtin (`:`) with `{fd}>file` still allocates the fd and
    // persists $fd >= 10 (bash leaves $fd set after a builtin command).
    let f = format!("/tmp/huck_named_b_{}", std::process::id());
    let script = format!(
        ": {{fd}}>{f}; if [ \"$fd\" -ge 10 ]; then echo ok; else echo bad; fi; rm -f {f}"
    );
    assert_matches_bash(&script);
}

#[test]
fn named_fd_input_read() {
    // `{fd}<file` opens the file for reading on the allocated fd; `cat <&$fd`
    // reads it back in-process.
    let f = format!("/tmp/huck_named_in_{}", std::process::id());
    let script = format!(
        "printf 'L1\\nL2\\n' > {f}; {{ cat <&$fd; }} {{fd}}<{f}; \
         if [ \"$fd\" -ge 10 ]; then echo n-ok; fi; rm -f {f}"
    );
    assert_matches_bash(&script);
}

#[test]
fn named_fd_close_makes_subsequent_write_fail() {
    // `{fd}<&-` closes the fd named by $var. A following write to `>&$fd` fails.
    // We compare the write's exit status (non-zero) byte-for-byte with bash.
    let f = format!("/tmp/huck_named_cl_{}", std::process::id());
    let script = format!(
        "{{ echo x >&$fd; }} {{fd}}>{f}; cat {f}; \
         {{ echo y >&$fd; }} 2>/dev/null {{fd}}<&-; echo \"closed-write-rc=$?\"; rm -f {f}"
    );
    assert_matches_bash(&script);
}

#[test]
fn external_parent_var_not_modified() {
    // bash semantics: for an EXTERNAL command the redirect + var-assignment happen
    // in the forked child, so the PARENT's $fd is untouched (a pre-set value is
    // preserved). huck matches: $fd stays 99.
    let f = format!("/tmp/huck_named_ext_{}", std::process::id());
    let script = format!(
        "fd=99; /bin/echo hi {{fd}}>{f} >/dev/null; echo \"fd=[$fd]\"; rm -f {f}"
    );
    assert_matches_bash(&script);
}

#[test]
fn external_child_inherits_named_fd_open() {
    // The exec'd child genuinely inherits the allocated fd OPEN (non-CLOEXEC):
    // perl writes to fd 10 (the first allocation is always 10 in both shells) and
    // the bytes land in the file. The parent $fd is NOT set for an external
    // command (bash parity), so we assert via the file content only.
    if Command::new("perl").arg("-e").arg("1").output().is_err() {
        return; // perl not available; skip
    }
    let f = format!("/tmp/huck_named_inh_{}", std::process::id());
    let script = format!(
        "perl -e 'open(F, \">&=10\") or die; print F \"inh\\n\"; close F' {{fd}}>{f}; \
         cat {f}; rm -f {f}"
    );
    assert_matches_bash(&script);
}
