use super::*;
use crate::error_emit::Diag;

#[test]
fn shopt_table_has_57_entries() {
    assert_eq!(SHOPT_TABLE.len(), 57);
}

#[test]
fn shopt_defaults_match_bash() {
    let o = ShoptOptions::default();
    // default-off
    assert_eq!(o.get("nullglob"), Some(false));
    assert_eq!(o.get("dotglob"), Some(false));
    assert_eq!(o.get("extglob"), Some(false));
    // default-on
    assert_eq!(o.get("checkwinsize"), Some(true));
    assert_eq!(o.get("interactive_comments"), Some(true));
    assert_eq!(o.get("sourcepath"), Some(true));
    // exactly 13 default-on
    assert_eq!(SHOPT_TABLE.iter().filter(|e| e.default).count(), 13);
    // unknown
    assert_eq!(o.get("bogus"), None);
}

#[test]
fn shopt_set_and_read_back() {
    let mut o = ShoptOptions::default();
    assert!(o.set("nullglob", true));
    assert_eq!(o.get("nullglob"), Some(true));
    assert!(!o.set("bogus", true)); // unknown → false (not applied)
}

#[test]
fn shell_glob_opts_reflects_shopt() {
    let mut shell = Shell::new();
    shell.shopt_options.set("nullglob", true);
    shell.shopt_options.set("dotglob", true);
    let g = shell.glob_opts();
    assert!(g.nullglob && g.dotglob && !g.nocaseglob && !g.failglob);
    assert!(!shell.nocasematch());
    shell.shopt_options.set("nocasematch", true);
    assert!(shell.nocasematch());
}

#[test]
fn dollar_dash_includes_v_when_verbose() {
    let mut sh = Shell::new();
    assert!(!sh.dollar_dash_value().contains('v'));
    sh.shell_options.verbose = true;
    assert!(sh.dollar_dash_value().contains('v'));
}

#[test]
fn dollar_dash_includes_x_when_xtrace() {
    let mut sh = Shell::new();
    assert!(!sh.dollar_dash_value().contains('x'));
    sh.shell_options.xtrace = true;
    assert!(sh.dollar_dash_value().contains('x'));
}

#[test]
fn dollar_dash_x_after_v() {
    let mut sh = Shell::new();
    sh.shell_options.verbose = true;
    sh.shell_options.xtrace = true;
    let d = sh.dollar_dash_value();
    let v = d.find('v').unwrap();
    let x = d.find('x').unwrap();
    assert!(v < x, "expected v before x in {d:?}");
}

#[test]
fn dollar_dash_v_after_u() {
    let mut sh = Shell::new();
    sh.shell_options.nounset = true;
    sh.shell_options.verbose = true;
    let d = sh.dollar_dash_value();
    assert!(d.find('u').unwrap() < d.find('v').unwrap(), "got {d:?}");
}

#[test]
fn dollar_dash_c_after_x() {
    let mut sh = Shell::new();
    sh.shell_options.xtrace = true;
    sh.shell_options.noclobber = true;
    let d = sh.dollar_dash_value();
    let xi = d.find('x').expect("x present");
    let ci = d.find('C').expect("C present");
    assert!(ci > xi, "C must come after x in $-: got {d:?}");
}

#[test]
fn noclobber_off_by_default() {
    let sh = Shell::new();
    assert!(!sh.shell_options.noclobber);
    assert!(!sh.dollar_dash_value().contains('C'));
}

#[test]
fn noclobber_shows_in_dollar_dash() {
    let mut sh = Shell::new();
    sh.shell_options.noclobber = true;
    assert!(sh.dollar_dash_value().contains('C'));
}

#[test]
fn resolve_histsize_bash_semantics() {
    let mut s = Shell::new();
    assert_eq!(s.resolve_histsize(), Some(1000)); // unset -> default
    s.set("HISTSIZE", "".to_string());
    assert_eq!(s.resolve_histsize(), Some(1000)); // empty -> default
    s.set("HISTSIZE", "abc".to_string());
    assert_eq!(s.resolve_histsize(), Some(1000)); // non-numeric -> default
    s.set("HISTSIZE", "0".to_string());
    assert_eq!(s.resolve_histsize(), Some(0)); // zero -> empty
    s.set("HISTSIZE", "200".to_string());
    assert_eq!(s.resolve_histsize(), Some(200)); // positive -> cap
    s.set("HISTSIZE", "-1".to_string());
    assert_eq!(s.resolve_histsize(), None); // negative -> unlimited
}

#[test]
fn resolve_histfilesize_bash_semantics() {
    let mut s = Shell::new();
    s.set("HISTSIZE", "200".to_string());
    assert_eq!(s.resolve_histfilesize(), Some(200)); // unset -> effective HISTSIZE
    s.set("HISTFILESIZE", "50".to_string());
    assert_eq!(s.resolve_histfilesize(), Some(50)); // positive -> cap
    s.set("HISTFILESIZE", "0".to_string());
    assert_eq!(s.resolve_histfilesize(), Some(0)); // zero -> empty file
    s.set("HISTFILESIZE", "-1".to_string());
    assert_eq!(s.resolve_histfilesize(), None); // negative -> inhibit
    s.set("HISTFILESIZE", "abc".to_string());
    assert_eq!(s.resolve_histfilesize(), None); // non-numeric -> inhibit
}

#[test]
fn apply_case_fold_lower_upper_and_none() {
    assert_eq!(apply_case_fold(None, "AbC".to_string()), "AbC");
    assert_eq!(
        apply_case_fold(Some(CaseFold::Lower), "AbC".to_string()),
        "abc"
    );
    assert_eq!(
        apply_case_fold(Some(CaseFold::Upper), "AbC".to_string()),
        "ABC"
    );
    // idempotent
    assert_eq!(
        apply_case_fold(Some(CaseFold::Lower), "abc".to_string()),
        "abc"
    );
}

#[test]
fn storage_mutators_apply_case_fold() {
    let mut shell = Shell::new();

    // scalar via try_set
    shell.set_case_fold("s", Some(CaseFold::Lower));
    shell.try_set("s", "ABCdef".to_string()).unwrap();
    assert_eq!(shell.get("s"), Some("abcdef"));

    // scalar += (try_set with concatenated value) folds the whole result
    shell.try_set("s", "abcdef".to_string() + "GHI").unwrap();
    assert_eq!(shell.get("s"), Some("abcdefghi"));

    // indexed element
    shell.set_case_fold("arr", Some(CaseFold::Lower));
    shell
        .set_indexed_element("arr", 1, "XYZ".to_string())
        .unwrap();
    assert_eq!(
        shell.lookup_indexed_element("arr", 1).as_deref(),
        Some("xyz")
    );

    // associative value folded, key NOT folded
    // must declare as associative first (set_case_fold creates a Scalar)
    shell.declare_associative("m").unwrap();
    shell.set_case_fold("m", Some(CaseFold::Lower));
    shell
        .set_associative_element("m", "Key".to_string(), "VALUE".to_string())
        .unwrap();
    assert_eq!(
        shell
            .get_associative("m")
            .unwrap()
            .iter()
            .find(|(k, _)| k == "Key")
            .map(|(_, v)| v.as_str()),
        Some("value")
    );

    // whole-array literal via replace_indexed, attribute preserved
    shell.set_case_fold("lit", Some(CaseFold::Lower));
    let mut map = std::collections::BTreeMap::new();
    map.insert(0usize, "ABC".to_string());
    map.insert(1usize, "DeF".to_string());
    shell.replace_indexed("lit", map).unwrap();
    assert_eq!(
        shell.lookup_indexed_element("lit", 0).as_deref(),
        Some("abc")
    );
    assert_eq!(
        shell.lookup_indexed_element("lit", 1).as_deref(),
        Some("def")
    );
    assert_eq!(shell.case_fold_of("lit"), Some(CaseFold::Lower)); // preserved

    // upper attribute through array append (extend_indexed)
    shell.set_case_fold("app", Some(CaseFold::Upper));
    let mut em = std::collections::BTreeMap::new();
    em.insert(0usize, "abc".to_string());
    shell.extend_indexed("app", em).unwrap();
    assert_eq!(
        shell.lookup_indexed_element("app", 0).as_deref(),
        Some("ABC")
    );

    // whole associative-array literal via replace_associative, attribute preserved
    shell.declare_associative("am").unwrap();
    shell.set_case_fold("am", Some(CaseFold::Upper));
    shell
        .replace_associative("am", vec![("k".to_string(), "abc".to_string())])
        .unwrap();
    assert_eq!(
        shell
            .get_associative("am")
            .unwrap()
            .iter()
            .find(|(k, _)| k == "k")
            .map(|(_, v)| v.as_str()),
        Some("ABC")
    );
    assert_eq!(shell.case_fold_of("am"), Some(CaseFold::Upper)); // preserved
}

#[test]
fn set_case_fold_creates_and_clears() {
    let mut shell = Shell::new();
    // create-if-absent, like mark_integer
    shell.set_case_fold("x", Some(CaseFold::Lower));
    assert_eq!(shell.case_fold_of("x"), Some(CaseFold::Lower));
    // overwrite (later-wins mutual exclusivity is handled by the caller)
    shell.set_case_fold("x", Some(CaseFold::Upper));
    assert_eq!(shell.case_fold_of("x"), Some(CaseFold::Upper));
    // clear
    shell.set_case_fold("x", None);
    assert_eq!(shell.case_fold_of("x"), None);
    // unknown var reads None
    assert_eq!(shell.case_fold_of("nope"), None);
}

// ── v159 Task 4: funnel-uniformity unit tests ────────────────────────────

/// Proves that assign() applies case-fold on every storage path:
/// scalar whole-variable, indexed element, and whole indexed-array literal.
#[test]
fn assign_funnel_applies_case_fold_on_every_path() {
    let mut shell = Shell::new();

    // scalar whole-variable path
    shell.set_case_fold("s", Some(CaseFold::Upper));
    shell
        .assign(
            AssignDest::Whole("s".into()),
            AssignKind::Set,
            AssignSource::Scalar("abc".into()),
        )
        .unwrap();
    assert_eq!(shell.get("s"), Some("ABC"));

    // indexed element path
    shell.set_case_fold("a", Some(CaseFold::Upper));
    shell
        .assign(
            AssignDest::Element {
                name: "a".into(),
                sub: Subscript::Index(2),
            },
            AssignKind::Set,
            AssignSource::Scalar("xy".into()),
        )
        .unwrap();
    assert_eq!(shell.lookup_indexed_element("a", 2).as_deref(), Some("XY"));

    // whole indexed-array literal path
    let mut m = std::collections::BTreeMap::new();
    m.insert(0usize, "lo".to_string());
    shell.set_case_fold("b", Some(CaseFold::Upper));
    shell
        .assign(
            AssignDest::Whole("b".into()),
            AssignKind::Set,
            AssignSource::Indexed(m),
        )
        .unwrap();
    assert_eq!(shell.lookup_indexed_element("b", 0).as_deref(), Some("LO"));
}

/// L-49: an integer-flagged array arith-coerces element VALUES on every
/// storage path (whole indexed literal, indexed element, whole associative
/// literal, associative element); a non-integer array stays literal.
#[test]
fn assign_funnel_integer_coerces_array_values() {
    let mut shell = Shell::new();

    // whole indexed-array literal coerces each value
    shell.mark_integer("a");
    let mut m = std::collections::BTreeMap::new();
    m.insert(0usize, "2+3".to_string());
    m.insert(1usize, "4*5".to_string());
    shell
        .assign(
            AssignDest::Whole("a".into()),
            AssignKind::Set,
            AssignSource::Indexed(m),
        )
        .unwrap();
    assert_eq!(shell.lookup_indexed_element("a", 0).as_deref(), Some("5"));
    assert_eq!(shell.lookup_indexed_element("a", 1).as_deref(), Some("20"));
    assert!(shell.is_integer("a")); // flag survives the replace

    // indexed element coerces
    shell
        .assign(
            AssignDest::Element {
                name: "a".into(),
                sub: Subscript::Index(2),
            },
            AssignKind::Set,
            AssignSource::Scalar("6/2".into()),
        )
        .unwrap();
    assert_eq!(shell.lookup_indexed_element("a", 2).as_deref(), Some("3"));

    // whole associative literal coerces VALUES (not keys)
    shell.declare_associative("m").unwrap();
    shell.mark_integer("m");
    shell
        .assign(
            AssignDest::Whole("m".into()),
            AssignKind::Set,
            AssignSource::Associative(vec![("x".into(), "2+3".into())]),
        )
        .unwrap();
    assert_eq!(
        shell
            .get_associative("m")
            .unwrap()
            .iter()
            .find(|(k, _)| k == "x")
            .map(|(_, v)| v.as_str()),
        Some("5")
    );
    assert!(shell.is_integer("m")); // flag survives the assoc replace

    // associative element coerces
    shell
        .assign(
            AssignDest::Element {
                name: "m".into(),
                sub: Subscript::Key("k".into()),
            },
            AssignKind::Set,
            AssignSource::Scalar("10-1".into()),
        )
        .unwrap();
    assert_eq!(
        shell
            .get_associative("m")
            .unwrap()
            .iter()
            .find(|(k, _)| k == "k")
            .map(|(_, v)| v.as_str()),
        Some("9")
    );

    // non-integer array stays literal
    let mut m2 = std::collections::BTreeMap::new();
    m2.insert(0usize, "2+3".to_string());
    shell
        .assign(
            AssignDest::Whole("plain".into()),
            AssignKind::Set,
            AssignSource::Indexed(m2),
        )
        .unwrap();
    assert_eq!(
        shell.lookup_indexed_element("plain", 0).as_deref(),
        Some("2+3")
    );
}

/// Proves that assign() enforces readonly on every write path (scalar).
#[test]
fn assign_funnel_readonly_blocks_all_paths() {
    let mut shell = Shell::new();
    shell.try_set("r", "init".into()).unwrap();
    shell.mark_readonly("r");
    assert!(
        shell
            .assign(
                AssignDest::Whole("r".into()),
                AssignKind::Set,
                AssignSource::Scalar("x".into()),
            )
            .is_err()
    );
    assert_eq!(shell.get("r"), Some("init")); // value unchanged
}

#[test]
fn resolve_nameref_covers_plain_chain_cycle_element_unbound() {
    let mut shell = Shell::new();
    // plain (not a nameref) → itself
    assert_eq!(shell.resolve_nameref("x"), ResolvedName::Name("x".into()));
    // single hop r -> x
    shell.set_nameref("r", true);
    shell.set("r", "x".into()); // store target name
    assert_eq!(shell.resolve_nameref("r"), ResolvedName::Name("x".into()));
    // chain a -> b -> c
    shell.set_nameref("a", true);
    shell.set("a", "b".into());
    shell.set_nameref("b", true);
    shell.set("b", "c".into());
    assert_eq!(shell.resolve_nameref("a"), ResolvedName::Name("c".into()));
    // element target e -> arr[2]
    shell.set_nameref("e", true);
    shell.set("e", "arr[2]".into());
    assert_eq!(
        shell.resolve_nameref("e"),
        ResolvedName::Element {
            name: "arr".into(),
            subscript: "2".into()
        }
    );
    // unbound u (attribute set, empty value)
    shell.set_nameref("u", true);
    assert_eq!(
        shell.resolve_nameref("u"),
        ResolvedName::Unbound("u".into())
    );
    // cycle p -> q -> p
    shell.set_nameref("p", true);
    shell.set("p", "q".into());
    shell.set_nameref("q", true);
    shell.set("q", "p".into());
    assert_eq!(shell.resolve_nameref("p"), ResolvedName::Cycle);
}

#[test]
fn readline_settings_set_and_list() {
    let mut shell = Shell::new();
    // default seeded vars present
    assert_eq!(
        shell
            .readline_settings
            .vars
            .get("editing-mode")
            .map(String::as_str),
        Some("emacs")
    );
    // set a mapped var
    shell.set_readline_var("editing-mode", "vi");
    assert_eq!(
        shell
            .readline_settings
            .vars
            .get("editing-mode")
            .map(String::as_str),
        Some("vi")
    );
    assert!(shell.readline_settings.dirty);
    // -v listing form
    let lines = shell.readline_var_lines();
    assert!(lines.iter().any(|l| l == "set editing-mode vi"));
    assert!(lines.iter().any(|l| l == "set bell-style audible"));
    // record a binding + list it
    shell.add_bind("\"\\C-x\"", "kill-line");
    assert_eq!(
        shell.readline_settings.pending_binds,
        vec![("\"\\C-x\"".to_string(), "kill-line".to_string())]
    );
}

#[test]
fn error_prefix_noninteractive_script_with_line_and_cmd() {
    let mut sh = Shell::new();
    sh.is_interactive = false;
    sh.shell_argv0 = "./arith.tests".to_string();
    sh.current_lineno = 168;
    assert_eq!(
        sh.error_prefix(Diag::Runtime(None)),
        "./arith.tests: line 168: "
    );
    assert_eq!(
        sh.error_prefix(Diag::Runtime(Some("let"))),
        "./arith.tests: line 168: let: "
    );
    assert_eq!(
        sh.error_prefix(Diag::Runtime(Some("(("))),
        "./arith.tests: line 168: ((: "
    );
}

#[test]
fn error_prefix_interactive_keeps_huck_no_line() {
    let mut sh = Shell::new();
    sh.is_interactive = true;
    sh.shell_argv0 = "huck".to_string();
    sh.current_lineno = 5;
    assert_eq!(sh.error_prefix(Diag::Runtime(None)), "huck: ");
    assert_eq!(sh.error_prefix(Diag::Runtime(Some("(("))), "huck: ((: ");
}

#[test]
fn error_prefix_prefers_bash_source_zero() {
    let mut sh = Shell::new();
    sh.is_interactive = false;
    sh.shell_argv0 = "huck".to_string();
    sh.current_lineno = 3;
    sh.seed_array_for_tests("BASH_SOURCE", &[(0, "./sourced.sh")]);
    assert_eq!(
        sh.error_prefix(Diag::Runtime(None)),
        "./sourced.sh: line 3: "
    );
}

#[test]
fn error_prefix_runtime_matrix() {
    let mut sh = Shell::new();
    sh.is_interactive = false;
    sh.shell_argv0 = "s.sh".into();
    sh.current_lineno = 5;
    assert_eq!(sh.error_prefix(Diag::Runtime(None)), "s.sh: line 5: ");
    assert_eq!(
        sh.error_prefix(Diag::Runtime(Some("cd"))),
        "s.sh: line 5: cd: "
    );
    sh.is_interactive = true;
    assert_eq!(sh.error_prefix(Diag::Runtime(None)), "huck: ");
}

#[test]
fn error_prefix_syntax_matrix() {
    let mut sh = Shell::new();
    sh.is_interactive = false;
    sh.shell_argv0 = "s.sh".into();
    // script mode: no -c:
    sh.is_command_string = false;
    assert_eq!(sh.error_prefix(Diag::Syntax { line: 2 }), "s.sh: line 2: ");
    // -c mode: -c: present
    sh.is_command_string = true;
    sh.shell_argv0 = "bash5".into();
    assert_eq!(
        sh.error_prefix(Diag::Syntax { line: 1 }),
        "bash5: -c: line 1: "
    );
}

#[test]
fn error_prefix_syntax_interactive_has_no_c_segment_or_line() {
    // Interactive syntax errors get plain `huck: ` — no line, no `-c:`,
    // even if `is_command_string` were somehow left set.
    let mut sh = Shell::new();
    sh.is_interactive = true;
    sh.is_command_string = true;
    assert_eq!(sh.error_prefix(Diag::Syntax { line: 9 }), "huck: ");
}

#[test]
fn error_prefix_syntax_no_c_segment_when_sourced_under_dash_c() {
    // `-c:` must not leak into a file sourced under `-c` — bash:
    // `bash -c 'source /tmp/bad.sh'` → `/tmp/bad.sh: line 2: ...` (no `-c:`).
    // `is_command_string` stays true for the whole `-c` invocation, so the
    // gate must additionally require top-level source depth (0).
    let mut sh = Shell::new();
    sh.is_interactive = false;
    sh.is_command_string = true;
    sh.shell_argv0 = "badfile".into();
    sh.source_depth = 1;
    assert_eq!(
        sh.error_prefix(Diag::Syntax { line: 2 }),
        "badfile: line 2: "
    );
    sh.source_depth = 0;
    assert_eq!(
        sh.error_prefix(Diag::Syntax { line: 2 }),
        "badfile: -c: line 2: "
    );
}
