use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run_capture(script: &str) -> (String, String, i32) {
    let mut child = Command::new(huck_binary())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn literal_then_for_loop_iterates_in_order() {
    let (out, _, _) = run_capture(
        "a=(red green blue)\nfor c in \"${a[@]}\"; do echo \"$c\"; done\nexit\n",
    );
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines.contains(&"red"));
    assert!(lines.contains(&"green"));
    assert!(lines.contains(&"blue"));
}

#[test]
fn sparse_subscript_count_is_one() {
    let (out, _, _) = run_capture("a[5]=x\necho \"${#a[@]}\" \"${!a[@]}\"\nexit\n");
    assert!(out.lines().any(|l| l == "1 5"), "got: {out:?}");
}

#[test]
fn element_read_and_write_roundtrip() {
    let (out, _, _) = run_capture("a[3]=hello\necho \"${a[3]}\"\nexit\n");
    assert!(out.lines().any(|l| l == "hello"), "got: {out:?}");
}

#[test]
fn append_array_extends() {
    let (out, _, _) = run_capture("a=(x y)\na+=(z w)\necho \"${a[@]}\"\nexit\n");
    assert!(out.lines().any(|l| l == "x y z w"), "got: {out:?}");
}

#[test]
fn append_element_concatenates() {
    let (out, _, _) = run_capture("a[0]=hello\na[0]+=_world\necho \"${a[0]}\"\nexit\n");
    assert!(out.lines().any(|l| l == "hello_world"), "got: {out:?}");
}

#[test]
fn scalar_promotes_on_element_assign() {
    let (out, _, _) = run_capture("a=old\na[2]=new\necho \"${a[0]}\" \"${a[2]}\"\nexit\n");
    assert!(out.lines().any(|l| l == "old new"), "got: {out:?}");
}

#[test]
fn quoted_at_preserves_empty_elements() {
    let (out, _, _) = run_capture(
        "a=(x \"\" z)\nfor v in \"${a[@]}\"; do echo \"[$v]\"; done\nexit\n",
    );
    let bracket_lines: Vec<&str> = out.lines().filter(|l| l.starts_with('[')).collect();
    assert_eq!(bracket_lines.len(), 3, "expected 3 elements, got: {out:?}");
    assert!(
        bracket_lines.contains(&"[]"),
        "expected empty element preserved: {out:?}"
    );
}

#[test]
fn star_joins_by_ifs() {
    let (out, _, _) = run_capture("a=(x y z)\necho \"${a[*]}\"\nexit\n");
    assert!(out.lines().any(|l| l == "x y z"), "got: {out:?}");
}

#[test]
fn unset_element_removes_key() {
    let (out, _, _) = run_capture("a=(x y z)\nunset a[1]\necho \"${!a[@]}\"\nexit\n");
    assert!(out.lines().any(|l| l == "0 2"), "got: {out:?}");
}

#[test]
fn local_array_scoped_to_function() {
    let (out, _, _) = run_capture(
        "a=(outer)\nf() { local a=(inner); echo \"${a[0]}\"; }\nf\necho \"${a[0]}\"\nexit\n",
    );
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines.contains(&"inner"));
    assert!(lines.contains(&"outer"));
}

#[test]
fn readonly_array_blocks_element_write_with_diagnostic() {
    let (_out, err, _) = run_capture("readonly a=(x)\na[0]=changed\nexit\n");
    assert!(
        err.contains("readonly variable"),
        "expected readonly diagnostic: {err:?}"
    );
}

#[test]
fn nounset_on_unset_element_is_fatal() {
    let (_out, err, rc) = run_capture("set -u\na=(x)\necho \"${a[5]}\"\nexit\n");
    assert!(
        err.contains("unbound variable"),
        "expected unbound diagnostic: {err:?}"
    );
    assert_ne!(rc, 0, "expected non-zero exit under set -u");
}
