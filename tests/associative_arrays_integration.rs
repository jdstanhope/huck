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
fn declare_and_read_roundtrip() {
    let (out, _, _) = run_capture("declare -A m\nm[foo]=bar\necho \"${m[foo]}\"\nexit\n");
    assert!(out.lines().any(|l| l == "bar"), "got: {out:?}");
}

#[test]
fn iteration_order_is_insertion_order() {
    let (out, _, _) = run_capture(
        "declare -A m\nm[a]=1\nm[b]=2\nm[c]=3\nfor k in \"${!m[@]}\"; do echo $k; done\nexit\n",
    );
    let lines: Vec<&str> = out.lines().collect();
    let key_lines: Vec<&str> = lines
        .iter()
        .filter(|l| ["a", "b", "c"].contains(l))
        .copied()
        .collect();
    assert_eq!(
        key_lines,
        vec!["a", "b", "c"],
        "expected insertion-order, got: {out:?}"
    );
}

#[test]
fn count_reflects_size_after_add_and_remove() {
    let (out, _, _) = run_capture(
        "declare -A m=([x]=1 [y]=2 [z]=3)\necho \"${#m[@]}\"\nunset m[y]\necho \"${#m[@]}\"\nexit\n",
    );
    let counts: Vec<&str> = out.lines().filter(|l| *l == "3" || *l == "2").collect();
    assert_eq!(counts, vec!["3", "2"], "got: {out:?}");
}

#[test]
fn append_element_concatenates() {
    let (out, _, _) =
        run_capture("declare -A m\nm[k]=hello\nm[k]+=_world\necho \"${m[k]}\"\nexit\n");
    assert!(out.lines().any(|l| l == "hello_world"), "got: {out:?}");
}

#[test]
fn append_compound_merges_keys() {
    let (out, _, _) = run_capture(
        "declare -A m=([a]=1)\nm+=([b]=2 [c]=3)\necho \"${#m[@]}\"\necho \"${m[b]}\"\nexit\n",
    );
    assert!(
        out.lines().any(|l| l == "3"),
        "expected count=3, got: {out:?}"
    );
    assert!(
        out.lines().any(|l| l == "2"),
        "expected m[b]=2, got: {out:?}"
    );
}

#[test]
fn unset_element_preserves_order_of_remaining() {
    let (out, _, _) = run_capture(
        "declare -A m=([first]=1 [middle]=2 [last]=3)\nunset m[middle]\nfor k in \"${!m[@]}\"; do echo $k; done\nexit\n",
    );
    let lines: Vec<&str> = out.lines().collect();
    let key_lines: Vec<&str> = lines
        .iter()
        .filter(|l| ["first", "middle", "last"].contains(l))
        .copied()
        .collect();
    assert_eq!(key_lines, vec!["first", "last"], "got: {out:?}");
}

#[test]
fn local_associative_scoped_to_function() {
    let (out, _, _) = run_capture(
        "declare -A m=([outer]=1)\n\
         f() { local -A m=([inner]=2); echo \"in: ${!m[@]}\"; }\n\
         f\n\
         echo \"out: ${!m[@]}\"\n\
         exit\n",
    );
    let scope_lines: Vec<&str> = out
        .lines()
        .filter(|l| l.starts_with("in: ") || l.starts_with("out: "))
        .collect();
    assert_eq!(
        scope_lines,
        vec!["in: inner", "out: outer"],
        "expected inner-then-outer ordering, got: {out:?}"
    );
}

#[test]
fn readonly_associative_blocks_element_write() {
    let (_out, err, _) = run_capture("readonly -A m=([k]=v)\nm[k]=changed\nexit\n");
    assert!(err.contains("readonly variable"), "got stderr: {err:?}");
}

#[test]
#[allow(non_snake_case)]
fn declare_dash_A_on_indexed_errors() {
    let (_out, err, _) = run_capture("a=(x y z)\ndeclare -A a\necho rc=$?\nexit\n");
    assert!(
        err.contains("cannot convert indexed to associative"),
        "got stderr: {err:?}"
    );
}

#[test]
fn nounset_on_missing_associative_key_is_fatal() {
    let (_out, err, rc) =
        run_capture("set -u\ndeclare -A m=([k]=v)\necho \"${m[nope]}\"\nexit\n");
    assert!(err.contains("unbound variable"), "got stderr: {err:?}");
    assert_ne!(rc, 0);
}
