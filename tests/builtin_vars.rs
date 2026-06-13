use std::process::Command;

fn huck(s: &str) -> String {
    let o = Command::new(env!("CARGO_BIN_EXE_huck"))
        .args(["-c", s])
        .output()
        .unwrap();
    String::from_utf8_lossy(&o.stdout).into_owned()
}

#[test]
fn random_in_range() {
    for _ in 0..20 {
        let n: i64 = huck("echo $RANDOM").trim().parse().unwrap();
        assert!((0..=32767).contains(&n), "RANDOM out of range: {n}");
    }
}

#[test]
fn random_reseed_is_deterministic() {
    let a = huck("RANDOM=42; echo $RANDOM $RANDOM $RANDOM");
    let b = huck("RANDOM=42; echo $RANDOM $RANDOM $RANDOM");
    assert_eq!(a, b);
    assert_ne!(a, huck("RANDOM=99; echo $RANDOM $RANDOM $RANDOM"));
}

#[test]
fn seconds_starts_zero_and_resets() {
    assert_eq!(huck("echo $SECONDS").trim(), "0");
    let n: i64 = huck("SECONDS=5; echo $SECONDS").trim().parse().unwrap();
    assert!(n >= 5, "got {n}");
}

#[test]
fn epochseconds_is_recent() {
    let n: i64 = huck("echo $EPOCHSECONDS").trim().parse().unwrap();
    assert!(n > 1_700_000_000, "got {n}");
}

#[test]
fn bashpid_top_level_equals_dollar_and_differs_in_subshell() {
    assert_eq!(huck("echo $(( $BASHPID == $$ ))").trim(), "1");
    assert_eq!(
        huck("( [ \"$BASHPID\" != \"$$\" ] && echo diff || echo same )").trim(),
        "diff"
    );
}
