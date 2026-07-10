use super::{format_select_menu, number_len, select_indent};

fn items(words: &[&str]) -> Vec<String> {
    words.iter().map(|s| s.to_string()).collect()
}

#[test]
fn number_len_digit_counts() {
    assert_eq!(number_len(1), 1);
    assert_eq!(number_len(9), 1);
    assert_eq!(number_len(10), 2);
    assert_eq!(number_len(99), 2);
    assert_eq!(number_len(100), 3);
}

#[test]
fn indent_emits_tab_across_stop_else_space() {
    let mut s = String::new();
    select_indent(&mut s, 6, 11); // crosses the 8-boundary once → tab + 3 spaces
    assert_eq!(s, "\t   ");
    let mut s2 = String::new();
    select_indent(&mut s2, 20, 22); // same tab block → 2 spaces
    assert_eq!(s2, "  ");
    let mut s3 = String::new();
    select_indent(&mut s3, 8, 11); // from is exactly on a tab stop → no tab emitted, 3 spaces
    assert_eq!(s3, "   ");
}

#[test]
fn single_item() {
    assert_eq!(format_select_menu(&items(&["only"]), 80), "1) only\n");
}

#[test]
fn three_items_single_column() {
    // 3 items: max_elem_len=6, cols=80/6=13, rows=ceil(3/13)=1 → flip → 3 rows × 1 col.
    assert_eq!(
        format_select_menu(&items(&["a", "b", "c"]), 80),
        "1) a\n2) b\n3) c\n"
    );
}

#[test]
fn ten_items_cols80_multicolumn() {
    let got = format_select_menu(
        &items(&[
            "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
        ]),
        80,
    );
    // Verified byte-for-byte against bash 5.2 (COLUMNS=80, cat -A):
    let expected = "1) one\t    3) three   5) five\t  7) seven   9) nine\n\
                        2) two\t    4) four    6) six\t  8) eight  10) ten\n";
    assert_eq!(got, expected);
}

#[test]
fn ten_items_cols40() {
    let got = format_select_menu(
        &items(&[
            "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
        ]),
        40,
    );
    let expected = "1) one\t    5) five    9) nine\n\
                        2) two\t    6) six    10) ten\n\
                        3) three    7) seven\n\
                        4) four\t    8) eight\n";
    assert_eq!(got, expected);
}

#[test]
fn ten_items_cols110_single_column_flip() {
    let got = format_select_menu(
        &items(&[
            "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
        ]),
        110,
    );
    // Wide COLS → rows==1 flip → single column, numbers right-justified to 2.
    // (Verified byte-for-byte against bash 5.2 COLUMNS=110 via cat -A.)
    let expected = concat!(
        " 1) one\n",
        " 2) two\n",
        " 3) three\n",
        " 4) four\n",
        " 5) five\n",
        " 6) six\n",
        " 7) seven\n",
        " 8) eight\n",
        " 9) nine\n",
        "10) ten\n",
    );
    assert_eq!(got, expected);
}
