// G3: the `==`/`!=` RHS inside `[[ … ]]` matches as an extended (extglob)
// pattern REGARDLESS of `shopt extglob`. These drive the whole
// parse→expand→match pipeline via `process_line` with extglob explicitly OFF.
use crate::shell_state::Shell;

fn status_of(line: &str) -> i32 {
    let mut s = Shell::new();
    s.shopt_options.set("extglob", false);
    crate::shell::process_line(line, &mut s, false);
    s.last_status()
}

#[test]
fn eq_extglob_matches_with_extglob_off() {
    assert_eq!(status_of("[[ record == @(record|top) ]]"), 0); // match
    assert_eq!(status_of("[[ nope == @(record|top) ]]"), 1); // no match
    assert_eq!(status_of("[[ aab == +(a|b) ]]"), 0);
    assert_eq!(status_of("[[ ac == a*(b)c ]]"), 0); // glued, empty *
    assert_eq!(status_of("[[ ab == a?(b) ]]"), 0);
}

#[test]
fn neg_group_and_ne_operator_with_extglob_off() {
    assert_eq!(status_of("[[ foo == !(bar) ]]"), 0); // foo is not bar → matches
    assert_eq!(status_of("[[ bar == !(bar) ]]"), 1);
    assert_eq!(status_of("[[ x != @(a|b) ]]"), 0); // x not in {a,b} → != true
    assert_eq!(status_of("[[ a != @(a|b) ]]"), 1);
}

#[test]
fn rhs_is_pattern_not_literal_with_extglob_off() {
    // The literal text "@(record|top)" does NOT match the pattern @(record|top).
    assert_eq!(status_of("[[ '@(record|top)' == @(record|top) ]]"), 1);
}

#[test]
fn quoted_paren_is_literal_not_extglob() {
    assert_eq!(status_of("x=y; [[ $x == \"(\" ]]"), 1); // y != "("
    assert_eq!(status_of("[[ '(' == \"(\" ]]"), 0);
}
