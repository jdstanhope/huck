use super::getopts_step;

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn plain_option_then_advance() {
    // "-a" at optind=1, sp=1 (fresh): consume 'a', word done -> optind=2, sp=1.
    let s = getopts_step("ab", &args(&["-a"]), 1, 1);
    assert_eq!(s.name, "a");
    assert_eq!(s.optarg, None);
    assert_eq!((s.optind, s.sp), (2, 1));
    assert!(!s.done);
    assert!(s.error.is_none());
}

#[test]
fn clustered_options_walk_within_word() {
    // "-ab": first call consumes 'a' (optind stays 1, sp 1->3),
    let s1 = getopts_step("ab", &args(&["-ab"]), 1, 1);
    assert_eq!(s1.name, "a");
    assert_eq!((s1.optind, s1.sp), (1, 3));
    assert!(!s1.done);
    assert!(s1.error.is_none());
    // second call (sp=3) consumes 'b', word done -> optind=2, sp=1.
    let s2 = getopts_step("ab", &args(&["-ab"]), 1, 3);
    assert_eq!(s2.name, "b");
    assert_eq!((s2.optind, s2.sp), (2, 1));
}

#[test]
fn option_with_attached_arg() {
    // "-bval": 'b' takes an arg; rest of word "val" is OPTARG; optind=2.
    let s = getopts_step("ab:", &args(&["-bval"]), 1, 1);
    assert_eq!(s.name, "b");
    assert_eq!(s.optarg.as_deref(), Some("val"));
    assert_eq!((s.optind, s.sp), (2, 1));
}

#[test]
fn option_with_separate_arg() {
    // "-b" "val": arg is the next word; optind jumps to 3.
    let s = getopts_step("ab:", &args(&["-b", "val"]), 1, 1);
    assert_eq!(s.name, "b");
    assert_eq!(s.optarg.as_deref(), Some("val"));
    assert_eq!((s.optind, s.sp), (3, 1));
}

#[test]
fn exhausted_returns_done_question() {
    let s = getopts_step("ab", &args(&["-a"]), 2, 1); // optind past end
    assert_eq!(s.name, "?");
    assert!(s.done);
    assert_eq!(s.optind, 2); // optind.max(1), unchanged
    assert_eq!(s.optarg, None);
}

#[test]
fn non_option_terminates() {
    let s = getopts_step("ab", &args(&["foo"]), 1, 1);
    assert_eq!(s.name, "?");
    assert!(s.done);
    assert_eq!(s.optind, 1); // OPTIND unchanged
}

#[test]
fn double_dash_terminates_and_advances() {
    let s = getopts_step("ab", &args(&["--", "x"]), 1, 1);
    assert_eq!(s.name, "?");
    assert!(s.done);
    assert_eq!(s.optind, 2); // advanced past "--"
}

#[test]
fn invalid_option_verbose() {
    let s = getopts_step("ab", &args(&["-z"]), 1, 1);
    assert_eq!(s.name, "?");
    assert_eq!(s.optarg, None);
    assert!(!s.done); // invalid option is NOT terminating (rc 0, keep going)
    assert_eq!(s.optind, 2); // "-z" exhausts the word → optind advances
    assert_eq!(s.error.as_deref(), Some("illegal option -- z"));
}

#[test]
fn invalid_option_silent() {
    let s = getopts_step(":ab", &args(&["-z"]), 1, 1);
    assert_eq!(s.name, "?");
    assert_eq!(s.optarg.as_deref(), Some("z")); // silent: OPTARG = offending char
    assert!(!s.done); // still rc 0 (keep processing)
    assert_eq!(s.optind, 2);
    assert!(s.error.is_none());
}

#[test]
fn missing_arg_verbose() {
    let s = getopts_step("ab:", &args(&["-b"]), 1, 1);
    assert_eq!(s.name, "?");
    assert_eq!(s.optarg, None);
    assert_eq!(s.error.as_deref(), Some("option requires an argument -- b"));
}

#[test]
fn missing_arg_silent() {
    let s = getopts_step(":ab:", &args(&["-b"]), 1, 1);
    assert_eq!(s.name, ":");
    assert_eq!(s.optarg.as_deref(), Some("b"));
    assert!(s.error.is_none());
}
