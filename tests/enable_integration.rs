//! v230: enable builtin — special listing, toggle, errors.
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);
fn huck_bin() -> &'static str {
    env!("CARGO_BIN_EXE_huck")
}

/// Run `script` as a file arg (true non-interactive path). Returns (stdout, stderr, code).
fn run_file(script: &str) -> (String, String, i32) {
    let dir = std::env::temp_dir();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = dir.join(format!("huck_enable_{}_{}.sh", std::process::id(), n));
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
fn enable_ps_lists_special_sorted() {
    let (o, _, c) = run_file("enable -ps\n");
    assert_eq!(c, 0);
    assert_eq!(
        o,
        "\
enable .
enable :
enable break
enable continue
enable eval
enable exec
enable exit
enable export
enable readonly
enable return
enable set
enable shift
enable source
enable times
enable trap
enable unset
"
    );
}

#[test]
fn enable_nps_empty_when_none_disabled() {
    let (o, _, _) = run_file("enable -nps\n");
    assert_eq!(o, "");
}

#[test]
fn disable_then_type_not_builtin() {
    // enable -n test makes `type -t test` no longer "builtin".
    let (o, _, _) = run_file("enable -n test\ntype -t test\n");
    assert_ne!(o.trim(), "builtin", "got: {o:?}");
}

#[test]
fn reenable_restores_builtin() {
    let (o, _, _) = run_file("enable -n test\nenable test\ntype -t test\n");
    assert_eq!(o.trim(), "builtin");
}

#[test]
fn enable_unknown_errors() {
    let (_, e, c) = run_file("enable sh bash\n");
    assert!(e.contains("enable: sh: not a shell builtin"), "stderr: {e}");
    assert!(
        e.contains("enable: bash: not a shell builtin"),
        "stderr: {e}"
    );
    assert_eq!(c, 1);
}
