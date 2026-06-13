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
#[test] fn procsub_arg_in_pipeline() {
    // arg procsub on an external-command stage: resolve() runs in the parent, so the
    // procsub is pushed to the parent's procsub_pending and must be drained after wait
    assert_eq!(huck("cat <(echo a) | cat"), "a\n");
}
#[test] fn procsub_redirect_in_pipeline() {
    // redirect-target procsub on a pipeline stage: stdin override takes effect
    assert_eq!(huck("echo ignored | cat < <(echo fromsub)"), "fromsub\n");
}
#[test] fn pipeline_procsub_no_zombies() {
    // many pipeline procsubs must not leave zombies (the shell reaps them)
    let out = huck("for i in 1 2 3 4 5; do cat <(echo x) | cat >/dev/null; done; ps -o stat= --ppid $$ 2>/dev/null | grep -c defunct || echo 0");
    let last = out.lines().last().unwrap_or("0").trim();
    assert_eq!(last, "0", "expected 0 zombies, full output: {out:?}");
}
