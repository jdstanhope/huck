use super::*;

// ── escape decoder ─────────────────────────────────────────

#[test]
fn escape_basic() {
    assert_eq!(decode_printf_escape(b"n"), (b"\n".to_vec(), 1));
    assert_eq!(decode_printf_escape(b"t"), (b"\t".to_vec(), 1));
    assert_eq!(decode_printf_escape(b"\\"), (b"\\".to_vec(), 1));
}

#[test]
fn escape_octal() {
    // \101 → 'A'
    assert_eq!(decode_printf_escape(b"101"), (b"A".to_vec(), 3));
    // \0101 → still 'A' (\0 prefix allows up to 4 digits)
    let (v, n) = decode_printf_escape(b"0101");
    assert_eq!(v, b"A".to_vec());
    assert_eq!(n, 4);
}

#[test]
fn escape_hex() {
    // \x41 → 'A'
    assert_eq!(decode_printf_escape(b"x41"), (b"A".to_vec(), 3));
    // \x4 → byte 0x04 (one hex digit consumed)
    let (v, n) = decode_printf_escape(b"x4");
    assert_eq!(v, vec![0x04]);
    assert_eq!(n, 2);
}

#[test]
fn escape_unknown_preserved() {
    // \z → literal "\\z"
    assert_eq!(decode_printf_escape(b"z"), (b"\\z".to_vec(), 1));
}

#[test]
fn escape_trailing_backslash() {
    // Empty rest after `\` → literal "\\"
    assert_eq!(decode_printf_escape(b""), (b"\\".to_vec(), 0));
}

// ── parse_printf_int ───────────────────────────────────────

#[test]
fn parse_printf_int_decimal() {
    let (v, e) = parse_printf_int("42");
    assert_eq!(v, 42);
    assert!(e.is_none());
}

#[test]
fn parse_printf_int_negative_hex_octal() {
    assert_eq!(parse_printf_int("-42").0, -42);
    assert_eq!(parse_printf_int("0x1F").0, 31);
    assert_eq!(parse_printf_int("017").0, 15);
}

#[test]
fn parse_printf_int_char_literal() {
    assert_eq!(parse_printf_int("'A").0, 65);
    assert_eq!(parse_printf_int("\"A").0, 65);
}

#[test]
fn parse_printf_int_trailing_garbage() {
    let (v, e) = parse_printf_int("42abc");
    assert_eq!(v, 42);
    assert!(e.is_some(), "expected error message");
}

// ── parse_format ───────────────────────────────────────────

#[test]
fn parse_format_literal_only() {
    let p = parse_format("hello\\n").unwrap();
    assert_eq!(p.len(), 1);
    match &p[0] {
        FormatPart::Literal(b) => assert_eq!(b, b"hello\n"),
        _ => panic!(),
    }
}

#[test]
fn parse_format_simple_conv() {
    let p = parse_format("%s").unwrap();
    assert_eq!(p.len(), 1);
    match &p[0] {
        FormatPart::Conv(c) => {
            assert_eq!(c.conv, ConvChar::S);
            assert_eq!(c.width, None);
            assert_eq!(c.precision, None);
            assert_eq!(c.flags, ConvFlags::default());
        }
        _ => panic!(),
    }
}

#[test]
fn parse_format_flags_width_prec() {
    let p = parse_format("%-5.3d").unwrap();
    assert_eq!(p.len(), 1);
    match &p[0] {
        FormatPart::Conv(c) => {
            assert!(c.flags.left_align);
            assert_eq!(c.width, Some(5));
            assert_eq!(c.precision, Some(3));
            assert_eq!(c.conv, ConvChar::D);
        }
        _ => panic!(),
    }
}

// ── format_one ─────────────────────────────────────────────

#[test]
fn format_s_basic() {
    let mut out = Vec::new();
    let spec = ConvSpec {
        flags: ConvFlags::default(),
        width: None,
        precision: None,
        width_star: false,
        prec_star: false,
        conv: ConvChar::S,
    };
    format_one(&spec, "hi", &mut out).unwrap();
    assert_eq!(out, b"hi");
}

#[test]
fn format_s_width() {
    let mut out = Vec::new();
    let spec = ConvSpec {
        flags: ConvFlags::default(),
        width: Some(5),
        precision: None,
        width_star: false,
        prec_star: false,
        conv: ConvChar::S,
    };
    format_one(&spec, "hi", &mut out).unwrap();
    assert_eq!(out, b"   hi");
}

#[test]
fn format_s_left_align() {
    let mut out = Vec::new();
    let spec = ConvSpec {
        flags: ConvFlags {
            left_align: true,
            ..ConvFlags::default()
        },
        width: Some(5),
        precision: None,
        width_star: false,
        prec_star: false,
        conv: ConvChar::S,
    };
    format_one(&spec, "hi", &mut out).unwrap();
    assert_eq!(out, b"hi   ");
}

#[test]
fn format_s_precision_truncates() {
    let mut out = Vec::new();
    let spec = ConvSpec {
        flags: ConvFlags::default(),
        width: None,
        precision: Some(3),
        width_star: false,
        prec_star: false,
        conv: ConvChar::S,
    };
    format_one(&spec, "hello", &mut out).unwrap();
    assert_eq!(out, b"hel");
}

#[test]
fn format_d_basic() {
    let mut out = Vec::new();
    let spec = ConvSpec {
        flags: ConvFlags::default(),
        width: None,
        precision: None,
        width_star: false,
        prec_star: false,
        conv: ConvChar::D,
    };
    format_one(&spec, "42", &mut out).unwrap();
    assert_eq!(out, b"42");
}

#[test]
fn format_d_zero_pad() {
    let mut out = Vec::new();
    let spec = ConvSpec {
        flags: ConvFlags {
            zero_pad: true,
            ..ConvFlags::default()
        },
        width: Some(5),
        precision: None,
        width_star: false,
        prec_star: false,
        conv: ConvChar::D,
    };
    format_one(&spec, "42", &mut out).unwrap();
    assert_eq!(out, b"00042");
}

#[test]
fn format_x_alt_form() {
    let mut out = Vec::new();
    let spec_x = ConvSpec {
        flags: ConvFlags {
            alt: true,
            ..ConvFlags::default()
        },
        width: None,
        precision: None,
        width_star: false,
        prec_star: false,
        conv: ConvChar::X,
    };
    format_one(&spec_x, "255", &mut out).unwrap();
    assert_eq!(out, b"0xff");

    let mut out2 = Vec::new();
    let spec_bigx = ConvSpec {
        flags: ConvFlags {
            alt: true,
            ..ConvFlags::default()
        },
        width: None,
        precision: None,
        width_star: false,
        prec_star: false,
        conv: ConvChar::BigX,
    };
    format_one(&spec_bigx, "255", &mut out2).unwrap();
    assert_eq!(out2, b"0XFF");
}

#[test]
fn format_b_arg_escapes() {
    let mut out = Vec::new();
    let spec = ConvSpec {
        flags: ConvFlags::default(),
        width: None,
        precision: None,
        width_star: false,
        prec_star: false,
        conv: ConvChar::B,
    };
    format_one(&spec, "a\\tb", &mut out).unwrap();
    assert_eq!(out, b"a\tb");
}

#[test]
fn format_d_precision_zero_with_value_zero_emits_empty() {
    // POSIX: precision 0 + value 0 produces no digits.
    // Regression for `%.0d` of 0 returning "0" instead of "".
    let mut out = Vec::new();
    let spec = ConvSpec {
        flags: ConvFlags::default(),
        width: None,
        precision: Some(0),
        width_star: false,
        prec_star: false,
        conv: ConvChar::D,
    };
    format_one(&spec, "0", &mut out).unwrap();
    assert_eq!(out, b"");

    // Sanity: precision 0 with NON-zero value still produces digits.
    let mut out2 = Vec::new();
    format_one(&spec, "5", &mut out2).unwrap();
    assert_eq!(out2, b"5");
}

#[test]
fn format_float_via_snprintf() {
    let mut out = Vec::new();
    let spec = ConvSpec {
        flags: ConvFlags::default(),
        width: Some(5),
        precision: Some(2),
        width_star: false,
        prec_star: false,
        conv: ConvChar::Float(b'f'),
    };
    format_one(&spec, "3.14159", &mut out).unwrap();
    assert_eq!(out, b" 3.14");
}

#[test]
fn format_float_invalid_arg_reports_err() {
    let mut out = Vec::new();
    let spec = ConvSpec {
        flags: ConvFlags::default(),
        width: None,
        precision: None,
        width_star: false,
        prec_star: false,
        conv: ConvChar::Float(b'f'),
    };
    // Non-numeric arg → 0.000000 plus an error (caller sets rc 1).
    let err = format_one(&spec, "abc", &mut out).unwrap_err();
    assert!(err.contains("invalid number"));
    assert_eq!(out, b"0.000000");
}

#[test]
fn parse_format_accepts_star_and_floats() {
    // `*` width/precision flagged; float convs parsed.
    let parts = parse_format("%*.*f").unwrap();
    match &parts[0] {
        FormatPart::Conv(c) => {
            assert!(c.width_star);
            assert!(c.prec_star);
            assert_eq!(c.conv, ConvChar::Float(b'f'));
        }
        _ => panic!("expected a conv part"),
    }
}
