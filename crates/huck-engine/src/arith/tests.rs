use super::*;

#[test]
fn display_division_by_zero() {
    assert_eq!(
        ArithError {
            kind: ArithErrorKind::DivisionByZero,
            offset: None,
            token_end: None
        }
        .to_string(),
        "division by zero"
    );
}

#[test]
fn display_modulo_by_zero() {
    assert_eq!(
        ArithError {
            kind: ArithErrorKind::ModuloByZero,
            offset: None,
            token_end: None
        }
        .to_string(),
        "modulo by zero"
    );
}

#[test]
fn display_parse_error_is_bare_message() {
    let e = ArithError::parse("unexpected end of input");
    assert_eq!(e.to_string(), "unexpected end of input");
}

#[test]
fn display_not_an_integer_quotes_var_and_value() {
    let e = ArithError {
        kind: ArithErrorKind::NotAnInteger {
            var: "x".to_string(),
            value: "abc".to_string(),
        },
        offset: None,
        token_end: None,
    };
    assert_eq!(e.to_string(), "variable 'x' is not an integer: 'abc'");
}

#[test]
fn tokenize_single_number() {
    let (toks, _) = tokenize("42").unwrap();
    assert_eq!(toks, vec![ArithToken::Number(42)]);
}

#[test]
fn tokenize_zero() {
    let (toks, _) = tokenize("0").unwrap();
    assert_eq!(toks, vec![ArithToken::Number(0)]);
}

#[test]
fn tokenize_large_number() {
    let (toks, _) = tokenize("9223372036854775807").unwrap();
    assert_eq!(toks, vec![ArithToken::Number(i64::MAX)]);
}

#[test]
fn tokenize_number_overflow_is_parse_error() {
    let err = tokenize("99999999999999999999").unwrap_err();
    assert!(
        matches!(err.kind, ArithErrorKind::Parse(_)),
        "got {:?}",
        err
    );
}

#[test]
fn tokenize_identifier() {
    let (toks, _) = tokenize("foo").unwrap();
    assert_eq!(toks, vec![ArithToken::Ident("foo".to_string())]);
}

#[test]
fn tokenize_identifier_with_dollar_prefix_strips_dollar() {
    let (toks, _) = tokenize("$foo").unwrap();
    assert_eq!(toks, vec![ArithToken::Ident("foo".to_string())]);
}

#[test]
fn tokenize_single_char_operators() {
    let input = "+ - * / % ( ) ! ? :";
    let expected = vec![
        ArithToken::Plus,
        ArithToken::Minus,
        ArithToken::Star,
        ArithToken::Slash,
        ArithToken::Percent,
        ArithToken::LParen,
        ArithToken::RParen,
        ArithToken::Bang,
        ArithToken::Question,
        ArithToken::Colon,
    ];
    let (toks, _) = tokenize(input).unwrap();
    assert_eq!(toks, expected);
}

#[test]
fn tokenize_multi_char_operators() {
    let input = "== != <= >= && || < >";
    let expected = vec![
        ArithToken::Eq,
        ArithToken::Ne,
        ArithToken::Le,
        ArithToken::Ge,
        ArithToken::AndAnd,
        ArithToken::OrOr,
        ArithToken::Lt,
        ArithToken::Gt,
    ];
    let (toks, _) = tokenize(input).unwrap();
    assert_eq!(toks, expected);
}

#[test]
fn tokenize_skips_whitespace() {
    let (toks, _) = tokenize("  1   +   2  ").unwrap();
    assert_eq!(
        toks,
        vec![
            ArithToken::Number(1),
            ArithToken::Plus,
            ArithToken::Number(2)
        ]
    );
}

#[test]
fn tokenize_unknown_char_is_parse_error() {
    let err = tokenize("1 @ 2").unwrap_err();
    assert!(matches!(err.kind, ArithErrorKind::Parse(_)));
}

#[test]
fn tokenize_single_amp_is_bitwise_and() {
    // v38: bare & is now bitwise AND (was: parse error).
    let (toks, _) = tokenize("1 & 2").unwrap();
    assert_eq!(
        toks,
        vec![
            ArithToken::Number(1),
            ArithToken::Amp,
            ArithToken::Number(2),
        ]
    );
}

#[test]
fn tokenize_single_pipe_is_bitwise_or() {
    // v38: bare | is now bitwise OR (was: parse error).
    let (toks, _) = tokenize("1 | 2").unwrap();
    assert_eq!(
        toks,
        vec![
            ArithToken::Number(1),
            ArithToken::Pipe,
            ArithToken::Number(2),
        ]
    );
}

#[test]
fn tokenize_hex_literal() {
    let (toks, _) = tokenize("0x10").unwrap();
    assert_eq!(toks, vec![ArithToken::Number(16)]);
}

#[test]
fn tokenize_hex_literal_uppercase() {
    let (toks, _) = tokenize("0X1F").unwrap();
    assert_eq!(toks, vec![ArithToken::Number(31)]);
}

#[test]
fn tokenize_octal_literal() {
    let (toks, _) = tokenize("010").unwrap();
    assert_eq!(toks, vec![ArithToken::Number(8)]);
}

#[test]
fn tokenize_octal_invalid_digit_errors() {
    // 08 has a digit (8) that's invalid for octal.
    assert!(tokenize("08").is_err());
}

#[test]
fn tokenize_base_n_binary() {
    let (toks, _) = tokenize("2#1010").unwrap();
    assert_eq!(toks, vec![ArithToken::Number(10)]);
}

#[test]
fn tokenize_base_n_hex_via_pound() {
    let (toks, _) = tokenize("16#FF").unwrap();
    assert_eq!(toks, vec![ArithToken::Number(255)]);
}

#[test]
fn tokenize_base_n_invalid_base_low_errors() {
    assert!(tokenize("1#0").is_err());
}

#[test]
fn tokenize_base_n_invalid_base_high_errors() {
    assert!(tokenize("65#0").is_err());
}

#[test]
fn tokenize_base_n_invalid_digit_errors() {
    // Base 8 cannot have digit 9.
    assert!(tokenize("8#9").is_err());
}

#[test]
fn tokenize_bitwise_operators() {
    let (toks, _) = tokenize("&|^~<<>>").unwrap();
    assert_eq!(
        toks,
        vec![
            ArithToken::Amp,
            ArithToken::Pipe,
            ArithToken::Caret,
            ArithToken::Tilde,
            ArithToken::Shl,
            ArithToken::Shr,
        ]
    );
}

#[test]
fn tokenize_power_operator() {
    let (toks, _) = tokenize("2**3").unwrap();
    assert_eq!(
        toks,
        vec![
            ArithToken::Number(2),
            ArithToken::Power,
            ArithToken::Number(3),
        ]
    );
}

#[test]
fn tokenize_compound_assignments() {
    // = += -= *= /= %= <<= >>= &= ^= |=
    let input = "= += -= *= /= %= <<= >>= &= ^= |=";
    let (tokens, _) = tokenize(input).unwrap();
    assert_eq!(
        tokens,
        vec![
            ArithToken::Assign,
            ArithToken::PlusEq,
            ArithToken::MinusEq,
            ArithToken::StarEq,
            ArithToken::SlashEq,
            ArithToken::PercentEq,
            ArithToken::ShlEq,
            ArithToken::ShrEq,
            ArithToken::AmpEq,
            ArithToken::CaretEq,
            ArithToken::PipeEq,
        ]
    );
}

#[test]
fn tokenize_inc_dec_operators() {
    let (toks, _) = tokenize("++ --").unwrap();
    assert_eq!(toks, vec![ArithToken::PlusPlus, ArithToken::MinusMinus]);
}

#[test]
fn tokenize_distinguishes_eq_from_assign() {
    let (toks, _) = tokenize("==").unwrap();
    assert_eq!(toks, vec![ArithToken::Eq]);
    let (toks, _) = tokenize("=").unwrap();
    assert_eq!(toks, vec![ArithToken::Assign]);
}

#[test]
fn tokenize_distinguishes_lt_from_shl() {
    let (toks, _) = tokenize("<").unwrap();
    assert_eq!(toks, vec![ArithToken::Lt]);
    let (toks, _) = tokenize("<<").unwrap();
    assert_eq!(toks, vec![ArithToken::Shl]);
    let (toks, _) = tokenize("<<=").unwrap();
    assert_eq!(toks, vec![ArithToken::ShlEq]);
}

fn n(x: i64) -> Box<ArithExpr> {
    Box::new(ArithExpr::Num(x))
}
fn v(name: &str) -> Box<ArithExpr> {
    Box::new(ArithExpr::Var(name.to_string()))
}

#[test]
fn parse_number_literal() {
    assert_eq!(parse("42").unwrap(), ArithExpr::Num(42));
}

#[test]
fn parse_identifier() {
    assert_eq!(parse("foo").unwrap(), ArithExpr::Var("foo".to_string()));
}

#[test]
fn parse_addition() {
    assert_eq!(parse("1+2").unwrap(), ArithExpr::Add(n(1), n(2)));
}

#[test]
fn parse_subtraction_left_associative() {
    assert_eq!(
        parse("1-2-3").unwrap(),
        ArithExpr::Sub(Box::new(ArithExpr::Sub(n(1), n(2))), n(3))
    );
}

#[test]
fn parse_multiplication_binds_tighter_than_addition() {
    assert_eq!(
        parse("1+2*3").unwrap(),
        ArithExpr::Add(n(1), Box::new(ArithExpr::Mul(n(2), n(3))))
    );
}

#[test]
fn parse_parenthesized_overrides_precedence() {
    assert_eq!(
        parse("(1+2)*3").unwrap(),
        ArithExpr::Mul(Box::new(ArithExpr::Add(n(1), n(2))), n(3))
    );
}

#[test]
fn parse_unary_minus() {
    assert_eq!(parse("-5").unwrap(), ArithExpr::Neg(n(5)));
}

#[test]
fn parse_double_minus_with_number_is_prefix_dec_error() {
    // v38: -- is now MinusMinus (prefix decrement), which requires a
    // variable name after it. --5 → parse error.
    assert!(matches!(parse("--5"), Err(ref e) if matches!(e.kind, ArithErrorKind::Parse(_))));
}

#[test]
fn parse_unary_minus_double_negation_uses_space() {
    // To express double negation of a literal, use a space: - -5.
    assert_eq!(
        parse("- -5").unwrap(),
        ArithExpr::Neg(Box::new(ArithExpr::Neg(n(5))))
    );
}

#[test]
fn parse_unary_not() {
    assert_eq!(parse("!0").unwrap(), ArithExpr::Not(n(0)));
}

#[test]
fn parse_comparison() {
    assert_eq!(parse("1<2").unwrap(), ArithExpr::Lt(n(1), n(2)));
}

#[test]
fn parse_equality_lower_than_comparison() {
    assert_eq!(
        parse("1<2 == 1").unwrap(),
        ArithExpr::Eq(Box::new(ArithExpr::Lt(n(1), n(2))), n(1))
    );
}

#[test]
fn parse_logical_and_binds_tighter_than_or() {
    assert_eq!(
        parse("a||b&&c").unwrap(),
        ArithExpr::Or(v("a"), Box::new(ArithExpr::And(v("b"), v("c"))))
    );
}

#[test]
fn parse_ternary_right_associative() {
    assert_eq!(
        parse("a?b:c?d:e").unwrap(),
        ArithExpr::Ternary(
            v("a"),
            v("b"),
            Box::new(ArithExpr::Ternary(v("c"), v("d"), v("e")))
        )
    );
}

#[test]
fn parse_empty_is_error() {
    // Empty input → OperandExpected (bump returns None, err_off stays 0).
    assert!(matches!(
        parse("").unwrap_err().kind,
        ArithErrorKind::OperandExpected
    ));
}

#[test]
fn parse_trailing_junk_is_error() {
    // Trailing junk → SyntaxErrorInExpression at the junk token's offset.
    assert!(matches!(
        parse("1+2 3").unwrap_err().kind,
        ArithErrorKind::SyntaxErrorInExpression
    ));
}

#[test]
fn parse_unbalanced_paren_is_error() {
    // Missing ')' → MissingCloseParen.
    assert!(matches!(
        parse("(1+2").unwrap_err().kind,
        ArithErrorKind::MissingCloseParen
    ));
}

#[test]
fn parse_missing_rhs_is_error() {
    // Missing RHS operand → OperandExpected.
    assert!(matches!(
        parse("1+").unwrap_err().kind,
        ArithErrorKind::OperandExpected
    ));
}

#[test]
fn parse_strips_dollar_on_var() {
    assert_eq!(parse("$x + 1").unwrap(), ArithExpr::Add(v("x"), n(1)));
}

#[test]
fn parse_bitwise_precedence_or_below_and() {
    // 1 | 2 & 3 parses as 1 | (2 & 3) — & binds tighter than |.
    let expr = parse("1 | 2 & 3").unwrap();
    assert_eq!(
        expr,
        ArithExpr::BitOr(n(1), Box::new(ArithExpr::BitAnd(n(2), n(3))),)
    );
}

#[test]
fn parse_shift_below_addition() {
    // 1 + 2 << 3 parses as (1 + 2) << 3 — << has lower precedence than +.
    let expr = parse("1 + 2 << 3").unwrap();
    assert_eq!(
        expr,
        ArithExpr::Shl(Box::new(ArithExpr::Add(n(1), n(2))), n(3),)
    );
}

#[test]
fn parse_power_right_associative() {
    // 2 ** 3 ** 2 parses as Pow(2, Pow(3, 2)).
    let expr = parse("2 ** 3 ** 2").unwrap();
    assert_eq!(
        expr,
        ArithExpr::Pow(n(2), Box::new(ArithExpr::Pow(n(3), n(2))),)
    );
}

#[test]
fn parse_assignment_right_associative() {
    // a = b = 5 parses as Assign(a, Set, Assign(b, Set, 5)).
    let expr = parse("a = b = 5").unwrap();
    assert_eq!(
        expr,
        ArithExpr::Assign {
            target: LValue::Scalar("a".to_string()),
            op: AssignOp::Set,
            rhs: Box::new(ArithExpr::Assign {
                target: LValue::Scalar("b".to_string()),
                op: AssignOp::Set,
                rhs: n(5),
            }),
        }
    );
}

#[test]
fn parse_assignment_lhs_must_be_var() {
    // (a + b) = 5 → AssignToNonVar (LHS not a scalar var or array element).
    assert!(
        matches!(parse("(a + b) = 5"), Err(ref e) if matches!(e.kind, ArithErrorKind::AssignToNonVar))
    );
}

#[test]
fn parse_postfix_lhs_must_be_var() {
    // (a + b)++ → parse error.
    assert!(matches!(parse("(a + b)++"), Err(ref e) if matches!(e.kind, ArithErrorKind::Parse(_))));
}

#[test]
fn parse_compound_assignment_all_forms() {
    // All 11 compound assignment forms.
    let cases = [
        ("a = 1", AssignOp::Set),
        ("a += 1", AssignOp::Add),
        ("a -= 1", AssignOp::Sub),
        ("a *= 1", AssignOp::Mul),
        ("a /= 1", AssignOp::Div),
        ("a %= 1", AssignOp::Mod),
        ("a <<= 1", AssignOp::Shl),
        ("a >>= 1", AssignOp::Shr),
        ("a &= 1", AssignOp::BitAnd),
        ("a ^= 1", AssignOp::BitXor),
        ("a |= 1", AssignOp::BitOr),
    ];
    for (input, expected_op) in cases {
        let expr = parse(input).unwrap();
        match expr {
            ArithExpr::Assign { target, op, rhs } => {
                assert_eq!(target, LValue::Scalar("a".to_string()), "for input {input}");
                assert_eq!(op, expected_op, "for input {input}");
                assert_eq!(*rhs, ArithExpr::Num(1), "for input {input}");
            }
            other => panic!("expected Assign for {input}, got {other:?}"),
        }
    }
}

#[test]
fn parse_pre_post_inc_dec() {
    assert_eq!(
        parse("++a").unwrap(),
        ArithExpr::PreInc(LValue::Scalar("a".to_string()))
    );
    assert_eq!(
        parse("--a").unwrap(),
        ArithExpr::PreDec(LValue::Scalar("a".to_string()))
    );
    assert_eq!(
        parse("a++").unwrap(),
        ArithExpr::PostInc(LValue::Scalar("a".to_string()))
    );
    assert_eq!(
        parse("a--").unwrap(),
        ArithExpr::PostDec(LValue::Scalar("a".to_string()))
    );
}

use crate::shell_state::Shell;

fn eval_str(s: &str, shell: &mut Shell) -> Result<i64, ArithError> {
    eval(&parse(s).unwrap(), shell)
}

#[test]
fn comma_value_is_last_operand() {
    let mut s = Shell::new();
    assert_eq!(eval_str("1, 2, 3", &mut s).unwrap(), 3);
}

#[test]
fn comma_keeps_side_effects_of_all_operands() {
    let mut s = Shell::new();
    // a=1 then b=2; value is the last (2); both vars set.
    assert_eq!(eval_str("a=1, b=2", &mut s).unwrap(), 2);
    assert_eq!(s.lookup_var("a").as_deref(), Some("1"));
    assert_eq!(s.lookup_var("b").as_deref(), Some("2"));
}

#[test]
fn comma_is_lower_precedence_than_assignment() {
    // `a = 1, 2` is `(a=1), 2`: value 2, a==1 (NOT a=(1,2)=2).
    let mut s = Shell::new();
    assert_eq!(eval_str("a = 1, 2", &mut s).unwrap(), 2);
    assert_eq!(s.lookup_var("a").as_deref(), Some("1"));
}

#[test]
fn comma_inside_parens() {
    let mut s = Shell::new();
    assert_eq!(eval_str("(1, 2) + 3", &mut s).unwrap(), 5);
}

#[test]
fn comma_side_effect_ordering() {
    // i=0 then i++ : value of i++ is 0, i becomes 1.
    let mut s = Shell::new();
    assert_eq!(eval_str("i=0, i++", &mut s).unwrap(), 0);
    assert_eq!(s.lookup_var("i").as_deref(), Some("1"));
}

#[test]
fn comma_nested_left_fold() {
    let mut s = Shell::new();
    assert_eq!(eval_str("(1,2),3", &mut s).unwrap(), 3);
}

#[test]
fn trailing_comma_is_error() {
    // `eval_str` unwraps `parse`, so a parse error would panic rather than
    // surface as Err — assert on `parse` directly to capture the error.
    assert!(parse("1,").is_err());
}

#[test]
fn leading_comma_is_error() {
    assert!(parse(",1").is_err());
}

#[test]
fn eval_number_literal() {
    let mut s = Shell::new();
    assert_eq!(eval_str("42", &mut s).unwrap(), 42);
}

#[test]
fn eval_addition() {
    let mut s = Shell::new();
    assert_eq!(eval_str("1+2", &mut s).unwrap(), 3);
}

#[test]
fn eval_precedence() {
    let mut s = Shell::new();
    assert_eq!(eval_str("2+3*4", &mut s).unwrap(), 14);
    assert_eq!(eval_str("(2+3)*4", &mut s).unwrap(), 20);
}

#[test]
fn eval_subtraction_left_assoc() {
    let mut s = Shell::new();
    assert_eq!(eval_str("1-2-3", &mut s).unwrap(), -4);
}

#[test]
fn eval_unary_minus() {
    let mut s = Shell::new();
    assert_eq!(eval_str("-5", &mut s).unwrap(), -5);
    // v38: "--5" now parses as prefix-decrement on a literal → error.
    // Use "- -5" (with space) for explicit double-negation.
    assert_eq!(eval_str("- -5", &mut s).unwrap(), 5);
}

#[test]
fn eval_division_truncates_toward_zero() {
    let mut s = Shell::new();
    assert_eq!(eval_str("7/2", &mut s).unwrap(), 3);
    assert_eq!(eval_str("-7/2", &mut s).unwrap(), -3);
}

#[test]
fn eval_modulo() {
    let mut s = Shell::new();
    assert_eq!(eval_str("7%3", &mut s).unwrap(), 1);
    assert_eq!(eval_str("-7%3", &mut s).unwrap(), -1);
}

#[test]
fn eval_division_by_zero() {
    let mut s = Shell::new();
    // offset 2 = byte offset of the `0` divisor token in "1/0"
    assert_eq!(
        eval_str("1/0", &mut s).unwrap_err(),
        ArithError {
            kind: ArithErrorKind::DivisionByZero,
            offset: Some(2),
            token_end: None
        }
    );
}

#[test]
fn eval_modulo_by_zero() {
    let mut s = Shell::new();
    // offset 2 = byte offset of the `0` divisor token in "1%0"
    assert_eq!(
        eval_str("1%0", &mut s).unwrap_err(),
        ArithError {
            kind: ArithErrorKind::ModuloByZero,
            offset: Some(2),
            token_end: None
        }
    );
}

#[test]
fn render_division_by_zero_token() {
    let mut sh = Shell::new();
    let expr = parse("44 / 0 ").unwrap();
    let err = eval(&expr, &mut sh).unwrap_err();
    assert_eq!(
        render_error_body("44 / 0 ", &err),
        "44 / 0 : division by 0 (error token is \"0 \")"
    );
}

#[test]
fn eval_comparison_returns_one_or_zero() {
    let mut s = Shell::new();
    assert_eq!(eval_str("1<2", &mut s).unwrap(), 1);
    assert_eq!(eval_str("2<1", &mut s).unwrap(), 0);
    assert_eq!(eval_str("1==1", &mut s).unwrap(), 1);
    assert_eq!(eval_str("1!=1", &mut s).unwrap(), 0);
}

#[test]
fn eval_logical_not() {
    let mut s = Shell::new();
    assert_eq!(eval_str("!0", &mut s).unwrap(), 1);
    assert_eq!(eval_str("!5", &mut s).unwrap(), 0);
    assert_eq!(eval_str("!!5", &mut s).unwrap(), 1);
}

#[test]
fn eval_logical_and_short_circuits() {
    let mut s = Shell::new();
    assert_eq!(eval_str("0 && 1/0", &mut s).unwrap(), 0);
}

#[test]
fn eval_logical_or_short_circuits() {
    let mut s = Shell::new();
    assert_eq!(eval_str("1 || 1/0", &mut s).unwrap(), 1);
}

#[test]
fn eval_logical_and_returns_one_when_both_truthy() {
    let mut s = Shell::new();
    assert_eq!(eval_str("5 && 3", &mut s).unwrap(), 1);
}

#[test]
fn eval_logical_or_returns_one_when_either_truthy() {
    let mut s = Shell::new();
    assert_eq!(eval_str("0 || 3", &mut s).unwrap(), 1);
    assert_eq!(eval_str("0 || 0", &mut s).unwrap(), 0);
}

#[test]
fn eval_ternary() {
    let mut s = Shell::new();
    assert_eq!(eval_str("1 ? 42 : 99", &mut s).unwrap(), 42);
    assert_eq!(eval_str("0 ? 42 : 99", &mut s).unwrap(), 99);
}

#[test]
fn eval_overflow_wraps() {
    let mut s = Shell::new();
    let max = i64::MAX.to_string();
    let expr = format!("{max} + 1");
    assert_eq!(eval_str(&expr, &mut s).unwrap(), i64::MIN);
}

#[test]
fn eval_unset_var_is_zero() {
    let mut s = Shell::new();
    assert_eq!(eval_str("HUCK_TEST_UNSET_ARITH + 5", &mut s).unwrap(), 5);
}

#[test]
fn eval_set_var_lookup() {
    let mut s = Shell::new();
    s.export_set("HUCK_TEST_ARITH_X", "10".to_string());
    assert_eq!(eval_str("HUCK_TEST_ARITH_X * 2", &mut s).unwrap(), 20);
}

#[test]
fn eval_var_with_dollar_prefix_same_as_bare() {
    let mut s = Shell::new();
    s.export_set("HUCK_TEST_ARITH_Y", "7".to_string());
    assert_eq!(eval_str("$HUCK_TEST_ARITH_Y + 1", &mut s).unwrap(), 8);
}

#[test]
fn eval_empty_var_is_zero() {
    let mut s = Shell::new();
    s.export_set("HUCK_TEST_ARITH_EMPTY", "".to_string());
    assert_eq!(eval_str("HUCK_TEST_ARITH_EMPTY + 3", &mut s).unwrap(), 3);
}

#[test]
fn eval_non_integer_var_is_error() {
    let mut s = Shell::new();
    s.export_set("HUCK_TEST_ARITH_BAD", "abc".to_string());
    let err = eval_str("HUCK_TEST_ARITH_BAD + 1", &mut s).unwrap_err();
    assert_eq!(
        err,
        ArithError {
            kind: ArithErrorKind::NotAnInteger {
                var: "HUCK_TEST_ARITH_BAD".to_string(),
                value: "abc".to_string()
            },
            offset: None,
            token_end: None
        }
    );
}

#[test]
fn eval_bitwise_and() {
    let mut s = Shell::new();
    assert_eq!(eval_str("0xF0 & 0x0F", &mut s).unwrap(), 0);
    assert_eq!(eval_str("0xFF & 0x33", &mut s).unwrap(), 0x33);
}

#[test]
fn eval_bitwise_or() {
    let mut s = Shell::new();
    assert_eq!(eval_str("0xF0 | 0x0F", &mut s).unwrap(), 0xFF);
}

#[test]
fn eval_bitwise_xor() {
    let mut s = Shell::new();
    assert_eq!(eval_str("0xFF ^ 0x0F", &mut s).unwrap(), 0xF0);
}

#[test]
fn eval_bitwise_not() {
    let mut s = Shell::new();
    assert_eq!(eval_str("~0", &mut s).unwrap(), -1);
    assert_eq!(eval_str("~(-1)", &mut s).unwrap(), 0);
}

#[test]
fn eval_left_shift() {
    let mut s = Shell::new();
    assert_eq!(eval_str("1 << 4", &mut s).unwrap(), 16);
    assert_eq!(eval_str("1 << 0", &mut s).unwrap(), 1);
}

#[test]
fn eval_arithmetic_right_shift_preserves_sign() {
    let mut s = Shell::new();
    // Rust's i64 >> is arithmetic right shift; sign bit replicates.
    assert_eq!(eval_str("(-8) >> 1", &mut s).unwrap(), -4);
    assert_eq!(eval_str("16 >> 2", &mut s).unwrap(), 4);
}

#[test]
fn eval_shift_negative_count_errors() {
    let mut s = Shell::new();
    assert!(matches!(
        eval_str("1 << -1", &mut s),
        Err(ArithError {
            kind: ArithErrorKind::ShiftCountOutOfRange { count: -1 },
            ..
        })
    ));
}

#[test]
fn eval_shift_count_64_or_more_errors() {
    let mut s = Shell::new();
    assert!(matches!(
        eval_str("1 << 64", &mut s),
        Err(ArithError {
            kind: ArithErrorKind::ShiftCountOutOfRange { count: 64 },
            ..
        })
    ));
}

#[test]
fn eval_pow_basic() {
    let mut s = Shell::new();
    assert_eq!(eval_str("2 ** 10", &mut s).unwrap(), 1024);
}

#[test]
fn eval_pow_zero_exponent() {
    let mut s = Shell::new();
    assert_eq!(eval_str("5 ** 0", &mut s).unwrap(), 1);
    assert_eq!(eval_str("0 ** 0", &mut s).unwrap(), 1);
}

#[test]
fn eval_pow_negative_exponent_errors() {
    let mut s = Shell::new();
    assert!(matches!(
        eval_str("2 ** -1", &mut s),
        Err(ArithError {
            kind: ArithErrorKind::NegativeExponent,
            ..
        })
    ));
}

#[test]
fn eval_assign_basic_mutates_shell() {
    let mut s = Shell::new();
    assert_eq!(eval_str("a = 5", &mut s).unwrap(), 5);
    assert_eq!(s.lookup_var("a"), Some("5".to_string()));
}

#[test]
fn eval_assign_compound_add() {
    let mut s = Shell::new();
    s.set("a", "3".to_string());
    assert_eq!(eval_str("a += 4", &mut s).unwrap(), 7);
    assert_eq!(s.lookup_var("a"), Some("7".to_string()));
}

#[test]
fn eval_assign_div_by_zero_errors() {
    let mut s = Shell::new();
    s.set("a", "10".to_string());
    assert!(matches!(
        eval_str("a /= 0", &mut s),
        Err(ArithError {
            kind: ArithErrorKind::DivisionByZero,
            ..
        })
    ));
}

#[test]
fn eval_pre_inc_returns_new_value() {
    let mut s = Shell::new();
    s.set("a", "5".to_string());
    assert_eq!(eval_str("++a", &mut s).unwrap(), 6);
    assert_eq!(s.lookup_var("a"), Some("6".to_string()));
}

#[test]
fn parse_index_read() {
    assert_eq!(
        parse("arr[0]").unwrap(),
        ArithExpr::Index {
            name: "arr".to_string(),
            subscript: n(0),
            subscript_raw: "0".to_string(),
        }
    );
}

#[test]
fn parse_index_arith_subscript_keeps_raw() {
    // The parsed subscript is an arith expr; the raw text is preserved
    // verbatim (used as an associative key).
    let expr = parse("m[1+1]").unwrap();
    match expr {
        ArithExpr::Index {
            name,
            subscript,
            subscript_raw,
        } => {
            assert_eq!(name, "m");
            assert_eq!(*subscript, ArithExpr::Add(n(1), n(1)));
            assert_eq!(subscript_raw, "1+1");
        }
        other => panic!("expected Index, got {other:?}"),
    }
}

#[test]
fn parse_index_assign_lvalue() {
    let expr = parse("a[2] = 9").unwrap();
    match expr {
        ArithExpr::Assign { target, op, rhs } => {
            assert_eq!(
                target,
                LValue::Element {
                    name: "a".to_string(),
                    subscript: n(2),
                    subscript_raw: "2".to_string(),
                }
            );
            assert_eq!(op, AssignOp::Set);
            assert_eq!(*rhs, ArithExpr::Num(9));
        }
        other => panic!("expected Assign, got {other:?}"),
    }
}

#[test]
fn eval_index_read_indexed_array() {
    let mut s = Shell::new();
    s.set_indexed_element("arr", 0, "10".to_string()).unwrap();
    s.set_indexed_element("arr", 1, "20".to_string()).unwrap();
    assert_eq!(eval_str("arr[0] + arr[1]", &mut s).unwrap(), 30);
}

#[test]
fn eval_index_unset_element_is_zero() {
    let mut s = Shell::new();
    assert_eq!(eval_str("nope[3] + 5", &mut s).unwrap(), 5);
}

#[test]
fn eval_index_compound_assign_indexed() {
    let mut s = Shell::new();
    s.set_indexed_element("a", 0, "10".to_string()).unwrap();
    s.set_indexed_element("a", 1, "20".to_string()).unwrap();
    assert_eq!(eval_str("a[0] += a[1]", &mut s).unwrap(), 30);
    assert_eq!(s.lookup_indexed_element("a", 0), Some("30".to_string()));
}

#[test]
fn eval_index_post_inc_element() {
    let mut s = Shell::new();
    s.set_indexed_element("a", 1, "2".to_string()).unwrap();
    assert_eq!(eval_str("a[1]++", &mut s).unwrap(), 2);
    assert_eq!(s.lookup_indexed_element("a", 1), Some("3".to_string()));
}

#[test]
fn eval_post_inc_returns_old_value() {
    let mut s = Shell::new();
    s.set("a", "5".to_string());
    assert_eq!(eval_str("a++", &mut s).unwrap(), 5);
    assert_eq!(s.lookup_var("a"), Some("6".to_string()));
}

#[test]
fn arith_error_bash_message_mapping() {
    use ArithErrorKind::*;
    let mk = |k| ArithError {
        kind: k,
        offset: None,
        token_end: None,
    };
    assert_eq!(
        mk(AssignToNonVar).bash_message(),
        "attempted assignment to non-variable"
    );
    assert_eq!(mk(DivisionByZero).bash_message(), "division by 0");
    assert_eq!(mk(InvalidBase).bash_message(), "invalid arithmetic base");
    assert_eq!(
        mk(InvalidIntegerConstant).bash_message(),
        "invalid integer constant"
    );
    assert_eq!(
        mk(ValueTooGreatForBase).bash_message(),
        "value too great for base"
    );
    assert_eq!(mk(MissingCloseParen).bash_message(), "missing `)'");
    assert_eq!(
        mk(OperandExpected).bash_message(),
        "syntax error: operand expected"
    );
    assert_eq!(mk(ExpressionExpected).bash_message(), "expression expected");
    assert_eq!(
        mk(ColonExpected).bash_message(),
        "`:' expected for conditional expression"
    );
    assert_eq!(
        mk(SyntaxErrorInExpression).bash_message(),
        "syntax error in expression"
    );
    assert_eq!(mk(InvalidNumber).bash_message(), "invalid number");
}

// ── Task 3: token offsets + render_error_body ──────────────────────────

#[test]
fn tokenize_reports_offsets() {
    let (toks, offs) = tokenize("7 = 43 ").unwrap();
    assert_eq!(toks.len(), offs.len());
    // tokens: 7@0, =@2, 43@4
    assert_eq!(offs, vec![0, 2, 4]);
}

#[test]
fn render_assign_to_nonvar() {
    // `$(( 7 = 43 ))` inner text, untrimmed
    let err = parse(" 7 = 43 ").unwrap_err();
    assert_eq!(err.bash_message(), "attempted assignment to non-variable");
    assert_eq!(
        render_error_body(" 7 = 43 ", &err),
        "7 = 43 : attempted assignment to non-variable (error token is \"= 43 \")"
    );
}

#[test]
fn render_operand_expected_at_eof() {
    let err = parse(" 4 + ").unwrap_err();
    assert_eq!(
        render_error_body(" 4 + ", &err),
        "4 + : syntax error: operand expected (error token is \"+ \")"
    );
}

#[test]
fn render_missing_close_paren() {
    let err = parse("rv = 7 + (43 * 6").unwrap_err();
    assert_eq!(
        render_error_body("rv = 7 + (43 * 6", &err),
        "rv = 7 + (43 * 6: missing `)' (error token is \"6\")"
    );
}

#[test]
fn render_trailing_junk() {
    let err = parse("a b").unwrap_err();
    assert_eq!(
        render_error_body("a b", &err),
        "a b: syntax error in expression (error token is \"b\")"
    );
}

#[test]
fn render_invalid_base_and_constants() {
    assert_eq!(
        render_error_body("3425#56", &parse("3425#56").unwrap_err()),
        "3425#56: invalid arithmetic base (error token is \"3425#56\")"
    );
    assert_eq!(
        render_error_body("2#", &parse("2#").unwrap_err()),
        "2#: invalid integer constant (error token is \"2#\")"
    );
    assert_eq!(
        render_error_body("2#44", &parse("2#44").unwrap_err()),
        "2#44: value too great for base (error token is \"2#44\")"
    );
}

#[test]
fn render_number_error_token_truncated_at_run_end() {
    // bash reports ONLY the full number run for tokenize-time number
    // errors — no trailing content, even with surrounding whitespace.
    assert_eq!(
        render_error_body(" 2#44 ", &parse(" 2#44 ").unwrap_err()),
        "2#44: value too great for base (error token is \"2#44\")"
    );
    assert_eq!(
        render_error_body(" 3425#56 ", &parse(" 3425#56 ").unwrap_err()),
        "3425#56: invalid arithmetic base (error token is \"3425#56\")"
    );
    assert_eq!(
        render_error_body(" 2# ", &parse(" 2# ").unwrap_err()),
        "2#: invalid integer constant (error token is \"2#\")"
    );
}

#[test]
fn render_number_error_token_truncated_with_trailing_expr() {
    // Run end stops at the first non-run byte; the echo is truncated there.
    assert_eq!(
        render_error_body("2#44 + 1", &parse("2#44 + 1").unwrap_err()),
        "2#44: value too great for base (error token is \"2#44\")"
    );
    assert_eq!(
        render_error_body("1 + 2#44", &parse("1 + 2#44").unwrap_err()),
        "1 + 2#44: value too great for base (error token is \"2#44\")"
    );
}

#[test]
fn render_parse_error_token_still_runs_to_end_of_source() {
    // Regression: parse errors (no token_end) keep trailing content,
    // including the trailing space.
    let body = render_error_body(" 7 = 43 ", &parse(" 7 = 43 ").unwrap_err());
    assert!(body.ends_with("(error token is \"= 43 \")"), "got {body}");
}

#[test]
fn render_ternary_branches() {
    assert_eq!(
        render_error_body("4 ? : 3 + 5", &parse("4 ? : 3 + 5").unwrap_err()),
        "4 ? : 3 + 5: expression expected (error token is \": 3 + 5\")"
    );
}
