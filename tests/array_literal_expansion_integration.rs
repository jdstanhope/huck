//! v117: array-literal element field-expansion (M-112).
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn huck_bin() -> &'static str {
    env!("CARGO_BIN_EXE_huck")
}

/// Run `script` as a file-arg (true non-interactive path). Returns stdout.
fn run(script: &str) -> String {
    let dir = std::env::temp_dir();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = dir.join(format!("huck_v117_{}_{}.sh", std::process::id(), n));
    {
        let mut f = std::fs::File::create(&path).expect("create temp script");
        f.write_all(script.as_bytes()).unwrap();
    }
    let out = Command::new(huck_bin())
        .arg(&path)
        .stdin(Stdio::null())
        .output()
        .expect("spawn huck");
    let _ = std::fs::remove_file(&path);
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn scalar_word_split() {
    assert_eq!(
        run("s=\"a b c\"\narr=($s)\necho \"n=${#arr[@]}\"\n"),
        "n=3\n"
    );
}
#[test]
fn cmdsub_split() {
    assert_eq!(run("arr=($(echo x y z))\necho \"n=${#arr[@]}\"\n"), "n=3\n");
}
#[test]
fn quoted_array_at_fans_out() {
    assert_eq!(
        run("w=(a b c)\narr=(\"${w[@]}\")\necho \"n=${#arr[@]}\"\n"),
        "n=3\n"
    );
}
#[test]
fn quoted_array_at_keeps_empty_member() {
    assert_eq!(
        run("w=(a \"\" c)\narr=(\"${w[@]}\")\necho \"n=${#arr[@]}\"\n"),
        "n=3\n"
    );
}
#[test]
fn unquoted_empty_drops() {
    assert_eq!(run("e=\narr=(a $e b)\necho \"n=${#arr[@]}\"\n"), "n=2\n");
}
#[test]
fn quoted_empty_kept() {
    assert_eq!(
        run("e=\narr=(a \"$e\" b)\necho \"n=${#arr[@]}\"\n"),
        "n=3\n"
    );
}
#[test]
fn quoted_star_joins_to_one() {
    assert_eq!(
        run("w=(a b c)\narr=(\"${w[*]}\")\necho \"n=${#arr[@]}\"\n"),
        "n=1\n"
    );
}
#[test]
fn subscript_value_not_split() {
    assert_eq!(
        run("s=\"a b c\"\narr=([0]=$s)\necho \"n=${#arr[@]} z=[${arr[0]}]\"\n"),
        "n=1 z=[a b c]\n"
    );
}
#[test]
fn mixed_bare_and_subscript_index_continuation() {
    assert_eq!(
        run("s=\"x y\"\narr=(a $s [9]=z b)\necho \"n=${#arr[@]} idx=[${!arr[@]}]\"\n"),
        "n=5 idx=[0 1 2 9 10]\n"
    );
}
#[test]
fn fatal_pe_in_element_aborts() {
    // set -u + unset var in an array element: bash aborts the assignment and the
    // whole script (the "after" line never prints). Verified vs bash --norc:
    // stdout is empty, rc=1. huck must match byte-for-byte on stdout.
    assert_eq!(run("set -u\narr=(a $undefined b)\necho after\n"), "");
}
#[test]
fn local_array_literal_fans_out() {
    assert_eq!(
        run("w=(a b c)\nf(){ local arr=(p \"${w[@]}\" q); echo \"n=${#arr[@]}\"; }\nf\n"),
        "n=5\n"
    );
}
#[test]
fn append_scalar_split() {
    assert_eq!(
        run("arr=(a)\ns=\"b c\"\narr+=($s)\necho \"n=${#arr[@]}\"\n"),
        "n=3\n"
    );
}
#[test]
fn append_array_at_fans_out() {
    assert_eq!(
        run("arr=(a)\nw=(b c d)\narr+=(\"${w[@]}\")\necho \"n=${#arr[@]}\"\n"),
        "n=4\n"
    );
}
#[test]
fn append_continues_index() {
    assert_eq!(
        run("arr=(a b)\narr+=(c d)\necho \"idx=[${!arr[@]}]\"\n"),
        "idx=[0 1 2 3]\n"
    );
}
#[test]
fn append_to_unset_starts_at_zero() {
    assert_eq!(
        run("arr+=(x y)\necho \"idx=[${!arr[@]}]\"\n"),
        "idx=[0 1]\n"
    );
}
#[test]
fn fatal_pe_in_subscripted_element_aborts() {
    // set -u + unset var in a [i]=value element: bash aborts the assignment.
    assert_eq!(run("set -u\narr=([0]=$undef)\necho after\n"), "");
}
