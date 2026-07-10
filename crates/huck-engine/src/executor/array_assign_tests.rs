use super::*;
use crate::shell_state::Shell;

/// Drive a fragment through the same execute path the REPL uses.
fn run_line(shell: &mut Shell, line: &str) {
    let mut src = String::from(line);
    if !src.ends_with('\n') {
        src.push('\n');
    }
    let seq = crate::parser::parse_sequence(&mut crate::lexer::Lexer::new(
        &src,
        &Default::default(),
        crate::lexer::LexerOptions::default(),
    ))
    .expect("parse ok")
    .expect("non-empty parse");
    execute(&seq, shell, &src);
}

#[test]
fn compound_assign_creates_array() {
    let mut s = Shell::new();
    run_line(&mut s, "a=(x y z)");
    let m = s.get_indexed("a").expect("a should be an array");
    assert_eq!(m.get(&0).map(String::as_str), Some("x"));
    assert_eq!(m.get(&1).map(String::as_str), Some("y"));
    assert_eq!(m.get(&2).map(String::as_str), Some("z"));
}

#[test]
fn sparse_compound_assign_respects_explicit_subscripts() {
    let mut s = Shell::new();
    run_line(&mut s, "a=([5]=x [2]=y)");
    let m = s.get_indexed("a").expect("a should be an array");
    assert_eq!(m.len(), 2);
    assert_eq!(m.get(&5).map(String::as_str), Some("x"));
    assert_eq!(m.get(&2).map(String::as_str), Some("y"));
}

#[test]
fn element_assign_creates_array() {
    let mut s = Shell::new();
    run_line(&mut s, "a[3]=hello");
    let m = s.get_indexed("a").expect("a should be an array");
    assert_eq!(m.get(&3).map(String::as_str), Some("hello"));
}

#[test]
fn element_assign_promotes_scalar() {
    let mut s = Shell::new();
    run_line(&mut s, "a=old");
    run_line(&mut s, "a[2]=new");
    let m = s.get_indexed("a").expect("scalar should promote to array");
    assert_eq!(m.get(&0).map(String::as_str), Some("old"));
    assert_eq!(m.get(&2).map(String::as_str), Some("new"));
}

#[test]
fn append_array_extends() {
    let mut s = Shell::new();
    run_line(&mut s, "a=(x y)");
    run_line(&mut s, "a+=(z w)");
    let m = s.get_indexed("a").unwrap();
    assert_eq!(
        m.values().cloned().collect::<Vec<_>>(),
        vec![
            "x".to_string(),
            "y".to_string(),
            "z".to_string(),
            "w".to_string()
        ]
    );
}

#[test]
fn append_element_concatenates() {
    let mut s = Shell::new();
    run_line(&mut s, "a[0]=hello");
    run_line(&mut s, "a[0]+=_world");
    let m = s.get_indexed("a").unwrap();
    assert_eq!(m.get(&0).map(String::as_str), Some("hello_world"));
}

#[test]
fn readonly_blocks_compound_assign() {
    let mut s = Shell::new();
    run_line(&mut s, "a=(initial)");
    s.mark_readonly("a");
    run_line(&mut s, "a=(changed)");
    let m = s.get_indexed("a").unwrap();
    assert_eq!(m.get(&0).map(String::as_str), Some("initial"));
}

#[test]
fn readonly_blocks_element_assign() {
    let mut s = Shell::new();
    run_line(&mut s, "a=(initial)");
    s.mark_readonly("a");
    run_line(&mut s, "a[5]=new");
    let m = s.get_indexed("a").unwrap();
    assert!(m.get(&5).is_none());
}

#[test]
fn unset_element_removes_one_key() {
    let mut s = Shell::new();
    run_line(&mut s, "a=(x y z)");
    run_line(&mut s, "unset a[1]");
    let m = s.get_indexed("a").unwrap();
    assert!(m.get(&1).is_none());
    assert_eq!(m.get(&0).map(String::as_str), Some("x"));
    assert_eq!(m.get(&2).map(String::as_str), Some("z"));
}

#[test]
fn unset_whole_array_removes_variable() {
    let mut s = Shell::new();
    run_line(&mut s, "a=(x y z)");
    run_line(&mut s, "unset a");
    assert!(s.get_indexed("a").is_none());
    assert!(s.get("a").is_none());
}

#[test]
fn scalar_append_to_existing_array_writes_element_zero() {
    // `a=(x y); a+=z` in bash appends to element 0 (i.e. concatenates
    // with a[0]), yielding a[0]="xz".
    let mut s = Shell::new();
    run_line(&mut s, "a=(x y)");
    run_line(&mut s, "a+=z");
    let m = s.get_indexed("a").expect("still an array");
    assert_eq!(m.get(&0).map(String::as_str), Some("xz"));
    assert_eq!(m.get(&1).map(String::as_str), Some("y"));
}

#[test]
fn indexed_lvalue_compound_rhs_rejected() {
    // `a[i]=(...)` is a syntax-level error in bash; huck rejects
    // it with a diagnostic and leaves `a` empty.
    let mut s = Shell::new();
    run_line(&mut s, "a[0]=(x y)");
    assert!(s.get_indexed("a").is_none());
}

#[test]
fn unset_with_empty_subscript_errors() {
    // bash treats `unset a[]` as a syntax error
    // ("bad array subscript") and leaves `a` untouched.
    let mut s = Shell::new();
    run_line(&mut s, "a=(x y z)");
    run_line(&mut s, "unset a[]");
    let m = s.get_indexed("a").expect("a should still exist");
    assert_eq!(m.len(), 3);
}

// v268 T2: the subscript-assignment lvalue (`a[i]=v`) is assembled by
// the atom parser's `parse_word` (same as other contexts). These exercise
// the full production path (parse_sequence + execute, same as the CLI) for
// every subscript shape: command-sub, `$var`, arithmetic, a quoted
// associative key, and append-assign on an already-set element.
#[test]
fn subscript_bridge_atom_path_regressions() {
    // `a[$(echo 2)]=hi` — command substitution inside the subscript.
    let mut s = Shell::new();
    run_line(&mut s, "a=(); a[$(echo 2)]=hi");
    let m = s.get_indexed("a").expect("a should be an array");
    assert_eq!(m.get(&2).map(String::as_str), Some("hi"));

    // `a[$i]=x` — a plain variable subscript.
    let mut s = Shell::new();
    run_line(&mut s, "i=3; a[$i]=x");
    let m = s.get_indexed("a").expect("a should be an array");
    assert_eq!(m.get(&3).map(String::as_str), Some("x"));

    // `a[1+1]=y` — arithmetic subscript (evaluates to numeric key).
    let mut s = Shell::new();
    run_line(&mut s, "a[1+1]=y");
    let m = s.get_indexed("a").expect("a should be an array");
    assert_eq!(m.get(&2).map(String::as_str), Some("y"));

    // `declare -A m; m["a b"]=z` — a quoted, space-containing associative
    // key, driven through the real `declare` builtin.
    let mut s = Shell::new();
    run_line(&mut s, "declare -A m; m[\"a b\"]=z");
    assert_eq!(s.lookup_associative_element("m", "a b"), Some("z".into()));

    // `a[2]=p; a[2]+=q` — append-assign re-resolves the same subscript.
    let mut s = Shell::new();
    run_line(&mut s, "a[2]=p; a[2]+=q");
    let m = s.get_indexed("a").expect("a should be an array");
    assert_eq!(m.get(&2).map(String::as_str), Some("pq"));
}

#[test]
fn array_literal_append_element_unset_takes_value() {
    // `x=(1 2 [2]+=7 4)`: element 2 is unset in the fresh literal, so
    // `[2]+=7` yields "7" (base empty). Matches bash `1 2 7 4`.
    let mut s = Shell::new();
    run_line(&mut s, "x=(1 2 [2]+=7 4)");
    let m = s.get_indexed("x").expect("x is an array");
    assert_eq!(m.get(&0).map(String::as_str), Some("1"));
    assert_eq!(m.get(&1).map(String::as_str), Some("2"));
    assert_eq!(m.get(&2).map(String::as_str), Some("7"));
    assert_eq!(m.get(&3).map(String::as_str), Some("4"));
}

#[test]
fn array_literal_append_element_concats_earlier_in_literal() {
    // `[0]=a [0]+=b [0]+=c` → "abc" (append against the map-so-far).
    let mut s = Shell::new();
    run_line(&mut s, "x=([0]=a [0]+=b [0]+=c)");
    assert_eq!(s.lookup_indexed_element("x", 0), Some("abc".into()));
}

#[test]
fn array_literal_append_replace_ignores_old_array() {
    // A plain `x=(…)` replace discards the old array: `[0]+=B` on the
    // discarded x[0]=9 → "B" (base empty), matching bash.
    let mut s = Shell::new();
    run_line(&mut s, "x=(9 9 9); x=([0]+=B)");
    assert_eq!(s.lookup_indexed_element("x", 0), Some("B".into()));
}

#[test]
fn array_literal_append_context_consults_existing_element() {
    // `x+=(…)` append keeps the old array: `[1]+=Z` on existing x[1]=b → "bZ".
    let mut s = Shell::new();
    run_line(&mut s, "x=(a b c); x+=([1]+=Z)");
    let m = s.get_indexed("x").expect("x is an array");
    assert_eq!(m.get(&1).map(String::as_str), Some("bZ"));
    assert_eq!(m.get(&0).map(String::as_str), Some("a"));
    assert_eq!(m.get(&2).map(String::as_str), Some("c"));
}

#[test]
fn array_literal_append_element_integer_arithmetic() {
    // Integer-flagged array: `[0]+=3` on n[0]=5 is arithmetic → 8.
    let mut s = Shell::new();
    run_line(&mut s, "declare -ia n=(5 [0]+=3)");
    assert_eq!(s.lookup_indexed_element("n", 0), Some("8".into()));
}
