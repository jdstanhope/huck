//! v140: `read -a` reads a line, IFS-splits into an indexed array. Run via the
//! huck binary with the script in `-c` (here-strings keep `read` in the main shell).
use std::process::Command;

fn huck_bin() -> &'static str {
    env!("CARGO_BIN_EXE_huck")
}

fn huck_c(script: &str) -> (String, i32) {
    let o = Command::new(huck_bin())
        .arg("-c")
        .arg(script)
        .output()
        .expect("spawn");
    (
        String::from_utf8_lossy(&o.stdout).into_owned(),
        o.status.code().unwrap_or(-1),
    )
}

#[test]
fn read_a_basic() {
    let (out, code) = huck_c(r#"read -a arr <<< "a b c"; echo "${arr[*]}|${#arr[@]}""#);
    assert_eq!(out, "a b c|3\n", "out={out:?}");
    assert_eq!(code, 0);
}

#[test]
fn read_a_custom_ifs() {
    let (out, _c) = huck_c(r#"IFS=: read -a arr <<< "a:b:c"; echo "${arr[*]}|${#arr[@]}""#);
    assert_eq!(out, "a b c|3\n", "out={out:?}");
}

#[test]
fn read_a_clears_existing_array() {
    let (out, _c) =
        huck_c(r#"arr=(old x y z); read -a arr <<< "a b"; echo "${arr[*]}|${#arr[@]}""#);
    assert_eq!(out, "a b|2\n", "out={out:?}");
}

#[test]
fn read_ra_raw_backslash() {
    // -r: backslash is literal. Input "x\ty" (literal backslash-t) -> with default
    // IFS no split on the literal chars; one field "x\ty".
    let (out, _c) = huck_c(r#"read -ra arr <<< 'x\ty'; echo "${#arr[@]}|${arr[0]}""#);
    assert_eq!(out, "1|x\\ty\n", "out={out:?}");
}

#[test]
fn read_a_leaves_trailing_names_untouched() {
    // bash leaves a pre-set trailing NAME alone when -a is used.
    let (out, _c) =
        huck_c(r#"extra=PRESET; read -a arr extra <<< "a b"; echo "${arr[*]}|[$extra]""#);
    assert_eq!(out, "a b|[PRESET]\n", "out={out:?}");
}

#[test]
fn read_a_readonly_array_returns_1() {
    // read -a into a readonly array: read itself fails rc 1 and leaves the
    // array unchanged. We capture read's own $? in stdout (a trailing echo
    // would otherwise reset the whole-script exit code to 0 — matches bash).
    let (out, _c) =
        huck_c(r#"readonly arr=(keep); read -a arr <<< "a b"; echo "rc:$? arr:${arr[*]}""#);
    assert_eq!(
        out, "rc:1 arr:keep\n",
        "read should fail and not modify array; out={out:?}"
    );
}

#[test]
fn mapfile_t_strips_newline() {
    let (out, _c) = huck_c("mapfile -t arr <<< $'x\\ny\\nz'; echo \"${#arr[@]}|${arr[1]}\"");
    assert_eq!(out, "3|y\n", "out={out:?}");
}

#[test]
fn mapfile_keeps_newline_without_t() {
    let (out, _c) =
        huck_c("mapfile arr <<< $'a\\nb'; printf '%q %q\\n' \"${arr[0]}\" \"${arr[1]}\"");
    assert_eq!(out, "$'a\\n' $'b\\n'\n", "out={out:?}");
}

#[test]
fn mapfile_n_limit() {
    let (out, _c) =
        huck_c("mapfile -n 2 -t arr <<< $'a\\nb\\nc\\nd'; echo \"${arr[*]}|${#arr[@]}\"");
    assert_eq!(out, "a b|2\n", "out={out:?}");
}

#[test]
fn mapfile_s_skip() {
    let (out, _c) = huck_c("mapfile -s 1 -t arr <<< $'a\\nb\\nc'; echo \"${arr[*]}\"");
    assert_eq!(out, "b c\n", "out={out:?}");
}

#[test]
fn mapfile_d_delim() {
    let (out, _c) = huck_c("mapfile -d : -t arr <<< 'a:b:c'; echo \"${#arr[@]}|${arr[1]}\"");
    assert_eq!(out, "3|b\n", "out={out:?}");
}

#[test]
fn mapfile_o_origin_no_clear() {
    let (out, _c) = huck_c("mapfile -O 2 -t arr <<< $'x\\ny'; echo \"${!arr[*]}|${arr[*]}\"");
    assert_eq!(out, "2 3|x y\n", "out={out:?}");
}

#[test]
fn readarray_synonym_and_default_name() {
    let (out, _c) = huck_c("readarray -t arr <<< $'p\\nq'; echo \"${arr[*]}\"");
    assert_eq!(out, "p q\n", "out={out:?}");
    let (out2, _c2) = huck_c("mapfile -t <<< $'a\\nb'; echo \"${MAPFILE[*]}\"");
    assert_eq!(out2, "a b\n", "out2={out2:?}");
}
