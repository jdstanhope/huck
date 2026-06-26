//! v111: getopts builtin integration tests (M-106).
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }
fn run(script: &str) -> (String, String, i32) {
    let mut child = Command::new(huck_bin())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().expect("spawn huck");
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(),
     String::from_utf8_lossy(&out.stderr).into_owned(),
     out.status.code().unwrap_or(-1))
}

#[test]
fn simple_loop_parses_options() {
    let (out, _e, _c) = run(
        "set -- -a -b val -c arg1\n\
         while getopts \"ab:c\" opt; do echo \"$opt:${OPTARG-}\"; done\n\
         echo \"rest=${@:$OPTIND}\"\n");
    assert_eq!(out, "a:\nb:val\nc:\nrest=arg1\n", "out: {out}");
}

#[test]
fn clustered_options() {
    let (out, _e, _c) = run(
        "set -- -abc\n\
         while getopts \"abc\" o; do echo \"$o\"; done\n");
    assert_eq!(out, "a\nb\nc\n", "out: {out}");
}

#[test]
fn attached_argument() {
    let (out, _e, _c) = run(
        "set -- -bVAL\n\
         getopts \"b:\" o; echo \"$o=$OPTARG\"\n");
    assert_eq!(out, "b=VAL\n", "out: {out}");
}

#[test]
fn double_dash_terminates() {
    let (out, _e, _c) = run(
        "set -- -a -- -b\n\
         while getopts \"ab\" o; do echo \"$o\"; done\n\
         echo \"optind=$OPTIND\"\n");
    // -a consumed; -- ends options; OPTIND points past -- (at -b, index 3).
    assert_eq!(out, "a\noptind=3\n", "out: {out}");
}

#[test]
fn invalid_option_verbose_sets_question() {
    let (out, err, _c) = run(
        "set -- -z\n\
         getopts \"ab\" o 2>/dev/null; echo \"o=$o\"\n");
    assert_eq!(out, "o=?\n", "out: {out}");
    assert!(!err.contains("illegal"), "stderr should be suppressed: {err}");
}

#[test]
fn missing_arg_silent_mode() {
    let (out, _e, _c) = run(
        "set -- -b\n\
         getopts \":ab:\" o; echo \"o=$o OPTARG=$OPTARG\"\n");
    assert_eq!(out, "o=: OPTARG=b\n", "out: {out}");
}

#[test]
fn no_args_uses_positional_params() {
    let (out, _e, _c) = run(
        "f() { while getopts \"x\" o; do echo \"$o\"; done; }\n\
         f -x -x\n");
    assert_eq!(out, "x\nx\n", "out: {out}");
}

#[test]
fn local_optind_resets_per_function() {
    // bash_completion shape: local OPTIND=1 restarts parsing cleanly each call.
    let (out, _e, _c) = run(
        "f() { local OPTIND=1 o; while getopts \"a\" o; do :; done; echo \"f-optind=$OPTIND\"; }\n\
         set -- -a -a -a\n\
         getopts \"a\" top; echo \"top=$top top-optind=$OPTIND\"\n\
         f -a\n\
         echo \"after top-optind=$OPTIND\"\n");
    // top consumes one -a (OPTIND 1->2); f has its own local OPTIND.
    assert_eq!(out, "top=a top-optind=2\nf-optind=2\nafter top-optind=2\n", "out: {out}");
}

#[test]
fn regression_get_comp_words_by_ref_shape() {
    // The exact bash_completion cascade: -n : cur prev must resolve, no error.
    let (out, err, _c) = run(
        "f() {\n\
           local OPTIND=1 flag exclude\n\
           while getopts \"c:i:n:p:w:\" flag \"$@\"; do\n\
             case $flag in n) exclude=$OPTARG;; esac\n\
           done\n\
           while [[ $# -ge $OPTIND ]]; do\n\
             case ${!OPTIND} in\n\
               cur|prev) echo \"arg:${!OPTIND}\";;\n\
               *) echo \"bash_completion: \\`${!OPTIND}': unknown argument\" >&2; return 1;;\n\
             esac\n\
             ((OPTIND += 1))\n\
           done\n\
           echo \"exclude=$exclude\"\n\
         }\n\
         f -n : cur prev\n");
    assert_eq!(out, "arg:cur\narg:prev\nexclude=:\n", "out: {out}");
    assert!(!err.contains("unknown argument"), "cascade still present: {err}");
}

#[test]
fn nested_getopts_does_not_corrupt_mid_cluster_caller() {
    // A caller mid-cluster (-abc, after 'a') invokes a function that runs its
    // own getopts loop. The caller must resume at 'b'/'c' (not panic, not
    // re-process 'a') — the within-word cursor is per-call-context. Matches
    // bash; previously panicked (index out of bounds).
    let (out, _e, c) = run(
        "nested() { local OPTIND=1 n; while getopts \"xy\" n; do echo \"  nested:$n\"; done; }\n\
         set -- -abc\n\
         while getopts \"abc\" o; do\n\
           echo \"caller:$o\"\n\
           if [ \"$o\" = a ]; then nested -x -y; fi\n\
         done\n");
    assert_eq!(out, "caller:a\n  nested:x\n  nested:y\ncaller:b\ncaller:c\n", "out: {out}");
    assert_eq!(c, 0, "should not panic/abort");
}

#[test]
fn usage_error_drops_huck_prefix_and_fixes_arg_ellipsis() {
    // Too few operands → builtin usage error, no shell prologue, rc 2.
    let (_o, e, c) = run("getopts\n");
    assert_eq!(e, "getopts: usage: getopts optstring name [arg ...]\n", "stderr: {e:?}");
    assert_eq!(c, 2);
}

#[test]
fn invalid_option_to_getopts_itself_is_rejected() {
    // getopts has no options of its own; `-a` is invalid → error + usage, rc 2.
    // `echo "rc=$?"` captures getopts' status into stdout; the script's own
    // exit (c) is the echo's success (0).
    let (o, e, c) = run("getopts -a opts name\necho \"rc=$?\"\n");
    assert!(e.contains("-a: invalid option"), "stderr: {e:?}");
    assert!(e.contains("getopts: usage: getopts optstring name [arg ...]"), "stderr: {e:?}");
    assert_eq!(o, "rc=2\n", "stdout: {o:?}");
    assert_eq!(c, 0, "script's own exit is the echo's success");
}

#[test]
fn invalid_name_still_binds_optind() {
    // bash binds OPTIND from the parsed option BEFORE validating the name var,
    // so an invalid name still advances OPTIND (here: parsed `-a` → OPTIND 2).
    let (o, e, _c) = run("set -- -a\ngetopts ab bad-name\necho \"oi=$OPTIND\"\n");
    assert_eq!(o, "oi=2\n", "stdout: {o:?}");
    assert!(e.contains("`bad-name': not a valid identifier"), "stderr: {e:?}");
}
