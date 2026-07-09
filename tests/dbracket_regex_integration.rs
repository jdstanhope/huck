//! v105: `[[ … =~ REGEX ]]` regex-operand match semantics equal bash.
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str {
    env!("CARGO_BIN_EXE_huck")
}

/// Returns (stdout, stderr, exit_code).
fn run(script: &str) -> (String, String, i32) {
    let mut child = Command::new(huck_bin())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn space_inside_parens_matches() {
    assert_eq!(run("[[ \"a b\" =~ (a b) ]] && echo M || echo N\n").0, "M\n");
    assert_eq!(run("[[ ab =~ (a b) ]] && echo M || echo N\n").0, "N\n");
}

#[test]
fn quoted_span_matches_literally() {
    // v199 (L-23): a QUOTED span of the regex operand matches literally — its
    // metacharacters are escaped. An unquoted span stays an active regex.
    // `.` quoted -> literal dot: "axb" does NOT match "a.b".
    assert_eq!(run("[[ axb =~ \"a.b\" ]] && echo M || echo N\n").0, "N\n");
    // `.` quoted -> literal dot: "a.b" matches "a.b".
    assert_eq!(run("[[ a.b =~ \"a.b\" ]] && echo M || echo N\n").0, "M\n");
    // unquoted `.` stays active: "axb" matches a.b.
    assert_eq!(run("[[ axb =~ a.b ]] && echo M || echo N\n").0, "M\n");
    // partial quoting: only the quoted `.` is literal.
    assert_eq!(run("[[ axb =~ a\".\"b ]] && echo M || echo N\n").0, "N\n");
    // a quoted `$var` is literal; an unquoted one is active (bash 3.2+).
    assert_eq!(
        run("re='a.b'; [[ axb =~ \"$re\" ]] && echo M || echo N\n").0,
        "N\n"
    );
    assert_eq!(
        run("re='a.b'; [[ axb =~ $re ]] && echo M || echo N\n").0,
        "M\n"
    );
}

#[test]
fn line847_shape_parses_and_matches() {
    // bash gives N here: the trailing `.` requires a char after `]`, but
    // the subject "[no-]" ends at `]`, so the overall pattern cannot match.
    assert_eq!(
        run("[[ \"[no-]\" =~ (\\[((no|dont)-?)\\]). ]] && echo M || echo N\n").0,
        "N\n"
    );
}

#[test]
fn anchored_groups() {
    assert_eq!(
        run("c=foo=bar; [[ $c =~ ^([A-Za-z_][A-Za-z0-9_]*)=(.*)$ ]] && echo M || echo N\n").0,
        "M\n"
    );
}

#[test]
fn bracket_with_double_close_inside() {
    assert_eq!(
        run("[[ \"-abc\" =~ (-[^]]+) ]] && echo M || echo N\n").0,
        "M\n"
    );
}

#[test]
fn var_interpolation_in_operand() {
    assert_eq!(
        run("re='(a|b)'; [[ a =~ $re ]] && echo M || echo N\n").0,
        "M\n"
    );
}

#[test]
fn alternation_operand() {
    assert_eq!(
        run("[[ /etc =~ ^\\~.*|^\\/.* ]] && echo M || echo N\n").0,
        "M\n"
    );
}

#[test]
fn grouping_not_regex_still_works() {
    assert_eq!(
        run("[[ -n a && ( -z \"\" || -n b ) ]] && echo M || echo N\n").0,
        "M\n"
    );
}

#[test]
fn multiline_dbracket_regex() {
    assert_eq!(run("[[ ab =~ (a)(b)\n]] && echo M || echo N\n").0, "M\n");
}
