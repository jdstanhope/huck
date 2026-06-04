//! Integration tests for v86 `shopt` builtin (M-08d).
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

/// Runs `script` through huck on stdin; returns (stdout, exit_code).
fn run(script: &str) -> (String, i32) {
    let mut child = Command::new(huck_bin())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn huck");
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(),
     out.status.code().unwrap_or(-1))
}

#[test]
fn shopt_oq_posix_returns_one_silently() {
    // The stock-bashrc case: `if ! shopt -oq posix`.
    let (out, rc) = run("shopt -oq posix; echo rc=$?\n");
    assert_eq!(out, "rc=1\n");
    assert_eq!(rc, 0);
}

#[test]
fn shopt_set_query_roundtrip() {
    assert_eq!(run("shopt -q nullglob; echo $?\n").0, "1\n");
    assert_eq!(run("shopt -s nullglob; shopt -q nullglob; echo $?\n").0, "0\n");
}

#[test]
fn shopt_inert_option_tracks() {
    // extglob is inert in huck but must round-trip.
    assert_eq!(run("shopt -s extglob; shopt -q extglob; echo $?\n").0, "0\n");
}

#[test]
fn shopt_invalid_name_rc_one() {
    let (_, rc) = run("shopt -s definitely_not_an_option\n");
    assert_eq!(rc, 1);
}

#[test]
fn shopt_query_prints_state() {
    assert_eq!(run("shopt -s dotglob; shopt dotglob\n").0, "dotglob        \ton\n");
}

#[test]
fn shopt_multi_query_rc_is_all_set() {
    // one on, one off → rc 1; both printed in table order.
    let (out, _) = run("shopt -s dotglob; shopt dotglob nullglob; echo rc=$?\n");
    assert_eq!(out, "dotglob        \ton\nnullglob       \toff\nrc=1\n");
}

#[test]
fn shopt_p_with_name_prints_reinput() {
    assert_eq!(run("shopt -p nullglob\n").0, "shopt -u nullglob\n");
    assert_eq!(run("shopt -s nullglob; shopt -p nullglob\n").0, "shopt -s nullglob\n");
}

#[test]
fn shopt_po_with_name_prints_reinput() {
    assert_eq!(run("shopt -po errexit\n").0, "set +o errexit\n");
}

#[test]
fn shopt_q_no_names_rc_zero() {
    assert_eq!(run("shopt -q; echo rc=$?\n").0, "rc=0\n");
    assert_eq!(run("shopt -oq; echo rc=$?\n").0, "rc=0\n");
}

// ---- Task 4: behavioral glob wiring (nullglob/dotglob/nocaseglob/failglob) ----
use std::fs;
use std::sync::atomic::{AtomicU32, Ordering};

static DIR_SEQ: AtomicU32 = AtomicU32::new(0);

/// Runs `script` with cwd set to a fresh temp dir containing the given files.
/// Returns (stdout, exit_code). Each call gets a unique dir so concurrently
/// running tests never collide.
fn run_in_dir(files: &[&str], script: &str) -> (String, i32) {
    let seq = DIR_SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("huck_shopt_{}_{}", std::process::id(), seq));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    for f in files { fs::write(dir.join(f), b"").unwrap(); }
    let mut child = Command::new(huck_bin())
        .current_dir(&dir)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn huck");
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    let _ = fs::remove_dir_all(&dir);
    (String::from_utf8_lossy(&out.stdout).into_owned(),
     out.status.code().unwrap_or(-1))
}

#[test]
fn nullglob_no_match_expands_empty() {
    // default: literal pattern survives.
    assert_eq!(run_in_dir(&["a.txt"], "echo no*match\n").0, "no*match\n");
    // nullglob: no match → no word; echo prints just a newline.
    assert_eq!(run_in_dir(&["a.txt"], "shopt -s nullglob; echo no*match\n").0, "\n");
}

#[test]
fn dotglob_includes_dotfiles() {
    // default: `*` skips .hidden.
    assert_eq!(run_in_dir(&["a", ".hidden"], "echo *\n").0, "a\n");
    // dotglob: `*` includes .hidden (sorted).
    assert_eq!(run_in_dir(&["a", ".hidden"], "shopt -s dotglob; echo *\n").0, ".hidden a\n");
}

#[test]
fn nocaseglob_matches_case_insensitively() {
    assert_eq!(run_in_dir(&["Abc.txt"], "echo a*\n").0, "a*\n"); // default: no match → literal
    assert_eq!(run_in_dir(&["Abc.txt"], "shopt -s nocaseglob; echo a*\n").0, "Abc.txt\n");
}

#[test]
fn failglob_no_match_aborts_command() {
    // `echo no*match` aborts (status 1, no stdout); the shell continues to the
    // NEXT line, so `echo after` still runs. (Commands are newline-separated:
    // bash aborts the rest of a `;`-joined LINE on a failglob no-match, but
    // continues across newlines — huck matches bash on the newline form. The
    // `;`-same-line whole-line-abort is a documented minor divergence; see the
    // M-08d note in bash-divergences.md.)
    let (out, _) = run_in_dir(&["a.txt"], "shopt -s failglob\necho no*match\necho after\n");
    assert_eq!(out, "after\n");
    let (out2, _) = run_in_dir(&["a.txt"], "shopt -s failglob\necho no*match\necho rc=$?\n");
    assert_eq!(out2, "rc=1\n");
}

// ---- Task 5: behavioral nocasematch wiring ([[ == / =~ ]] and case) ----

#[test]
fn nocasematch_double_bracket_eq() {
    assert_eq!(run("[[ ABC == abc ]] && echo m || echo n\n").0, "n\n");
    assert_eq!(run("shopt -s nocasematch; [[ ABC == abc ]] && echo m || echo n\n").0, "m\n");
}

#[test]
fn nocasematch_double_bracket_regex() {
    assert_eq!(run("[[ ABC =~ ^abc$ ]] && echo m || echo n\n").0, "n\n");
    assert_eq!(run("shopt -s nocasematch; [[ ABC =~ ^abc$ ]] && echo m || echo n\n").0, "m\n");
}

#[test]
fn nocasematch_case_statement() {
    assert_eq!(run("case ABC in abc) echo m;; *) echo n;; esac\n").0, "n\n");
    assert_eq!(run("shopt -s nocasematch; case ABC in abc) echo m;; *) echo n;; esac\n").0, "m\n");
}
