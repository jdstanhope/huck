//! The `test` / `[` builtin: conditional expression evaluation.
//!
//! `evaluate` implements the POSIX argument-count algorithm. Operator
//! application lives in `apply_unary` / `apply_binary`.

/// Evaluates a `test` expression. `Ok(true)` / `Ok(false)` are the
/// result; `Err(message)` is a usage error.
pub fn evaluate(args: &[String]) -> Result<bool, String> {
    match args.len() {
        0 => Ok(false),
        1 => Ok(!args[0].is_empty()),
        2 => {
            if args[0] == "!" {
                negate(evaluate(&args[1..2]))
            } else if is_unary_op(&args[0]) {
                apply_unary(&args[0], &args[1])
            } else {
                Err(format!("{}: unary operator expected", args[0]))
            }
        }
        3 => {
            if is_binary_op(&args[1]) {
                apply_binary(&args[1], &args[0], &args[2])
            } else if args[0] == "!" {
                negate(evaluate(&args[1..3]))
            } else {
                Err(format!("{}: binary operator expected", args[1]))
            }
        }
        4 => {
            if args[0] == "!" {
                negate(evaluate(&args[1..4]))
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
        "-e" | "-f" | "-d" | "-r" | "-w" | "-x" | "-s" | "-L" | "-z" | "-n"
    )
}

/// True if `s` is a recognized binary operator.
fn is_binary_op(s: &str) -> bool {
    matches!(
        s,
        "=" | "==" | "!=" | "-eq" | "-ne" | "-lt" | "-le" | "-gt" | "-ge"
    )
}

/// Applies a unary operator to its operand.
fn apply_unary(op: &str, operand: &str) -> Result<bool, String> {
    match op {
        "-z" => Ok(operand.is_empty()),
        "-n" => Ok(!operand.is_empty()),
        "-e" => Ok(std::fs::metadata(operand).is_ok()),
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
        _ => Err(format!("{op}: unknown operator")),
    }
}

/// Parses a `test` integer operand (decimal `i64`).
fn parse_int(s: &str) -> Result<i64, String> {
    s.parse::<i64>()
        .map_err(|_| "integer expression expected".to_string())
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
}
