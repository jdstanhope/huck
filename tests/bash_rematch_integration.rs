//! v122: BASH_REMATCH array population after [[ =~ ]] (M-14 sub-feature).
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
static COUNTER: AtomicU64 = AtomicU64::new(0);
fn huck_bin() -> &'static str {
    env!("CARGO_BIN_EXE_huck")
}
fn run(script: &str) -> String {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("huck_v122_{}_{}.sh", std::process::id(), n));
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
fn whole_match_and_groups() {
    assert_eq!(
        run(
            "[[ abcdef =~ b(c)(d) ]]\necho \"n=${#BASH_REMATCH[@]} 0=[${BASH_REMATCH[0]}] 1=[${BASH_REMATCH[1]}] 2=[${BASH_REMATCH[2]}]\"\n"
        ),
        "n=3 0=[bcd] 1=[c] 2=[d]\n"
    );
}
#[test]
fn no_match_clears() {
    assert_eq!(
        run(
            "BASH_REMATCH=(stale x y)\n[[ xyz =~ nomatch ]]\necho \"rc=$? n=${#BASH_REMATCH[@]} 0=[${BASH_REMATCH[0]}]\"\n"
        ),
        "rc=1 n=0 0=[]\n"
    );
}
#[test]
fn nonparticipating_group_is_empty() {
    assert_eq!(
        run(
            "[[ ab =~ (a)|(b) ]]\necho \"0=[${BASH_REMATCH[0]}] 1=[${BASH_REMATCH[1]}] 2=[${BASH_REMATCH[2]}]\"\n"
        ),
        "0=[a] 1=[a] 2=[]\n"
    );
}
#[test]
fn matched_substring() {
    assert_eq!(
        run("[[ foobar =~ o+ ]]\necho \"[${BASH_REMATCH[0]}]\"\n"),
        "[oo]\n"
    );
}
#[test]
fn quoted_regex_sets_rematch() {
    assert_eq!(
        run("[[ \"a.b\" =~ \"a.b\" ]]\necho \"rc=$? [${BASH_REMATCH[0]}]\"\n"),
        "rc=0 [a.b]\n"
    );
}
#[test]
fn longopt_style_extraction() {
    let s = "for w in --all -x --almost-all; do [[ $w =~ (--[a-z-]+) ]] && printf '%s\\n' \"${BASH_REMATCH[1]}\"; done\n";
    assert_eq!(run(s), "--all\n--almost-all\n");
}
