//! v147: export -f — function export/import interop + Shellshock hardening.
use std::process::{Command, Stdio};

fn huck() -> &'static str {
    env!("CARGO_BIN_EXE_huck")
}

fn run(prog: &str, args: &[&str], envs: &[(&str, &str)]) -> (String, i32) {
    let mut c = Command::new(prog);
    c.args(args).stdin(Stdio::null());
    for (k, v) in envs {
        c.env(k, v);
    }
    let o = c.output().expect("spawn");
    (
        String::from_utf8_lossy(&o.stdout).into_owned(),
        o.status.code().unwrap_or(-1),
    )
}

#[test]
fn huck_exports_to_child_bash() {
    let (out, _) = run(
        huck(),
        &["-c", "f(){ echo OK; }; export -f f; bash -c f"],
        &[],
    );
    assert_eq!(out, "OK\n", "child bash didn't import: {out:?}");
}

#[test]
fn huck_exports_to_child_huck() {
    let cmd = format!("f(){{ echo OK; }}; export -f f; {} -c f", huck());
    let (out, _) = run(huck(), &["-c", &cmd], &[]);
    assert_eq!(out, "OK\n", "child huck didn't import: {out:?}");
}

#[test]
fn huck_imports_bash_shaped_env() {
    let (out, _) = run(
        huck(),
        &["-c", "g"],
        &[("BASH_FUNC_g%%", "() { echo FROMBASH; }")],
    );
    assert_eq!(
        out, "FROMBASH\n",
        "huck didn't import BASH_FUNC env: {out:?}"
    );
}

#[test]
fn huck_imports_hyphenated_bash_shaped_env() {
    // R1 (#339): bash routinely uses hyphens in function names (`foo-a`);
    // huck's import must accept them too, matching bash.
    let (out, _) = run(
        huck(),
        &["-c", "foo-a"],
        &[("BASH_FUNC_foo-a%%", "() { echo exportfunc ok 2; }")],
    );
    assert_eq!(
        out, "exportfunc ok 2\n",
        "huck didn't import hyphenated BASH_FUNC env: {out:?}"
    );
}

#[test]
fn huck_hyphenated_function_round_trips_to_child_huck() {
    // Full round-trip: define a hyphenated-name function, export -f it, and
    // have a CHILD huck import + run it via the inherited BASH_FUNC_ env var.
    let cmd = format!(
        "foo-a(){{ echo exportfunc ok 2; }}; export -f foo-a; {} -c foo-a",
        huck()
    );
    let (out, _) = run(huck(), &["-c", &cmd], &[]);
    assert_eq!(
        out, "exportfunc ok 2\n",
        "hyphenated round-trip failed: {out:?}"
    );
}

#[test]
fn export_f_unencodable_name_cannot_export_rc1() {
    // R4 (#339): a function name containing `=` can't be encoded as
    // BASH_FUNC_<name>%%, so bash (and now huck) reject the export.
    let (_o, rc) = run(
        huck(),
        &["-c", "function foo=bar { :; }; export -f foo=bar"],
        &[],
    );
    assert_eq!(rc, 1);
}

#[test]
fn export_f_not_a_function_rc1() {
    let (_o, rc) = run(huck(), &["-c", "export -f nope"], &[]);
    assert_eq!(rc, 1);
}

#[test]
fn unset_f_unexports() {
    let (out, _) = run(
        huck(),
        &[
            "-c",
            "f(){ :; }; export -f f; unset -f f; env | grep -c BASH_FUNC_f || true",
        ],
        &[],
    );
    assert_eq!(out.trim(), "0", "unset -f should drop the export: {out:?}");
}

#[test]
fn shellshock_trailing_command_not_executed() {
    let marker = format!("/tmp/huck_pwn_{}", std::process::id());
    let _ = std::fs::remove_file(&marker);
    let payload = format!("() {{ :; }}; touch {marker}");
    let (out, _rc) = run(
        huck(),
        &[
            "-c",
            "type x >/dev/null 2>&1 && echo DEFINED || echo undefined",
        ],
        &[("BASH_FUNC_x%%", &payload)],
    );
    assert!(
        !std::path::Path::new(&marker).exists(),
        "Shellshock: trailing command ran!"
    );
    assert_eq!(
        out.trim(),
        "undefined",
        "malicious function should not be defined: {out:?}"
    );
    let _ = std::fs::remove_file(&marker);
}

/// #341/#343 (v349): `export -f name=value` — quoted OR unquoted — reports the
/// WHOLE token in the `not a function` error, not the truncated name. The
/// executor's DeclArg split (`is_assignment_word` and Root D) turns `foo=bar`
/// into an `Assign{foo}` before the `-f` path; the `-f` branch must
/// reconstruct the full token. Captures stderr (the `run` helper is
/// stdout-only). Regression guard for the v349 Root-D export fix.
#[test]
fn export_f_name_eq_value_reports_full_token() {
    for frag in [
        "export -f foo=bar",              // unquoted (#341)
        "export -f 'foo=bar'",            // quoted (Root-D regression)
        "foo(){ :; }; export -f foo=bar", // even when the bare name IS a function
        "x=bar; export -f foo=$x",        // #346: value is expanded, not dropped
        "b=b; a=r; export -f foo=ba$a",   // #346: mixed literal + expansion
    ] {
        let o = std::process::Command::new(huck())
            .args(["-c", frag])
            .stdin(std::process::Stdio::null())
            .output()
            .expect("spawn");
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(
            err.contains("foo=bar: not a function"),
            "frag {frag:?}: expected full-token error, got stderr {err:?}"
        );
        assert_eq!(o.status.code(), Some(1), "frag {frag:?} rc");
    }
}

/// #114: `export NAME[subscript]=v` (an invalid identifier for export) names the
/// FULL `NAME[subscript]` in the error, not the bare `NAME` — with the subscript
/// after word expansion but not arithmetic eval.
#[test]
fn export_invalid_subscript_names_full_target() {
    let cases = [
        ("export AA[4]=1", "`AA[4]': not a valid identifier"),
        ("x=9; export AA[$x]=1", "`AA[9]': not a valid identifier"),
        ("export AA[2+2]=1", "`AA[2+2]': not a valid identifier"),
    ];
    for (frag, want) in cases {
        let o = std::process::Command::new(huck())
            .args(["-c", frag])
            .stdin(std::process::Stdio::null())
            .output()
            .expect("spawn");
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(
            err.contains(want),
            "frag {frag:?}: want {want:?}, got {err:?}"
        );
        assert_eq!(o.status.code(), Some(1), "frag {frag:?} rc");
    }
}
