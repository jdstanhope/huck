use super::*;

#[test]
fn scalar_view_returns_empty_for_associative() {
    let v = VarValue::Associative(vec![
        ("k1".to_string(), "v1".to_string()),
        ("k2".to_string(), "v2".to_string()),
    ]);
    assert_eq!(v.scalar_view(), "");
}

#[test]
fn declare_associative_on_unset_creates_empty() {
    let mut shell = Shell::new();
    assert!(shell.declare_associative("m").is_ok());
    assert_eq!(shell.get_associative("m").map(Vec::len), Some(0));
}

#[test]
fn declare_associative_on_existing_associative_is_noop() {
    let mut shell = Shell::new();
    shell.declare_associative("m").unwrap();
    shell
        .set_associative_element("m", "k".into(), "v".into())
        .unwrap();
    assert!(shell.declare_associative("m").is_ok());
    assert_eq!(shell.lookup_associative_element("m", "k"), Some("v".into()));
}

#[test]
fn declare_associative_on_indexed_errors() {
    let mut shell = Shell::new();
    let mut m = BTreeMap::new();
    m.insert(0, "x".into());
    shell.vars.insert(
        "a".into(),
        Variable {
            value: VarValue::Indexed(m),
            exported: false,
            readonly: false,
            integer: false,
            case_fold: None,
            nameref: false,
        },
    );
    assert!(matches!(
        shell.declare_associative("a"),
        Err(DeclareErr::IndexedExists)
    ));
}

#[test]
fn declare_associative_on_scalar_errors() {
    let mut shell = Shell::new();
    shell.set("s", "hello".into());
    assert!(matches!(
        shell.declare_associative("s"),
        Err(DeclareErr::ScalarExists)
    ));
}

#[test]
fn declare_err_message_uses_command_name() {
    use super::declare_err_message;
    assert_eq!(
        declare_err_message("declare", "a", &DeclareErr::IndexedExists),
        "declare: a: cannot convert indexed to associative array",
    );
    assert_eq!(
        declare_err_message("local", "s", &DeclareErr::ScalarExists),
        "local: s: cannot convert scalar to associative array",
    );
    assert_eq!(
        declare_err_message("readonly", "s", &DeclareErr::ScalarExists),
        "readonly: s: cannot convert scalar to associative array",
    );
}

#[test]
fn set_associative_element_preserves_insertion_order_on_update() {
    let mut shell = Shell::new();
    shell.declare_associative("m").unwrap();
    shell
        .set_associative_element("m", "a".into(), "1".into())
        .unwrap();
    shell
        .set_associative_element("m", "b".into(), "2".into())
        .unwrap();
    shell
        .set_associative_element("m", "c".into(), "3".into())
        .unwrap();
    shell
        .set_associative_element("m", "a".into(), "999".into())
        .unwrap();
    let pairs = shell.get_associative("m").unwrap();
    assert_eq!(pairs[0], ("a".into(), "999".into()));
    assert_eq!(pairs[1], ("b".into(), "2".into()));
    assert_eq!(pairs[2], ("c".into(), "3".into()));
}

#[test]
fn append_associative_element_concatenates() {
    let mut shell = Shell::new();
    shell.declare_associative("m").unwrap();
    shell
        .set_associative_element("m", "k".into(), "hello".into())
        .unwrap();
    shell
        .append_associative_element("m", "k", "_world")
        .unwrap();
    assert_eq!(
        shell.lookup_associative_element("m", "k"),
        Some("hello_world".into())
    );
}

#[test]
fn append_associative_element_creates_when_missing() {
    let mut shell = Shell::new();
    shell.declare_associative("m").unwrap();
    shell
        .append_associative_element("m", "new", "value")
        .unwrap();
    assert_eq!(
        shell.lookup_associative_element("m", "new"),
        Some("value".into())
    );
}

#[test]
fn unset_associative_element_removes_one_key() {
    let mut shell = Shell::new();
    shell.declare_associative("m").unwrap();
    shell
        .set_associative_element("m", "a".into(), "1".into())
        .unwrap();
    shell
        .set_associative_element("m", "b".into(), "2".into())
        .unwrap();
    shell
        .set_associative_element("m", "c".into(), "3".into())
        .unwrap();
    shell.unset_associative_element("m", "b").unwrap();
    let pairs = shell.get_associative("m").unwrap();
    assert_eq!(pairs.len(), 2);
    assert_eq!(pairs[0].0, "a");
    assert_eq!(pairs[1].0, "c");
}

#[test]
fn unset_associative_element_on_missing_key_is_noop() {
    let mut shell = Shell::new();
    shell.declare_associative("m").unwrap();
    shell
        .set_associative_element("m", "a".into(), "1".into())
        .unwrap();
    assert!(shell.unset_associative_element("m", "nope").is_ok());
    assert_eq!(shell.lookup_associative_element("m", "a"), Some("1".into()));
}

#[test]
fn unset_associative_element_on_non_associative_is_noop() {
    let mut shell = Shell::new();
    shell.set("s", "hello".into());
    // Non-associative variable — should NOT modify it and NOT error.
    assert!(shell.unset_associative_element("s", "anything").is_ok());
    assert_eq!(shell.get("s"), Some("hello"));
}

#[test]
fn replace_associative_overwrites() {
    let mut shell = Shell::new();
    shell.declare_associative("m").unwrap();
    shell
        .set_associative_element("m", "old".into(), "1".into())
        .unwrap();
    let new_pairs = vec![
        ("x".to_string(), "10".to_string()),
        ("y".to_string(), "20".to_string()),
    ];
    shell.replace_associative("m", new_pairs).unwrap();
    assert!(shell.lookup_associative_element("m", "old").is_none());
    assert_eq!(
        shell.lookup_associative_element("m", "x"),
        Some("10".into())
    );
    assert_eq!(
        shell.lookup_associative_element("m", "y"),
        Some("20".into())
    );
}

#[test]
fn readonly_blocks_set_associative_element() {
    let mut shell = Shell::new();
    shell.declare_associative("m").unwrap();
    shell
        .set_associative_element("m", "k".into(), "v".into())
        .unwrap();
    shell.mark_readonly("m");
    assert!(matches!(
        shell.set_associative_element("m", "k2".into(), "v2".into()),
        Err(AssignErr::Readonly)
    ));
    assert!(shell.lookup_associative_element("m", "k2").is_none());
}
