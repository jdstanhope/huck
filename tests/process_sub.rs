// End-to-end process substitution, driven through the binary.
use std::process::Command;
fn huck(script: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_huck"))
        .args(["-c", script]).output().expect("run huck");
    String::from_utf8_lossy(&out.stdout).into_owned()
}
#[test] fn cat_input_process_sub() { assert_eq!(huck("cat <(echo hi)"), "hi\n"); }
#[test] fn two_input_process_subs() { assert_eq!(huck("cat <(echo a) <(echo b)"), "a\nb\n"); }
#[test] fn redirect_source_process_sub() { assert_eq!(huck("wc -c < <(printf hello)").trim(), "5"); }
#[test] fn while_read_from_process_sub() {
    assert_eq!(huck("while read x; do echo \"[$x]\"; done < <(seq 3)"), "[1]\n[2]\n[3]\n");
}
#[test] fn output_process_sub_tee() {
    assert_eq!(huck("printf 'foo\\n' | tee >(cat) >/dev/null"), "foo\n");
}
#[test] fn nested_process_sub() { assert_eq!(huck("cat <(cat <(echo deep))"), "deep\n"); }
#[test] fn quoted_is_literal() { assert_eq!(huck("echo \"<(echo hi)\""), "<(echo hi)\n"); }
