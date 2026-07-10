use crate::shell_state::Shell;

fn run(shell: &mut Shell, line: &str) {
    crate::shell::process_line(line, shell, false);
}

#[test]
fn element_assign_on_declared_associative_uses_string_key() {
    let mut s = Shell::new();
    s.declare_associative("m").unwrap();
    run(&mut s, "m[foo]=bar");
    assert_eq!(s.lookup_associative_element("m", "foo"), Some("bar".into()));
}

#[test]
fn element_assign_without_declare_creates_indexed() {
    // Bash gotcha: `m[foo]=v` on unset `m` creates indexed (foo→0).
    let mut s = Shell::new();
    run(&mut s, "m[foo]=bar");
    assert!(s.get_indexed("m").is_some());
    assert!(s.get_associative("m").is_none());
    assert_eq!(s.lookup_indexed_element("m", 0), Some("bar".into()));
}

#[test]
fn compound_append_literal_on_associative_appends_to_existing() {
    // `a+=([one]+=more)` on existing [one]=one → "onemore".
    let mut s = Shell::new();
    run(&mut s, "declare -A a=([one]=one); a+=([one]+=more)");
    assert_eq!(
        s.lookup_associative_element("a", "one"),
        Some("onemore".into())
    );
}

#[test]
fn compound_replace_literal_on_associative_append_is_plain_set() {
    // bash quirk: in a fresh `declare -A a=(…)` replace, `[k]+=y` does NOT
    // concat with an earlier `[k]=x` in the same literal — result is "y".
    let mut s = Shell::new();
    run(&mut s, "declare -A a=([k]=x [k]+=y)");
    assert_eq!(s.lookup_associative_element("a", "k"), Some("y".into()));
}

#[test]
fn compound_literal_on_associative_uses_keys() {
    let mut s = Shell::new();
    s.declare_associative("m").unwrap();
    run(&mut s, "m=([a]=1 [b]=2)");
    assert_eq!(s.lookup_associative_element("m", "a"), Some("1".into()));
    assert_eq!(s.lookup_associative_element("m", "b"), Some("2".into()));
}

#[test]
fn append_compound_on_associative_merges() {
    let mut s = Shell::new();
    s.declare_associative("m").unwrap();
    run(&mut s, "m=([a]=1 [b]=2)");
    run(&mut s, "m+=([c]=3 [a]=99)");
    let pairs = s.get_associative("m").unwrap();
    assert_eq!(pairs.len(), 3);
    assert_eq!(s.lookup_associative_element("m", "a"), Some("99".into()));
    assert_eq!(s.lookup_associative_element("m", "c"), Some("3".into()));
}

#[test]
fn append_element_on_associative_concatenates() {
    let mut s = Shell::new();
    s.declare_associative("m").unwrap();
    run(&mut s, "m[k]=hello");
    run(&mut s, "m[k]+=_world");
    assert_eq!(
        s.lookup_associative_element("m", "k"),
        Some("hello_world".into())
    );
}

#[test]
fn positional_literal_on_associative_rejects() {
    let mut s = Shell::new();
    s.declare_associative("m").unwrap();
    s.set_associative_element("m", "preexisting".into(), "x".into())
        .unwrap();
    run(&mut s, "m=(a b c)");
    // associative `m` should be unchanged; positional literal is rejected.
    assert_eq!(
        s.lookup_associative_element("m", "preexisting"),
        Some("x".into())
    );
}

#[test]
fn scalar_rhs_on_associative_rejects() {
    let mut s = Shell::new();
    s.declare_associative("m").unwrap();
    s.set_associative_element("m", "k".into(), "v".into())
        .unwrap();
    run(&mut s, "m=newscalar");
    // associative `m` should be unchanged.
    assert_eq!(s.lookup_associative_element("m", "k"), Some("v".into()));
}

#[test]
fn unset_associative_element_removes_one_key() {
    let mut s = Shell::new();
    s.declare_associative("m").unwrap();
    run(&mut s, "m[a]=1");
    run(&mut s, "m[b]=2");
    run(&mut s, "m[c]=3");
    run(&mut s, "unset m[b]");
    let pairs = s.get_associative("m").unwrap();
    assert_eq!(pairs.len(), 2);
    assert!(s.lookup_associative_element("m", "b").is_none());
    assert_eq!(s.lookup_associative_element("m", "a"), Some("1".into()));
    assert_eq!(s.lookup_associative_element("m", "c"), Some("3".into()));
}

#[test]
fn unset_whole_associative_removes_variable() {
    let mut s = Shell::new();
    s.declare_associative("m").unwrap();
    run(&mut s, "m[a]=1");
    run(&mut s, "unset m");
    assert!(s.get_associative("m").is_none());
    assert!(s.get("m").is_none());
}

#[test]
fn readonly_blocks_element_write_on_associative() {
    let mut s = Shell::new();
    s.declare_associative("m").unwrap();
    s.set_associative_element("m", "a".into(), "1".into())
        .unwrap();
    s.mark_readonly("m");
    run(&mut s, "m[b]=2");
    assert!(s.lookup_associative_element("m", "b").is_none());
}

#[test]
fn unset_name_with_separate_assoc_still_creates_indexed() {
    // The gotcha is name-specific: having declared `foo` as associative
    // should not influence routing for an UNSET `bar`. `bar[baz]=v`
    // should still create indexed `bar[0]=v`.
    let mut s = Shell::new();
    s.declare_associative("foo").unwrap();
    s.set_associative_element("foo", "k".into(), "v".into())
        .unwrap();
    run(&mut s, "bar[baz]=value");
    assert!(s.get_indexed("bar").is_some(), "bar should be indexed");
    assert!(
        s.get_associative("bar").is_none(),
        "bar should NOT be associative"
    );
    // foo should be unaffected.
    assert_eq!(s.lookup_associative_element("foo", "k"), Some("v".into()));
}
