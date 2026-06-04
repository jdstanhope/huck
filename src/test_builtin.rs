//! The `test` / `[` builtin: conditional expression evaluation.
//!
//! `evaluate` implements the POSIX argument-count algorithm. Operator
//! application lives in `apply_unary` / `apply_binary`.

/// Evaluates a `test` expression. `var_is_set(name)` answers `-v NAME`.
/// `Ok(true)` / `Ok(false)` are the result; `Err(message)` is a usage error.
pub fn evaluate_with(args: &[String], var_is_set: &dyn Fn(&str) -> bool) -> Result<bool, String> {
    // POSIX § 4.62 short-form for 0-1 args: required for correctness
    // (e.g. `[ -a ]` is true — a 1-arg call returns truthiness of the
    // string, not a unary-op application).
    match args.len() {
        0 => return Ok(false),
        1 => return Ok(!args[0].is_empty()),
        _ => {}
    }
    // For 2-4 args, try the POSIX short-form first. It handles every
    // backward-compatible case (existing tests). On Err, fall through
    // to the grammar parser, which handles forms the short-form
    // rejects (e.g. `[ ( -n a ) ]`).
    if args.len() <= 4
        && let Ok(b) = evaluate_short_form(args, var_is_set)
    {
        return Ok(b);
    }
    let mut p = Parser {
        args,
        pos: 0,
        dry_run: false,
        var_is_set,
    };
    let result = p.parse_expr()?;
    if p.pos != args.len() {
        return Err(format!("{}: unexpected argument", args[p.pos]));
    }
    Ok(result)
}

/// Back-compat entry: no variables are considered set (`-v` → false).
/// Used by every caller that doesn't evaluate `-v` (all existing unit
/// tests and `[[`'s file-test delegation).
pub fn evaluate(args: &[String]) -> Result<bool, String> {
    evaluate_with(args, &|_| false)
}

fn evaluate_short_form(
    args: &[String],
    var_is_set: &dyn Fn(&str) -> bool,
) -> Result<bool, String> {
    match args.len() {
        2 => {
            if args[0] == "!" {
                negate(evaluate_with(&args[1..2], var_is_set))
            } else if is_unary_op(&args[0]) {
                apply_unary(&args[0], &args[1], var_is_set)
            } else {
                Err(format!("{}: unary operator expected", args[0]))
            }
        }
        3 => {
            if is_binary_op(&args[1]) {
                apply_binary(&args[1], &args[0], &args[2])
            } else if args[0] == "!" {
                negate(evaluate_with(&args[1..3], var_is_set))
            } else {
                Err(format!("{}: binary operator expected", args[1]))
            }
        }
        4 => {
            if args[0] == "!" {
                negate(evaluate_with(&args[1..4], var_is_set))
            } else {
                Err("too many arguments".to_string())
            }
        }
        _ => Err("too many arguments".to_string()),
    }
}

/// Negates a result; a usage error stays a usage error.
fn negate(r: Result<bool, String>) -> Result<bool, String> {
    r.map(|b| !b)
}

/// True if `s` is a recognized unary operator.
fn is_unary_op(s: &str) -> bool {
    matches!(
        s,
        // `-a` is bash's deprecated unary alias for `-e` (file exists).
        // It also serves as the binary AND combinator in operator
        // position; the grammar parser (v75) disambiguates by context.
        "-a" | "-e" | "-f" | "-d" | "-r" | "-w" | "-x" | "-s" | "-L" | "-z" | "-n" | "-v"
    )
}

/// True if `s` is a recognized binary operator.
fn is_binary_op(s: &str) -> bool {
    matches!(
        s,
        "=" | "==" | "!=" | "-eq" | "-ne" | "-lt" | "-le" | "-gt" | "-ge" | "-nt" | "-ot" | "-ef"
    )
}

/// `-nt`/`-ot`/`-ef` for the `test`/`[` and `[[ ]]` constructs. A missing
/// file is treated as the oldest possible mtime; `-ef` requires both files
/// to exist (bash 5.2 semantics).
pub(crate) fn compare_files(op: &str, lhs: &str, rhs: &str) -> bool {
    use std::os::unix::fs::MetadataExt;
    let lm = std::fs::metadata(lhs);
    let rm = std::fs::metadata(rhs);
    let nanos = |m: &std::fs::Metadata| (m.mtime() as i128) * 1_000_000_000 + (m.mtime_nsec() as i128);
    match op {
        "-nt" => match (&lm, &rm) {
            (Ok(a), Ok(b)) => nanos(a) > nanos(b),
            (Ok(_), Err(_)) => true, // lhs exists, rhs missing
            _ => false,              // lhs missing
        },
        "-ot" => match (&lm, &rm) {
            (Ok(a), Ok(b)) => nanos(a) < nanos(b),
            (Err(_), Ok(_)) => true, // lhs missing, rhs exists
            _ => false,
        },
        "-ef" => match (&lm, &rm) {
            (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
            _ => false,
        },
        _ => false,
    }
}

/// Applies a unary operator to its operand.
fn apply_unary(
    op: &str,
    operand: &str,
    var_is_set: &dyn Fn(&str) -> bool,
) -> Result<bool, String> {
    match op {
        "-v" => Ok(var_is_set(operand)),
        "-z" => Ok(operand.is_empty()),
        "-n" => Ok(!operand.is_empty()),
        // `-a` and `-e` both test for file existence. POSIX prefers
        // `-e`; bash retains `-a` as a deprecated alias.
        "-a" | "-e" => Ok(std::fs::metadata(operand).is_ok()),
        "-f" => Ok(std::fs::metadata(operand)
            .map(|m| m.is_file())
            .unwrap_or(false)),
        "-d" => Ok(std::fs::metadata(operand)
            .map(|m| m.is_dir())
            .unwrap_or(false)),
        "-s" => Ok(std::fs::metadata(operand)
            .map(|m| m.len() > 0)
            .unwrap_or(false)),
        "-L" => Ok(std::fs::symlink_metadata(operand)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)),
        "-r" => Ok(access(operand, libc::R_OK)),
        "-w" => Ok(access(operand, libc::W_OK)),
        "-x" => Ok(access(operand, libc::X_OK)),
        _ => Err(format!("{op}: unknown operator")),
    }
}

/// True if the calling process can access `path` with `mode`
/// (`libc::R_OK` / `W_OK` / `X_OK`), per `access(2)`.
fn access(path: &str, mode: i32) -> bool {
    use std::ffi::CString;
    let Ok(c_path) = CString::new(path) else {
        return false;
    };
    unsafe { libc::access(c_path.as_ptr(), mode) == 0 }
}

/// Applies a binary operator to its two operands.
fn apply_binary(op: &str, lhs: &str, rhs: &str) -> Result<bool, String> {
    match op {
        "=" | "==" => Ok(lhs == rhs),
        "!=" => Ok(lhs != rhs),
        "-eq" | "-ne" | "-lt" | "-le" | "-gt" | "-ge" => {
            let l = parse_int(lhs)?;
            let r = parse_int(rhs)?;
            Ok(match op {
                "-eq" => l == r,
                "-ne" => l != r,
                "-lt" => l < r,
                "-le" => l <= r,
                "-gt" => l > r,
                "-ge" => l >= r,
                _ => unreachable!("checked by the outer match"),
            })
        }
        "-nt" | "-ot" | "-ef" => Ok(compare_files(op, lhs, rhs)),
        _ => Err(format!("{op}: unknown operator")),
    }
}

/// Parses a `test` integer operand (decimal `i64`).
fn parse_int(s: &str) -> Result<i64, String> {
    s.parse::<i64>()
        .map_err(|_| "integer expression expected".to_string())
}

/// Recursive-descent parser for the `test` grammar:
///
/// ```text
/// EXPR    ::= EXPR -o ANDEXPR | ANDEXPR
/// ANDEXPR ::= ANDEXPR -a UNEXPR | UNEXPR
/// UNEXPR  ::= ! UNEXPR | PRIMARY
/// PRIMARY ::= ( EXPR ) | <unary> <word> | <word> <binop> <word> | <word>
/// ```
///
/// Used for 5+ argument calls and for 2-4 argument calls that the
/// POSIX short-form algorithm rejects (e.g. `[ ( -n a ) ]`).
struct Parser<'a> {
    args: &'a [String],
    pos: usize,
    /// When true, `parse_primary` skips operator application (file
    /// syscalls, binary-op evaluation, bare-word truthiness) and
    /// returns a placeholder `false`. Tokens are still consumed so
    /// `pos` advances correctly, and token-structure errors (empty
    /// parens, missing `)`, expression-expected) still fire. Used to
    /// implement bash-style short-circuit evaluation of `-a` / `-o`
    /// without duplicating the grammar walker.
    dry_run: bool,
    /// Predicate answering `-v NAME` (variable-is-set). Injected so the
    /// module stays decoupled from `Shell`.
    var_is_set: &'a dyn Fn(&str) -> bool,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&str> {
        self.args.get(self.pos).map(String::as_str)
    }

    fn take(&mut self) -> Option<&str> {
        let s = self.args.get(self.pos).map(String::as_str);
        if s.is_some() {
            self.pos += 1;
        }
        s
    }

    /// EXPR ::= ANDEXPR ( -o ANDEXPR )*
    ///
    /// Short-circuits: when the accumulated LHS is already `true`,
    /// the RHS is parsed in dry-run mode (tokens consumed, but no
    /// operator side effects). Matches bash `[ … ]` semantics.
    fn parse_expr(&mut self) -> Result<bool, String> {
        let mut result = self.parse_and()?;
        while self.peek() == Some("-o") {
            self.pos += 1; // consume -o
            let saved_dry = self.dry_run;
            // If we're already in dry-run from an outer short-circuit,
            // stay that way. Otherwise short-circuit if LHS is true.
            if result {
                self.dry_run = true;
            }
            let rhs = self.parse_and()?;
            self.dry_run = saved_dry;
            if !result {
                result = rhs;
            }
        }
        Ok(result)
    }

    /// ANDEXPR ::= UNEXPR ( -a UNEXPR )*
    ///
    /// Short-circuits: when the accumulated LHS is already `false`,
    /// the RHS is parsed in dry-run mode. Matches bash `[ … ]`
    /// semantics — e.g. `[ -n "" -a -f /no/path ]` does not syscall.
    fn parse_and(&mut self) -> Result<bool, String> {
        let mut result = self.parse_unary()?;
        while self.peek() == Some("-a") {
            self.pos += 1; // consume -a
            let saved_dry = self.dry_run;
            if !result {
                self.dry_run = true;
            }
            let rhs = self.parse_unary()?;
            self.dry_run = saved_dry;
            if result {
                result = rhs;
            }
        }
        Ok(result)
    }

    /// UNEXPR ::= ! UNEXPR | PRIMARY
    fn parse_unary(&mut self) -> Result<bool, String> {
        if self.peek() == Some("!") {
            self.pos += 1;
            let inner = self.parse_unary()?;
            Ok(!inner)
        } else {
            self.parse_primary()
        }
    }

    /// PRIMARY ::= ( EXPR ) | <unary> <word> | <word> <binop> <word> | <word>
    fn parse_primary(&mut self) -> Result<bool, String> {
        // Empty input where a primary is expected.
        if self.peek().is_none() {
            return Err("expression expected".to_string());
        }
        // Closing paren / combinator where a primary is expected.
        match self.peek() {
            Some(")") | Some("-a") | Some("-o") => {
                return Err("expression expected".to_string());
            }
            _ => {}
        }
        // Parenthesized group.
        if self.peek() == Some("(") {
            self.pos += 1; // consume (
            let inner = self.parse_expr()?;
            match self.peek() {
                Some(")") => {
                    self.pos += 1;
                    return Ok(inner);
                }
                _ => return Err("missing ')'".to_string()),
            }
        }
        // <unary> <word> — recognize a known unary op followed by a word.
        if let (Some(op), Some(_operand)) = (
            self.args.get(self.pos).map(String::as_str),
            self.args.get(self.pos + 1).map(String::as_str),
        ) && is_unary_op(op)
        {
            let op = op.to_string();
            let operand = self.args[self.pos + 1].clone();
            self.pos += 2;
            // Dry-run: tokens consumed, but no syscall.
            if self.dry_run {
                return Ok(false);
            }
            return apply_unary(&op, &operand, self.var_is_set);
        }
        // <word> <binop> <word> — three-token binary form.
        if let (Some(_lhs), Some(op), Some(_rhs)) = (
            self.args.get(self.pos).map(String::as_str),
            self.args.get(self.pos + 1).map(String::as_str),
            self.args.get(self.pos + 2).map(String::as_str),
        ) && is_binary_op(op)
        {
            let lhs = self.args[self.pos].clone();
            let op = op.to_string();
            let rhs = self.args[self.pos + 2].clone();
            self.pos += 3;
            // Dry-run: tokens consumed, but no comparison / parse_int
            // (which would otherwise turn a syscall-free short-circuit
            // into a spurious "integer expression expected" error).
            if self.dry_run {
                return Ok(false);
            }
            return apply_binary(&op, &lhs, &rhs);
        }
        // Bare word — truthiness of the string.
        let word = self.take().unwrap_or("").to_string();
        if self.dry_run {
            return Ok(false);
        }
        Ok(!word.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn zero_args_is_false() {
        assert_eq!(evaluate(&[]), Ok(false));
    }

    #[test]
    fn compare_files_nt_ot_ef() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("huck_ntot_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let old = dir.join("old");
        let new = dir.join("new");
        std::fs::File::create(&old).unwrap().write_all(b"x").unwrap();
        // Force `new` to be strictly newer.
        std::thread::sleep(std::time::Duration::from_millis(10));
        std::fs::File::create(&new).unwrap().write_all(b"y").unwrap();
        let (o, n) = (old.to_str().unwrap(), new.to_str().unwrap());
        let missing = dir.join("missing");
        let m = missing.to_str().unwrap();

        assert!(compare_files("-nt", n, o)); // new newer than old
        assert!(!compare_files("-nt", o, n));
        assert!(compare_files("-ot", o, n)); // old older than new
        assert!(compare_files("-nt", n, m)); // exists -nt missing → true
        assert!(!compare_files("-nt", m, n)); // missing -nt exists → false
        assert!(compare_files("-ot", m, n)); // missing -ot exists → true
        assert!(!compare_files("-nt", m, m)); // both missing → false
        assert!(compare_files("-ef", o, o)); // same path → same inode
        assert!(!compare_files("-ef", o, n));
        assert!(!compare_files("-ef", m, m)); // missing -ef → false
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_v_uses_predicate() {
        let setvars = ["HOME", "EMPTYVAR"];
        let pred = |n: &str| setvars.contains(&n);
        assert_eq!(evaluate_with(&["-v".into(), "HOME".into()], &pred), Ok(true));
        assert_eq!(evaluate_with(&["-v".into(), "NOPE".into()], &pred), Ok(false));
    }

    #[test]
    fn one_arg_nonempty_is_true() {
        assert_eq!(evaluate(&args(&["x"])), Ok(true));
    }

    #[test]
    fn one_arg_empty_is_false() {
        assert_eq!(evaluate(&args(&[""])), Ok(false));
    }

    #[test]
    fn one_arg_operatorlike_is_just_truthiness() {
        assert_eq!(evaluate(&args(&["-f"])), Ok(true));
        assert_eq!(evaluate(&args(&["!"])), Ok(true));
    }

    #[test]
    fn bang_negates_one_arg_truthiness() {
        assert_eq!(evaluate(&args(&["!", "x"])), Ok(false));
        assert_eq!(evaluate(&args(&["!", ""])), Ok(true));
    }

    #[test]
    fn two_args_unknown_operator_is_usage_error() {
        assert!(evaluate(&args(&["foo", "bar"])).is_err());
    }

    #[test]
    fn three_args_no_operator_no_bang_is_usage_error() {
        assert!(evaluate(&args(&["a", "b", "c"])).is_err());
    }

    #[test]
    fn five_or_more_args_is_usage_error() {
        assert!(evaluate(&args(&["a", "b", "c", "d", "e"])).is_err());
    }

    #[test]
    fn four_args_without_leading_bang_is_usage_error() {
        assert!(evaluate(&args(&["a", "b", "c", "d"])).is_err());
    }

    #[test]
    fn unary_string_z_and_n() {
        assert_eq!(evaluate(&args(&["-z", ""])), Ok(true));
        assert_eq!(evaluate(&args(&["-z", "x"])), Ok(false));
        assert_eq!(evaluate(&args(&["-n", "x"])), Ok(true));
        assert_eq!(evaluate(&args(&["-n", ""])), Ok(false));
    }

    #[test]
    fn unary_file_exists_and_type() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("regular");
        std::fs::write(&file, b"data").unwrap();
        let subdir = dir.path().join("sub");
        std::fs::create_dir(&subdir).unwrap();
        let file_s = file.to_str().unwrap();
        let dir_s = subdir.to_str().unwrap();

        assert_eq!(evaluate(&args(&["-e", file_s])), Ok(true));
        assert_eq!(evaluate(&args(&["-e", dir_s])), Ok(true));
        assert_eq!(evaluate(&args(&["-f", file_s])), Ok(true));
        assert_eq!(evaluate(&args(&["-f", dir_s])), Ok(false));
        assert_eq!(evaluate(&args(&["-d", dir_s])), Ok(true));
        assert_eq!(evaluate(&args(&["-d", file_s])), Ok(false));
    }

    #[test]
    fn unary_file_nonexistent_is_false_not_error() {
        let r = evaluate(&args(&["-f", "/no/such/huck/path"]));
        assert_eq!(r, Ok(false));
    }

    #[test]
    fn unary_size_nonempty() {
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("empty");
        std::fs::write(&empty, b"").unwrap();
        let full = dir.path().join("full");
        std::fs::write(&full, b"content").unwrap();
        assert_eq!(evaluate(&args(&["-s", empty.to_str().unwrap()])), Ok(false));
        assert_eq!(evaluate(&args(&["-s", full.to_str().unwrap()])), Ok(true));
    }

    #[test]
    fn unary_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        std::fs::write(&target, b"x").unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert_eq!(evaluate(&args(&["-L", link.to_str().unwrap()])), Ok(true));
        assert_eq!(evaluate(&args(&["-L", target.to_str().unwrap()])), Ok(false));
        assert_eq!(evaluate(&args(&["-f", link.to_str().unwrap()])), Ok(true));
    }

    #[test]
    fn unary_readable_true_case() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("readable");
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(evaluate(&args(&["-r", file.to_str().unwrap()])), Ok(true));
        assert_eq!(evaluate(&args(&["-w", file.to_str().unwrap()])), Ok(true));
    }

    #[test]
    fn unary_negation_over_file_test() {
        let r = evaluate(&args(&["!", "-f", "/no/such/huck/path"]));
        assert_eq!(r, Ok(true));
    }

    #[test]
    fn unary_unknown_operator_in_one_arg_position_is_truthiness() {
        assert_eq!(evaluate(&args(&["-q"])), Ok(true));
    }

    #[test]
    fn binary_string_equality() {
        assert_eq!(evaluate(&args(&["abc", "=", "abc"])), Ok(true));
        assert_eq!(evaluate(&args(&["abc", "=", "xyz"])), Ok(false));
        assert_eq!(evaluate(&args(&["abc", "==", "abc"])), Ok(true));
        assert_eq!(evaluate(&args(&["abc", "!=", "xyz"])), Ok(true));
        assert_eq!(evaluate(&args(&["abc", "!=", "abc"])), Ok(false));
    }

    #[test]
    fn binary_string_empty_operands() {
        assert_eq!(evaluate(&args(&["", "=", ""])), Ok(true));
        assert_eq!(evaluate(&args(&["", "=", "x"])), Ok(false));
    }

    #[test]
    fn binary_integer_comparisons() {
        assert_eq!(evaluate(&args(&["3", "-eq", "3"])), Ok(true));
        assert_eq!(evaluate(&args(&["3", "-ne", "4"])), Ok(true));
        assert_eq!(evaluate(&args(&["3", "-lt", "10"])), Ok(true));
        assert_eq!(evaluate(&args(&["10", "-lt", "3"])), Ok(false));
        assert_eq!(evaluate(&args(&["3", "-le", "3"])), Ok(true));
        assert_eq!(evaluate(&args(&["10", "-gt", "3"])), Ok(true));
        assert_eq!(evaluate(&args(&["3", "-ge", "3"])), Ok(true));
    }

    #[test]
    fn binary_integer_negative_operands() {
        assert_eq!(evaluate(&args(&["-5", "-lt", "0"])), Ok(true));
        assert_eq!(evaluate(&args(&["-5", "-eq", "-5"])), Ok(true));
    }

    #[test]
    fn binary_integer_non_integer_operand_is_error() {
        assert!(evaluate(&args(&["abc", "-eq", "3"])).is_err());
        assert!(evaluate(&args(&["3", "-eq", "abc"])).is_err());
    }

    #[test]
    fn binary_negation_over_comparison() {
        assert_eq!(evaluate(&args(&["!", "a", "=", "b"])), Ok(true));
        assert_eq!(evaluate(&args(&["!", "a", "=", "a"])), Ok(false));
    }

    #[test]
    fn binary_negation_over_failing_comparison_stays_error() {
        assert!(evaluate(&args(&["!", "abc", "-eq", "3"])).is_err());
    }

    #[test]
    fn three_args_binary_op_wins_over_leading_bang() {
        // `! = x` — args[1] is the binary `=`, so this is a string
        // comparison of "!" against "x", not a negation.
        assert_eq!(evaluate(&args(&["!", "=", "x"])), Ok(false));
        assert_eq!(evaluate(&args(&["!", "=", "!"])), Ok(true));
    }

    #[test]
    fn binary_integer_plus_signed_operand() {
        // A leading `+` is accepted on integer operands.
        assert_eq!(evaluate(&args(&["+5", "-eq", "5"])), Ok(true));
    }

    #[test]
    fn unary_dash_a_is_file_exists_alias() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("present");
        std::fs::write(&file, b"data").unwrap();
        let file_s = file.to_str().unwrap();
        let missing = dir.path().join("absent");
        let missing_s = missing.to_str().unwrap();

        // 2-arg short-form: `[ -a present ]` → true.
        assert_eq!(evaluate(&args(&["-a", file_s])), Ok(true));
        // 2-arg short-form: `[ -a absent ]` → false (file does not exist).
        assert_eq!(evaluate(&args(&["-a", missing_s])), Ok(false));
    }

    #[test]
    fn dash_a_in_two_arg_position_is_unary_not_and() {
        // Sanity: `-a` in the unary-op position of a 2-arg call is
        // file-exists, not the AND combinator (which requires 3+ args
        // and operand on each side).
        assert_eq!(evaluate(&args(&["-a", "/"])), Ok(true));
    }

    #[test]
    fn combinator_and_both_true() {
        // [ -n a -a -n b ] → true
        assert_eq!(evaluate(&args(&["-n", "a", "-a", "-n", "b"])), Ok(true));
    }

    #[test]
    fn combinator_and_one_false() {
        // [ -z a -a -n b ] → false (left is false)
        assert_eq!(evaluate(&args(&["-z", "a", "-a", "-n", "b"])), Ok(false));
    }

    #[test]
    fn combinator_or_first_true() {
        // [ -n a -o -z b ] → true
        assert_eq!(evaluate(&args(&["-n", "a", "-o", "-z", "b"])), Ok(true));
    }

    #[test]
    fn combinator_or_both_false() {
        // [ -z a -o -z b ] → false
        assert_eq!(evaluate(&args(&["-z", "a", "-o", "-z", "b"])), Ok(false));
    }

    #[test]
    fn parens_simple_wrapping() {
        // [ ( -n a ) ] → true (falls through from 4-arg short-form)
        assert_eq!(evaluate(&args(&["(", "-n", "a", ")"])), Ok(true));
    }

    #[test]
    fn nested_parens() {
        // [ ( ( -n a ) ) ] → true
        assert_eq!(
            evaluate(&args(&["(", "(", "-n", "a", ")", ")"])),
            Ok(true)
        );
    }

    #[test]
    fn parens_group_changes_precedence() {
        // Without parens: [ -z a -o -n b -a -n c ] is `-z a -o (-n b -a -n c)`
        // = false OR (true AND true) = true.
        // With parens around the OR: [ ( -z a -o -n b ) -a -n c ]
        // = (false OR true) AND true = true.
        assert_eq!(
            evaluate(&args(&["(", "-z", "a", "-o", "-n", "b", ")", "-a", "-n", "c"])),
            Ok(true)
        );
    }

    #[test]
    fn precedence_and_higher_than_or() {
        // [ -z a -o -n b -a -n c ] = false OR (true AND true) = true.
        assert_eq!(
            evaluate(&args(&["-z", "a", "-o", "-n", "b", "-a", "-n", "c"])),
            Ok(true)
        );
    }

    #[test]
    fn negation_of_combinator_lhs() {
        // [ ! -n a -a -n b ] = (NOT (-n a)) AND (-n b) = false AND true = false.
        assert_eq!(evaluate(&args(&["!", "-n", "a", "-a", "-n", "b"])), Ok(false));
    }

    #[test]
    fn double_negation() {
        // [ ! ! -n a ] = NOT NOT true = true.
        assert_eq!(evaluate(&args(&["!", "!", "-n", "a"])), Ok(true));
    }

    #[test]
    fn empty_parens_error() {
        let r = evaluate(&args(&["(", ")"]));
        assert!(r.is_err(), "expected error, got {r:?}");
        assert!(
            r.unwrap_err().contains("expression"),
            "expected 'expression' in error"
        );
    }

    #[test]
    fn unbalanced_open_paren_error() {
        let r = evaluate(&args(&["(", "-n", "a"]));
        assert!(r.is_err());
        assert!(r.unwrap_err().contains(")"), "expected ')' in error");
    }

    #[test]
    fn unbalanced_close_paren_error() {
        // [ -n a ) ] — `)` at position 2 is an unexpected token; falls
        // through short-form to grammar, which rejects.
        let r = evaluate(&args(&["-n", "a", ")"]));
        assert!(r.is_err());
    }

    #[test]
    fn dangling_combinator_at_end_error() {
        // [ -n a -a ] — falls through short-form (4 args, not !-prefixed),
        // parser consumes `-n a -a`, then `parse_unary` runs out of input.
        let r = evaluate(&args(&["-n", "a", "-a"]));
        assert!(r.is_err());
        assert!(
            r.unwrap_err().contains("expression"),
            "expected 'expression' in error"
        );
    }

    #[test]
    fn combinator_with_binary_operands() {
        // [ a = a -a 1 -lt 2 ] = (a == a) AND (1 < 2) = true.
        assert_eq!(
            evaluate(&args(&["a", "=", "a", "-a", "1", "-lt", "2"])),
            Ok(true)
        );
    }

    #[test]
    fn mixed_unary_and_binary() {
        // [ -e /tmp -a 1 -lt 2 ] = true AND true = true.
        assert_eq!(
            evaluate(&args(&["-e", "/tmp", "-a", "1", "-lt", "2"])),
            Ok(true)
        );
    }

    #[test]
    fn long_and_chain_left_associative() {
        // [ -n a -a -n b -a -n c -a -n d ] = all true.
        assert_eq!(
            evaluate(&args(&[
                "-n", "a", "-a", "-n", "b", "-a", "-n", "c", "-a", "-n", "d"
            ])),
            Ok(true)
        );
    }

    #[test]
    fn or_chain_left_associative() {
        // [ -z a -o -z b -o -n c ] = false OR false OR true = true.
        assert_eq!(
            evaluate(&args(&["-z", "a", "-o", "-z", "b", "-o", "-n", "c"])),
            Ok(true)
        );
    }

    #[test]
    fn short_circuit_and_skips_rhs_file_check() {
        // [ -z "" -a -f /no/such/path/that/does/not/exist ]
        // LHS is true (-z "" → true), so -a evaluates the RHS.
        // Verify the result is false (RHS file doesn't exist).
        assert_eq!(
            evaluate(&args(&["-z", "", "-a", "-f", "/no/such/path/that/does/not/exist"])),
            Ok(false)
        );
    }

    #[test]
    fn short_circuit_and_lhs_false_skips_rhs() {
        // [ -n "" -a -f /no/such/path ]
        // LHS is false (-n "" → false), so -a short-circuits; RHS is
        // NOT evaluated (no syscall on the non-existent path). Result
        // is false either way, but a future test could assert the
        // syscall count is 0 if we add instrumentation. For now,
        // just verify the result is false.
        assert_eq!(
            evaluate(&args(&["-n", "", "-a", "-f", "/no/such/path"])),
            Ok(false)
        );
    }

    #[test]
    fn short_circuit_or_lhs_true_skips_rhs() {
        // [ -z "" -o -f /no/such/path ]
        // LHS is true (-z "" → true), so -o short-circuits; RHS NOT
        // evaluated. Result is true.
        assert_eq!(
            evaluate(&args(&["-z", "", "-o", "-f", "/no/such/path"])),
            Ok(true)
        );
    }

    #[test]
    fn negation_wrapping_paren_group() {
        // [ ! ( -n a ) ] = NOT (true) = false.
        assert_eq!(evaluate(&args(&["!", "(", "-n", "a", ")"])), Ok(false));
    }

    #[test]
    fn negation_inside_parens() {
        // [ ( ! -n a ) -a -n b ] = (NOT true) AND true = false.
        assert_eq!(
            evaluate(&args(&["(", "!", "-n", "a", ")", "-a", "-n", "b"])),
            Ok(false)
        );
    }
}
