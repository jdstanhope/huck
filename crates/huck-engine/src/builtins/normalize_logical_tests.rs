use super::normalize_logical;

#[test]
fn normalize_logical_collapses_lexically() {
    assert_eq!(normalize_logical("/a/b/../c"), "/a/c");
    assert_eq!(normalize_logical("/a/./b"), "/a/b");
    assert_eq!(normalize_logical("/a//b"), "/a/b");
    assert_eq!(normalize_logical("/a/b/.."), "/a");
    assert_eq!(normalize_logical("/.."), "/");
    assert_eq!(normalize_logical("/a/../.."), "/");
    assert_eq!(normalize_logical("/"), "/");
    assert_eq!(normalize_logical("/tmp/m/link/.."), "/tmp/m");
}
