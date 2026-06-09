//! v118: `unset -v` dynamic-scope reveal/pop (M-115).
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

fn run(script: &str) -> String {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("huck_v118_{}_{}.sh", std::process::id(), n));
    {
        let mut f = std::fs::File::create(&path).expect("create temp script");
        f.write_all(script.as_bytes()).unwrap();
    }
    let out = Command::new(huck_bin()).arg(&path).stdin(Stdio::null()).output().expect("spawn huck");
    let _ = std::fs::remove_file(&path);
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn a_unset_eval_promotes_across_intervening_local() {
    let s = "inner(){ unset -v \"$1\"; eval $1=VAL; }\n\
             mid(){ local x=midval; inner x; echo \"mid:[$x]\"; }\n\
             outer(){ local x=orig; mid x; echo \"out:[$x]\"; }\n\
             outer\n";
    assert_eq!(run(s), "mid:[VAL]\nout:[VAL]\n");
}
#[test]
fn b_bare_unset_reveals_enclosing() {
    let s = "inner(){ unset -v \"$1\"; }\n\
             mid(){ local x=midval; inner x; echo \"mid:[${x-U}]\"; }\n\
             outer(){ local x=orig; mid x; echo \"out:[${x-U}]\"; }\n\
             outer\n";
    assert_eq!(run(s), "mid:[orig]\nout:[orig]\n");
}
#[test]
fn c_current_fn_local_stays_local() {
    let s = "inner(){ local x=il; unset -v \"$1\"; eval $1=VAL; }\n\
             outer(){ local x=orig; inner x; echo \"[$x]\"; }\n\
             outer\n";
    assert_eq!(run(s), "[orig]\n");
}
#[test]
fn d_three_intervening_locals() {
    let s = "leaf(){ unset -v \"$1\"; eval $1=VAL; }\n\
             a(){ local x=av; leaf x; }\n\
             b(){ local x=bv; a x; }\n\
             outer(){ local x=orig; b x; echo \"[$x]\"; }\n\
             outer\n";
    assert_eq!(run(s), "[orig]\n");
}
#[test]
fn e_global_only() {
    let s = "inner(){ unset -v \"$1\"; eval $1=VAL; }\nx=global\ninner x\necho \"[$x]\"\n";
    assert_eq!(run(s), "[VAL]\n");
}
#[test]
fn f_current_fn_local_reads_unset_after() {
    let s = "inner(){ local x=iv; unset -v x; echo \"in:[${x-U}]\"; }\n\
             outer(){ local x=orig; inner; echo \"out:[$x]\"; }\n\
             outer\n";
    assert_eq!(run(s), "in:[U]\nout:[orig]\n");
}
#[test]
fn g_unset_skips_intervening_nonlocal_frame() {
    let s = "leaf(){ unset -v \"$1\"; eval $1=VAL; }\n\
             pass(){ leaf \"$1\"; }\n\
             mid(){ local x=mv; pass x; echo \"mid:[$x]\"; }\n\
             outer(){ local x=orig; mid x; echo \"out:[$x]\"; }\n\
             outer\n";
    assert_eq!(run(s), "mid:[VAL]\nout:[VAL]\n");
}
#[test]
fn h_caller_reassigns_after_callee_unset() {
    let s = "inner(){ unset -v \"$1\"; }\n\
             mid(){ local x=mv; inner x; x=re; echo \"mid:[$x]\"; }\n\
             outer(){ local x=orig; mid x; echo \"out:[$x]\"; }\n\
             outer\n";
    assert_eq!(run(s), "mid:[re]\nout:[re]\n");
}
#[test]
fn i_unset_reveals_unset_when_enclosing_local_shadowed_nothing() {
    // mid's bare `local x` shadowed an unset global (snapshot None); inner's
    // unset pops mid's local and reveals "unset" (the Some(None) reveal arm).
    let s = "inner(){ unset -v \"$1\"; echo \"in:${x-U}\"; }\n\
             mid(){ local x=mv; inner x; echo \"mid:${x-U}\"; }\n\
             outer(){ mid x; echo \"out:${x-U}\"; }\n\
             outer\n";
    assert_eq!(run(s), "in:U\nmid:U\nout:U\n");
}
