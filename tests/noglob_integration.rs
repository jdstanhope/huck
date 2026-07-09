//! v120: set -f / set -o noglob (M-08).
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
static COUNTER: AtomicU64 = AtomicU64::new(0);
fn huck_bin() -> &'static str {
    env!("CARGO_BIN_EXE_huck")
}
fn run(script: &str) -> String {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("huck_v120g_{}_{}.sh", std::process::id(), n));
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
    String::from_utf8_lossy(&out.stdout).into_owned()
}
#[test]
fn noglob_makes_star_literal() {
    let s = "d=$(mktemp -d); touch \"$d\"/x.txt\ncd \"$d\"\nset -f; echo *.txt\nset +f; echo *.txt\nrm -rf \"$d\"\n";
    assert_eq!(run(s), "*.txt\nx.txt\n");
}
#[test]
fn noglob_via_long_form() {
    let s = "d=$(mktemp -d); touch \"$d\"/x.txt\ncd \"$d\"\nset -o noglob; echo *.txt\nset +o noglob; echo *.txt\nrm -rf \"$d\"\n";
    assert_eq!(run(s), "*.txt\nx.txt\n");
}
#[test]
fn noglob_in_dollar_dash_and_minus_o() {
    assert_eq!(
        run(
            "set -f\n[[ -o noglob ]] && echo ON || echo OFF\ncase \"$-\" in *f*) echo HASF;; *) echo no;; esac\n"
        ),
        "ON\nHASF\n"
    );
}
#[test]
fn noglob_is_pathname_only() {
    assert_eq!(
        run(
            "set -f\ncase abc in a*) echo CY;; esac\ns=a1b; echo \"${s//[0-9]/_}\"\n[[ x == ? ]] && echo BY\n"
        ),
        "CY\na_b\nBY\n"
    );
}
