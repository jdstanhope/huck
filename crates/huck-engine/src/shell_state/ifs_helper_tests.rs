use super::*;

#[test]
fn ifs_default_when_unset() {
    let s = Shell::new();
    assert_eq!(s.ifs(), " \t\n");
}

#[test]
fn ifs_returns_set_value() {
    let mut s = Shell::new();
    s.set("IFS", ":".to_string());
    assert_eq!(s.ifs(), ":");
}

#[test]
fn ifs_returns_empty_when_set_to_empty() {
    let mut s = Shell::new();
    s.set("IFS", "".to_string());
    assert_eq!(s.ifs(), "");
}
