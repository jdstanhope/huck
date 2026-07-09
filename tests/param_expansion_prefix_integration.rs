//! v233 M1: `${!prefix*}` / `${!prefix@}` prefix-name expansion — expand to
//! the sorted names of set shell variables whose name starts with `prefix`.
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);
fn huck_bin() -> &'static str {
    env!("CARGO_BIN_EXE_huck")
}

fn run_file(script: &str) -> (String, String, i32) {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("huck_v233pfx_{}_{}_.sh", std::process::id(), n));
    {
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(script.as_bytes()).unwrap();
    }
    let out = Command::new(huck_bin())
        .arg(&path)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let _ = std::fs::remove_file(&path);
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn prefix_names_star_lists_sorted() {
    let (o, _e, c) = run_file("_Qa=1\n_Qb=2\necho ${!_Q*}\n");
    assert_eq!(c, 0);
    assert_eq!(o, "_Qa _Qb\n");
}

#[test]
fn prefix_names_at_iterates() {
    let (o, _e, _c) = run_file("_Qa=1\n_Qb=2\nfor k in ${!_Q@}; do echo $k; done\n");
    assert_eq!(o, "_Qa\n_Qb\n");
}

#[test]
fn prefix_names_no_match_empty() {
    let (o, _e, c) = run_file("echo \"[${!NOSUCHPREFIX_XYZ*}]\"\n");
    assert_eq!(c, 0);
    assert_eq!(o, "[]\n");
}

#[test]
fn prefix_names_star_quoted_is_single_field() {
    // Quoted `${!pfx*}` joins on the first IFS char (space) as ONE field.
    let (o, _e, c) = run_file("_Qa=1\n_Qb=2\nprintf '<%s>\\n' \"${!_Q*}\"\n");
    assert_eq!(c, 0);
    assert_eq!(o, "<_Qa _Qb>\n");
}

#[test]
fn prefix_names_at_quoted_separate_fields() {
    // Quoted `${!pfx@}` yields separate words like `"$@"`.
    let (o, _e, c) = run_file("_Qa=1\n_Qb=2\nprintf '<%s>\\n' \"${!_Q@}\"\n");
    assert_eq!(c, 0);
    assert_eq!(o, "<_Qa>\n<_Qb>\n");
}
