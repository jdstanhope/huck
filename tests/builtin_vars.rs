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

#[test]
fn ids_match_bash() {
    for v in ["UID", "EUID", "PPID"] {
        let frag = format!("echo ${}", v);
        let b = std::process::Command::new("bash").args(["-c", &frag]).output().unwrap();
        assert_eq!(huck(&frag).trim(), String::from_utf8_lossy(&b.stdout).trim(), "{v}");
    }
}

#[test]
fn bash_version_and_huck_version() {
    assert_eq!(huck("[ -n \"$BASH_VERSION\" ] && echo yes").trim(), "yes");
    assert_eq!(huck("echo ${BASH_VERSINFO[0]}").trim(), "5");
    assert_eq!(huck("echo $HUCK_VERSION").trim(), env!("CARGO_PKG_VERSION"));
}

#[test]
fn platform_and_host_present() {
    for v in ["HOSTNAME", "HOSTTYPE", "OSTYPE", "MACHTYPE", "BASH"] {
        assert!(!huck(&format!("echo ${}", v)).trim().is_empty(), "{v} empty");
    }
    assert!(!huck("echo ${GROUPS[0]}").trim().is_empty());
}

#[test]
fn uid_is_readonly() {
    let real = huck("echo $UID");
    let after = huck("UID=99999 2>/dev/null; echo $UID");
    assert_eq!(after.trim(), real.trim(), "UID must be readonly (unchanged)");
}

#[test]
fn shlvl_increments_from_env() {
    let o = std::process::Command::new(env!("CARGO_BIN_EXE_huck"))
        .args(["-c", "echo $SHLVL"]).env("SHLVL", "5").output().unwrap();
    assert_eq!(String::from_utf8_lossy(&o.stdout).trim(), "6");
}

#[test]
fn compgen_v_lists_dynamic_specials() {
    let out = huck("compgen -v");
    for v in ["RANDOM", "SECONDS", "LINENO", "BASHPID", "UID", "BASH_VERSION", "BASH_SOURCE"] {
        assert!(out.lines().any(|l| l == v), "compgen -v should list {v}; got:\n{out}");
    }
}

#[test]
fn compgen_v_omits_funcname_at_top_level() {
    // bash omits FUNCNAME from top-level compgen -v; huck should too (not in registry, unset at top level)
    let out = huck("compgen -v");
    assert!(!out.lines().any(|l| l == "FUNCNAME"), "FUNCNAME should NOT be listed at top level");
}
