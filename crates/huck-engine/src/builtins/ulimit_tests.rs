#[test]
fn ulimit_table_lookup_and_scale() {
    let c = super::ulimit_lookup('c').unwrap();
    assert_eq!(c.mult, 1024);
    let n = super::ulimit_lookup('n').unwrap();
    assert_eq!(n.mult, 1);
    assert!(super::ulimit_lookup('Z').is_none());
}
