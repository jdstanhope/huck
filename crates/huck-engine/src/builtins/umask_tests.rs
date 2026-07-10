#[test]
fn umask_octal_rejects_nonoctal() {
    assert!(super::parse_octal_umask("09").is_err());
    assert_eq!(super::parse_octal_umask("022").unwrap(), 0o22);
}
#[test]
fn umask_symbolic_roundtrip() {
    assert_eq!(super::format_symbolic_umask(0o022), "u=rwx,g=rx,o=rx");
    // set o to deny write from a clear mask
    assert_eq!(
        super::parse_symbolic_umask("u=rwx,g=rwx,o=rx", 0).unwrap(),
        0o002
    );
}
#[test]
fn umask_symbolic_errors() {
    assert!(matches!(
        super::parse_symbolic_umask("g=u", 0),
        Err(super::SymErr::Char('u'))
    ));
    assert!(matches!(
        super::parse_symbolic_umask("u:rwx", 0),
        Err(super::SymErr::Operator(':'))
    ));
    assert!(matches!(
        super::parse_symbolic_umask("u=rwx:g=rwx", 0),
        Err(super::SymErr::Char(':'))
    ));
}
