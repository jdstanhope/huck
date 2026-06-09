//! v119: POSIX bracket character classes in glob patterns (M-54).
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);
fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }
fn run(script: &str) -> String {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("huck_v119_{}_{}.sh", std::process::id(), n));
    { let mut f = std::fs::File::create(&path).unwrap(); f.write_all(script.as_bytes()).unwrap(); }
    let out = Command::new(huck_bin()).arg(&path).stdin(Stdio::null()).output().unwrap();
    let _ = std::fs::remove_file(&path);
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn subst_digit_alpha_space() {
    assert_eq!(run("s=\"a1 b2\"\necho \"[${s//[[:digit:]]/_}]\"\n"), "[a_ b_]\n");
    assert_eq!(run("s=\"a1 b2\"\necho \"[${s//[[:alpha:]]/_}]\"\n"), "[_1 _2]\n");
    assert_eq!(run("s=\"a b\tc\"\necho \"[${s//[[:space:]]/_}]\"\n"), "[a_b_c]\n");
}
#[test]
fn subst_upper_alnum_punct() {
    assert_eq!(run("s=\"aXbY\"\necho \"[${s//[[:upper:]]/_}]\"\n"), "[a_b_]\n");
    assert_eq!(run("s=\"a.b!c\"\necho \"[${s//[[:punct:]]/_}]\"\n"), "[a_b_c]\n");
    // a,1,b are alnum (→X); the literal `_` is NOT alnum (kept) → "XX_X".
    assert_eq!(run("s=\"a1_b\"\necho \"[${s//[[:alnum:]]/X}]\"\n"), "[XX_X]\n");
}
#[test]
fn case_and_dbracket_membership() {
    assert_eq!(run("case \" \" in [[:space:]]) echo SP;; *) echo no;; esac\n"), "SP\n");
    assert_eq!(run("case \"5\" in [[:space:]]) echo SP;; *) echo no;; esac\n"), "no\n");
    assert_eq!(run("[[ \"x\" == [[:alpha:]] ]] && echo Y || echo N\n"), "Y\n");
    assert_eq!(run("[[ \"5\" == [[:alpha:]] ]] && echo Y || echo N\n"), "N\n");
}
#[test]
fn negation_and_mixed() {
    assert_eq!(run("[[ \"x\" == [^[:digit:]] ]] && echo Y || echo N\n"), "Y\n");
    assert_eq!(run("s=\"a5_b\"\necho \"[${s//[[:digit:]_]/X}]\"\n"), "[aXXb]\n");
}
#[test]
fn extglob_off_classes_still_work() {
    assert_eq!(run("shopt -u extglob\ncase \"5\" in [[:digit:]]) echo D;; *) echo no;; esac\n"), "D\n");
}
#[test]
fn pathname_upper_class() {
    let s = "d=$(mktemp -d); touch \"$d\"/Afile \"$d\"/bfile \"$d\"/Cfile\n\
             cd \"$d\"; for f in [[:upper:]]*; do echo \"$f\"; done; rm -rf \"$d\"\n";
    assert_eq!(run(s), "Afile\nCfile\n");
}
