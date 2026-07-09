//! v232: command-position-aware alias expansion — regression coverage for
//! the v231 bug where a case-pattern word matching an alias broke parsing
//! when sourcing files like ~/.bashrc / nvm bash_completion.
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);
fn huck_bin() -> &'static str {
    env!("CARGO_BIN_EXE_huck")
}

/// Run `script` as a file arg (non-interactive). Returns (stdout, stderr, code).
fn run_file(script: &str) -> (String, String, i32) {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("huck_v232cp_{}_{}_.sh", std::process::id(), n));
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
fn case_pattern_alias_does_not_break_parsing() {
    // The exact shape from nvm bash_completion: an aliased name (`ls`) used
    // as a case pattern after `|`. Must parse and run cleanly.
    let script = "shopt -s expand_aliases\n\
                  alias ls='ls --color'\n\
                  f() { case \"$1\" in use | ls | list) echo HIT ;; *) echo MISS ;; esac; }\n\
                  f ls\n\
                  f other\n";
    let (o, e, c) = run_file(script);
    assert_eq!(c, 0, "stderr: {e}");
    assert!(!e.contains("syntax error"), "unexpected syntax error: {e}");
    assert_eq!(o, "HIT\nMISS\n");
}

#[test]
fn alias_expands_in_case_body() {
    let script = "shopt -s expand_aliases\n\
                  alias greet='echo HELLO'\n\
                  case x in x) greet ;; esac\n";
    let (o, _, c) = run_file(script);
    assert_eq!(c, 0);
    assert_eq!(o, "HELLO\n");
}

#[test]
fn alias_expands_after_reserved_word() {
    let script = "shopt -s expand_aliases\n\
                  alias greet='echo HELLO'\n\
                  if true; then greet; fi\n";
    let (o, _, c) = run_file(script);
    assert_eq!(c, 0);
    assert_eq!(o, "HELLO\n");
}

#[test]
fn for_list_words_not_alias_expanded() {
    let script = "shopt -s expand_aliases\n\
                  alias one='echo BAD'\n\
                  for w in one two; do echo \"$w\"; done\n";
    let (o, _, c) = run_file(script);
    assert_eq!(c, 0);
    assert_eq!(o, "one\ntwo\n");
}
