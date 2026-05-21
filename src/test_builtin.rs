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

/// Applies a unary operator. Implemented in Task 2.
fn apply_unary(op: &str, _operand: &str) -> Result<bool, String> {
    Err(format!("{op}: operator not yet implemented"))
}

/// Applies a binary operator. Implemented in Task 3.
fn apply_binary(op: &str, _lhs: &str, _rhs: &str) -> Result<bool, String> {
    Err(format!("{op}: operator not yet implemented"))
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
}
