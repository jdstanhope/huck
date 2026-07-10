use super::*;

#[test]
fn scalar_view_returns_string_for_scalar() {
    let v = VarValue::Scalar("hello".to_string());
    assert_eq!(v.scalar_view(), "hello");
}

#[test]
fn scalar_view_returns_element_zero_for_indexed() {
    let mut m = BTreeMap::new();
    m.insert(0, "first".to_string());
    m.insert(1, "second".to_string());
    let v = VarValue::Indexed(m);
    assert_eq!(v.scalar_view(), "first");
}

#[test]
fn scalar_view_empty_for_indexed_without_zero() {
    let mut m = BTreeMap::new();
    m.insert(5, "x".to_string());
    let v = VarValue::Indexed(m);
    assert_eq!(v.scalar_view(), "");
}

#[test]
fn scalar_view_empty_for_empty_indexed() {
    let v = VarValue::Indexed(BTreeMap::new());
    assert_eq!(v.scalar_view(), "");
}

#[test]
fn variable_scalar_constructor_sets_defaults() {
    let v = Variable::scalar("x".to_string());
    assert!(!v.exported);
    assert!(!v.readonly);
    assert!(!v.integer);
    assert_eq!(v.value.scalar_view(), "x");
}

#[test]
fn try_set_on_indexed_overwrites_element_zero_only() {
    let mut shell = Shell::new();
    let mut m = BTreeMap::new();
    m.insert(0, "old".to_string());
    m.insert(1, "x".to_string());
    shell.vars.insert(
        "a".to_string(),
        Variable {
            value: VarValue::Indexed(m),
            exported: false,
            readonly: false,
            integer: false,
            case_fold: None,
            nameref: false,
        },
    );
    let result = shell.try_set("a", "new".to_string());
    assert!(result.is_ok());
    match &shell.vars.get("a").unwrap().value {
        VarValue::Indexed(m) => {
            assert_eq!(m.get(&0).map(String::as_str), Some("new"));
            assert_eq!(m.get(&1).map(String::as_str), Some("x"));
        }
        _ => panic!("expected Indexed"),
    }
}

#[test]
fn set_on_indexed_overwrites_element_zero_only() {
    let mut shell = Shell::new();
    let mut m = BTreeMap::new();
    m.insert(0, "old".to_string());
    m.insert(1, "x".to_string());
    shell.vars.insert(
        "a".to_string(),
        Variable {
            value: VarValue::Indexed(m),
            exported: false,
            readonly: false,
            integer: false,
            case_fold: None,
            nameref: false,
        },
    );
    shell.set("a", "new".to_string());
    match &shell.vars.get("a").unwrap().value {
        VarValue::Indexed(m) => {
            assert_eq!(m.get(&0).map(String::as_str), Some("new"));
            assert_eq!(m.get(&1).map(String::as_str), Some("x"));
        }
        _ => panic!("expected Indexed"),
    }
}

#[test]
fn export_set_on_indexed_overwrites_element_zero_only_and_marks_exported() {
    let mut shell = Shell::new();
    let mut m = BTreeMap::new();
    m.insert(0, "old".to_string());
    m.insert(1, "x".to_string());
    shell.vars.insert(
        "a".to_string(),
        Variable {
            value: VarValue::Indexed(m),
            exported: false,
            readonly: false,
            integer: false,
            case_fold: None,
            nameref: false,
        },
    );
    shell.export_set("a", "new".to_string());
    let v = shell.vars.get("a").unwrap();
    assert!(v.exported);
    match &v.value {
        VarValue::Indexed(m) => {
            assert_eq!(m.get(&0).map(String::as_str), Some("new"));
            assert_eq!(m.get(&1).map(String::as_str), Some("x"));
        }
        _ => panic!("expected Indexed"),
    }
}
