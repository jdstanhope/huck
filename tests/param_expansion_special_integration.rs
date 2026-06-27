//! v233 M2: special parameters as the name in `${#X}` (length) and `${!X}`
//! (indirect). bash treats `${##}` as len of `$#`, `${!#}` as indirect of
//! `$#`, `${!*}`/`${!@}` as indirect of `$*`/`$@`. The engine already resolves
//! special-parameter names; this exercises the lexer change end-to-end.
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);
fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

fn run_file(script: &str) -> (String, String, i32) {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("huck_v233sp_{}_{}_.sh", std::process::id(), n));
    { let mut f = std::fs::File::create(&path).unwrap(); f.write_all(script.as_bytes()).unwrap(); }
    let out = Command::new(huck_bin()).arg(&path).stdin(Stdio::null()).output().unwrap();
    let _ = std::fs::remove_file(&path);
    (String::from_utf8_lossy(&out.stdout).into_owned(),
     String::from_utf8_lossy(&out.stderr).into_owned(),
     out.status.code().unwrap_or(-1))
}

#[test]
fn length_of_arg_count() {
    // ${##} = ${#<#>} = length of $# (which is "3") = 1.
    let (o, _e, c) = run_file("set -- a b c\necho ${##}\n");
    assert_eq!(c, 0);
    assert_eq!(o, "1\n");
}

#[test]
fn indirect_through_arg_count() {
    // ${!#} = indirect of $# (=3) -> $3 = c.
    let (o, _e, c) = run_file("set -- a b c\necho ${!#}\n");
    assert_eq!(c, 0);
    assert_eq!(o, "c\n");
}

#[test]
fn indirect_star_empty_is_empty() {
    // ${!*} with no positional params: indirect of empty $* -> empty, rc 0.
    let (o, e, c) = run_file("a=1\necho \"[${!*}]\"\n");
    assert_eq!(c, 0, "stderr: {e}");
    assert_eq!(o, "[]\n");
}

#[test]
fn indirect_star_set_is_invalid_variable_name() {
    // ${!*} with positionals set: $* = "x y z" is not a valid var name -> rc 1.
    let (_o, e, c) = run_file("set -- x y z\necho \"[${!*}]\"\n");
    assert_eq!(c, 1, "stderr: {e}");
    assert!(e.contains("invalid variable name"), "stderr: {e}");
}

#[test]
fn indirect_at_set_is_invalid_variable_name() {
    // ${!@} behaves like ${!*} for a multi-word $@.
    let (_o, e, c) = run_file("set -- x y z\necho \"[${!@}]\"\n");
    assert_eq!(c, 1, "stderr: {e}");
    assert!(e.contains("invalid variable name"), "stderr: {e}");
}

#[test]
fn arg_count_at_star_regression() {
    // Regression: ${#@} / ${#*} stay "count of positional params", NOT
    // length-of-special-param.
    let (o, _e, c) = run_file("set -- a b c\necho ${#@} ${#*}\n");
    assert_eq!(c, 0);
    assert_eq!(o, "3 3\n");
}

#[test]
fn indirect_dollar_bang_bad_subst() {
    // ${!$} and ${!!} are bad substitutions in bash (rc 1), not special-param
    // indirect.
    let (_o, e, c) = run_file("echo before\necho ${!$}\necho after\n");
    assert!(e.contains("bad substitution"), "stderr: {e}");
    assert_ne!(c, 0);
}
