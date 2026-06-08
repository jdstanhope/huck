//! v117: array-literal element field-expansion (M-112).
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

/// Run `script` as a file-arg (true non-interactive path). Returns stdout.
fn run(script: &str) -> String {
    let dir = std::env::temp_dir();
    let path = dir.join(format!("huck_v117_{}.sh", std::process::id()));
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
    assert_eq!(run("s=\"a b c\"\narr=($s)\necho \"n=${#arr[@]}\"\n"), "n=3\n");
}
#[test]
fn cmdsub_split() {
    assert_eq!(run("arr=($(echo x y z))\necho \"n=${#arr[@]}\"\n"), "n=3\n");
}
#[test]
fn quoted_array_at_fans_out() {
    assert_eq!(run("w=(a b c)\narr=(\"${w[@]}\")\necho \"n=${#arr[@]}\"\n"), "n=3\n");
}
#[test]
fn quoted_array_at_keeps_empty_member() {
    assert_eq!(run("w=(a \"\" c)\narr=(\"${w[@]}\")\necho \"n=${#arr[@]}\"\n"), "n=3\n");
}
#[test]
fn unquoted_empty_drops() {
    assert_eq!(run("e=\narr=(a $e b)\necho \"n=${#arr[@]}\"\n"), "n=2\n");
}
#[test]
fn quoted_empty_kept() {
    assert_eq!(run("e=\narr=(a \"$e\" b)\necho \"n=${#arr[@]}\"\n"), "n=3\n");
}
#[test]
fn quoted_star_joins_to_one() {
    assert_eq!(run("w=(a b c)\narr=(\"${w[*]}\")\necho \"n=${#arr[@]}\"\n"), "n=1\n");
}
#[test]
fn subscript_value_not_split() {
    assert_eq!(run("s=\"a b c\"\narr=([0]=$s)\necho \"n=${#arr[@]} z=[${arr[0]}]\"\n"), "n=1 z=[a b c]\n");
}
#[test]
fn mixed_bare_and_subscript_index_continuation() {
    assert_eq!(
        run("s=\"x y\"\narr=(a $s [9]=z b)\necho \"n=${#arr[@]} idx=[${!arr[@]}]\"\n"),
        "n=5 idx=[0 1 2 9 10]\n"
    );
}
#[test]
fn local_array_literal_fans_out() {
    assert_eq!(
        run("w=(a b c)\nf(){ local arr=(p \"${w[@]}\" q); echo \"n=${#arr[@]}\"; }\nf\n"),
        "n=5\n"
    );
}
