//! v97: redirections on compound commands.
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

fn run(script: &str) -> (String, i32) {
    let mut child = Command::new(huck_bin())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn huck");
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(), out.status.code().unwrap_or(-1))
}

#[test]
fn heredoc_on_while_done() {
    assert_eq!(run("while read x; do echo \"g:$x\"; done <<EOF\na\nb\nEOF\n").0, "g:a\ng:b\n");
}

#[test]
fn redirect_out_on_for() {
    assert_eq!(run("for i in 1 2; do echo $i; done > /tmp/huck_t_for\ncat /tmp/huck_t_for\n").0, "1\n2\n");
}

#[test]
fn redirect_out_on_if() {
    assert_eq!(run("if true; then echo hi; fi > /tmp/huck_t_if\ncat /tmp/huck_t_if\n").0, "hi\n");
}

#[test]
fn redirect_out_on_brace_group() {
    assert_eq!(run("{ echo a; echo b; } > /tmp/huck_t_bg\ncat /tmp/huck_t_bg\n").0, "a\nb\n");
}

#[test]
fn redirect_out_on_subshell() {
    assert_eq!(run("( echo x ) > /tmp/huck_t_ss\ncat /tmp/huck_t_ss\n").0, "x\n");
}

#[test]
fn redirect_out_on_case() {
    assert_eq!(run("case z in z) echo m;; esac > /tmp/huck_t_case\ncat /tmp/huck_t_case\n").0, "m\n");
}

#[test]
fn herestring_on_while() {
    assert_eq!(run("while read x; do echo \"[$x]\"; done <<< 'one two'\n").0, "[one two]\n");
}

#[test]
fn append_on_brace_group() {
    assert_eq!(run("echo first > /tmp/huck_t_ap\n{ echo second; } >> /tmp/huck_t_ap\ncat /tmp/huck_t_ap\n").0,
               "first\nsecond\n");
}

#[test]
fn stderr_redirect_on_compound() {
    // 2>&1 then capture: error inside a group goes to the redirected stdout file.
    assert_eq!(run("{ echo out; echo err >&2; } > /tmp/huck_t_se 2>&1\ncat /tmp/huck_t_se\n").0, "out\nerr\n");
}

#[test]
fn no_redirect_compound_unchanged() {
    // Regression: a bare compound still works (not wrapped).
    assert_eq!(run("for i in 1 2; do echo $i; done\n").0, "1\n2\n");
}

#[test]
fn capture_with_inner_redirect() {
    // A >file inside a captured compound diverts that line to the file.
    assert_eq!(run("x=$({ echo a; echo b > /tmp/huck_t_cap; }); echo \"[$x]\"\ncat /tmp/huck_t_cap\n").0,
               "[a]\nb\n");
}

#[test]
fn capture_with_compound_stdout_redirect() {
    // bash: a >file on the COMPOUND inside $() diverts to the file; capture is empty.
    let (out, _rc) = run("x=$({ echo a; echo b; } > /tmp/huck_t_capredir); echo \"[$x]\"\ncat /tmp/huck_t_capredir\n");
    assert_eq!(out, "[]\na\nb\n");
}

// ── v156 task 3: in-process ordered redirect applier (RedirectScope) ──

#[test]
fn l08_source_order_2gt1_then_redirect() {
    // L-08 ordering. `2>&1 >file`: stderr is dup'd onto the ORIGINAL stdout
    // FIRST, then stdout goes to the file. So ONLY `out` lands in the file
    // (`err` follows the original stdout). Asserting on the FILE content
    // isolates the ordering from where the un-redirected stderr ends up.
    let f = "/tmp/huck_t_l08a";
    run(&format!(
        "{{ echo out; echo err >&2; }} 2>&1 >{f}\n"
    ));
    let contents = std::fs::read_to_string(f).unwrap();
    assert_eq!(contents, "out\n", "2>&1 >file: only stdout in the file");
}

#[test]
fn l08_source_order_redirect_then_2gt1() {
    // `>file 2>&1`: stdout to file FIRST, then stderr follows the (already
    // redirected) stdout — BOTH land in the file. Different from the above,
    // proving source-order is honored.
    let f = "/tmp/huck_t_l08b";
    run(&format!(
        "{{ echo out; echo err >&2; }} >{f} 2>&1\n"
    ));
    let contents = std::fs::read_to_string(f).unwrap();
    assert_eq!(contents, "out\nerr\n", ">file 2>&1: both streams in the file");
}

#[test]
fn compound_stderr_to_file_via_dup() {
    // `{ echo x >&2; } 2>file`: the inner `>&2` writes to fd 2, which the
    // compound redirects to the file. The file gets `x`.
    let f = "/tmp/huck_t_cdup";
    let (out, _rc) = run(&format!("{{ echo x >&2; }} 2>{f}\ncat {f}\n"));
    assert_eq!(out, "x\n");
}

#[test]
fn builtin_dup_stdout_to_stderr_then_file() {
    // `echo y 1>&2 2>file`: 1>&2 dups fd1 onto the ORIGINAL fd2 (terminal), then
    // 2>file redirects fd2. `echo` writes to fd1 (terminal stderr), so the file
    // stays empty — the create-truncates-it side effect is the only file change.
    let f = "/tmp/huck_t_bdup";
    let (out, _rc) = run(&format!("echo first > {f}\necho y 1>&2 2>{f}\ncat {f}\n"));
    assert_eq!(out, "", "file should be truncated and not receive y");
}

#[test]
fn capture_stdout_not_redirected_still_captures() {
    // Only stderr redirected on the compound -> stdout is still captured.
    // The stderr writer is an external command (`sh -c`) rather than a
    // builtin: huck has a separate, pre-existing limitation where a builtin's
    // `>&2` write is steered into the in-memory capture buffer instead of the
    // real fd 2, so `echo e >&2` would wrongly appear in the capture. That bug
    // is orthogonal to this fix (compound stdout-redirect diversion); using an
    // external isolates the behavior under test, and matches bash's `[o]`.
    let (out, _rc) = run("x=$({ echo o; sh -c 'echo e 1>&2'; } 2>/dev/null); echo \"[$x]\"\n");
    assert_eq!(out, "[o]\n");
}
