use super::*;

// ── read_record ─────────────────────────────────────────────

#[test]
fn read_record_stops_at_delim() {
    let mut c = std::io::Cursor::new(b"abc\ndef".to_vec());
    let cfg = ReadCfg {
        raw: false,
        delim: b'\n',
        delim_active: true,
        max_chars: None,
        deadline: None,
    };
    let (s, stop, any) = read_record(&mut c, &cfg, None).unwrap();
    assert_eq!(s, "abc");
    assert!(matches!(stop, ReadStop::Delim));
    assert!(any);
}

#[test]
fn read_record_eof_partial_reports_eof() {
    let mut c = std::io::Cursor::new(b"abc".to_vec());
    let cfg = ReadCfg {
        raw: false,
        delim: b'\n',
        delim_active: true,
        max_chars: None,
        deadline: None,
    };
    let (s, stop, any) = read_record(&mut c, &cfg, None).unwrap();
    assert_eq!(s, "abc");
    assert!(matches!(stop, ReadStop::Eof));
    assert!(any);
}

#[test]
fn read_record_eof_empty_reports_not_any() {
    let mut c = std::io::Cursor::new(Vec::<u8>::new());
    let cfg = ReadCfg {
        raw: false,
        delim: b'\n',
        delim_active: true,
        max_chars: None,
        deadline: None,
    };
    let (s, stop, any) = read_record(&mut c, &cfg, None).unwrap();
    assert_eq!(s, "");
    assert!(matches!(stop, ReadStop::Eof));
    assert!(!any);
}

#[test]
fn read_record_backslash_continuation_and_escape() {
    // "a\<newline>b\c" -> line continuation joins, \c -> c
    let mut c = std::io::Cursor::new(b"a\\\nb\\c\n".to_vec());
    let cfg = ReadCfg {
        raw: false,
        delim: b'\n',
        delim_active: true,
        max_chars: None,
        deadline: None,
    };
    let (s, stop, _) = read_record(&mut c, &cfg, None).unwrap();
    assert_eq!(s, "abc");
    assert!(matches!(stop, ReadStop::Delim));
}

#[test]
fn read_record_raw_keeps_backslash() {
    let mut c = std::io::Cursor::new(b"a\\c\n".to_vec());
    let cfg = ReadCfg {
        raw: true,
        delim: b'\n',
        delim_active: true,
        max_chars: None,
        deadline: None,
    };
    let (s, _, _) = read_record(&mut c, &cfg, None).unwrap();
    assert_eq!(s, "a\\c");
}

#[test]
fn read_record_custom_delim() {
    let mut c = std::io::Cursor::new(b"foo:bar\n".to_vec());
    let cfg = ReadCfg {
        raw: false,
        delim: b':',
        delim_active: true,
        max_chars: None,
        deadline: None,
    };
    let (s, stop, _) = read_record(&mut c, &cfg, None).unwrap();
    assert_eq!(s, "foo");
    assert!(matches!(stop, ReadStop::Delim));
}

#[test]
fn read_record_nul_delim() {
    let mut c = std::io::Cursor::new(b"foo\0bar".to_vec());
    let cfg = ReadCfg {
        raw: false,
        delim: 0u8,
        delim_active: true,
        max_chars: None,
        deadline: None,
    };
    let (s, stop, _) = read_record(&mut c, &cfg, None).unwrap();
    assert_eq!(s, "foo");
    assert!(matches!(stop, ReadStop::Delim));
}

// ── read -u FD ──────────────────────────────────────────────

fn run_read(args: &[&str], shell: &mut crate::shell_state::Shell) -> (i32, String) {
    let argv: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let mut out: Vec<u8> = Vec::new();
    let mut err: Vec<u8> = Vec::new();
    let rc = match builtin_read(&argv, &mut out, &mut err, shell) {
        ExecOutcome::Continue(c) => c,
        other => panic!("unexpected outcome: {other:?}"),
    };
    (rc, String::from_utf8_lossy(&err).into_owned())
}

#[test]
fn read_u_nonnumeric_fd_is_spec_error() {
    let mut shell = crate::shell_state::Shell::new();
    let (rc, err) = run_read(&["-u", "xyz", "v"], &mut shell);
    assert_eq!(rc, 1);
    assert!(
        err.contains("xyz: invalid file descriptor specification"),
        "got: {err}"
    );
}

#[test]
fn read_u_separate_and_bundled_read_from_pipe() {
    // Create a real OS pipe, write a line into it, and read it back via
    // `read -u FD` in both the separate (`-u N`) and bundled (`-uN`) forms.
    for bundled in [false, true] {
        let mut fds = [0 as std::os::unix::io::RawFd; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let (rfd, wfd) = (fds[0], fds[1]);
        let payload = b"hello world extra\n";
        let n = unsafe { libc::write(wfd, payload.as_ptr() as *const libc::c_void, payload.len()) };
        assert_eq!(n, payload.len() as isize);
        unsafe { libc::close(wfd) };

        let mut shell = crate::shell_state::Shell::new();
        let fd_str = rfd.to_string();
        let (rc, err) = if bundled {
            run_read(&[&format!("-u{fd_str}"), "a", "b"], &mut shell)
        } else {
            run_read(&["-u", &fd_str, "a", "b"], &mut shell)
        };
        unsafe { libc::close(rfd) };
        assert_eq!(rc, 0, "err: {err}");
        assert_eq!(shell.get("a"), Some("hello"));
        // Remaining fields collapse into the last name (IFS join preserved).
        assert_eq!(shell.get("b"), Some("world extra"));
    }
}

#[test]
fn read_u_unopened_fd_is_bad_file_descriptor() {
    // Pick an fd that is (almost certainly) not open, verify up-front.
    let fd: std::os::unix::io::RawFd = 90;
    assert_eq!(unsafe { libc::fcntl(fd, libc::F_GETFD) }, -1);
    let mut shell = crate::shell_state::Shell::new();
    let (rc, err) = run_read(&["-u", "90", "v"], &mut shell);
    assert_eq!(rc, 1);
    assert!(
        err.contains("90: invalid file descriptor: Bad file descriptor"),
        "got: {err}"
    );
}

// ── read_one_record ────────────────────────────────────────

#[test]
fn read_one_record_newline_delim() {
    let mut r = std::io::Cursor::new(b"a\nb\n".to_vec());
    assert_eq!(
        read_one_record(&mut r, b'\n').unwrap(),
        Some(("a".to_string(), true))
    );
    assert_eq!(
        read_one_record(&mut r, b'\n').unwrap(),
        Some(("b".to_string(), true))
    );
    assert_eq!(read_one_record(&mut r, b'\n').unwrap(), None);
}

#[test]
fn read_one_record_unterminated_last() {
    let mut r = std::io::Cursor::new(b"a\nb".to_vec());
    assert_eq!(
        read_one_record(&mut r, b'\n').unwrap(),
        Some(("a".to_string(), true))
    );
    assert_eq!(
        read_one_record(&mut r, b'\n').unwrap(),
        Some(("b".to_string(), false))
    );
    assert_eq!(read_one_record(&mut r, b'\n').unwrap(), None);
}

#[test]
fn read_one_record_custom_delim_keeps_other_bytes() {
    let mut r = std::io::Cursor::new(b"a:b:c\n".to_vec());
    assert_eq!(
        read_one_record(&mut r, b':').unwrap(),
        Some(("a".to_string(), true))
    );
    assert_eq!(
        read_one_record(&mut r, b':').unwrap(),
        Some(("b".to_string(), true))
    );
    assert_eq!(
        read_one_record(&mut r, b':').unwrap(),
        Some(("c\n".to_string(), false))
    );
    assert_eq!(read_one_record(&mut r, b':').unwrap(), None);
}

// ── split_into_names ───────────────────────────────────────

#[test]
fn split_into_names_single_name_strip_ws() {
    let names = vec!["X".to_string()];
    let r = split_into_names("  hi  ", &names, " \t\n");
    assert_eq!(r, vec![("X".to_string(), "hi".to_string())]);
}

#[test]
fn split_into_names_multi_simple() {
    let names = vec!["X".to_string(), "Y".to_string(), "Z".to_string()];
    let r = split_into_names("a b c d", &names, " \t\n");
    assert_eq!(
        r,
        vec![
            ("X".to_string(), "a".to_string()),
            ("Y".to_string(), "b".to_string()),
            ("Z".to_string(), "c d".to_string()),
        ]
    );
}

#[test]
fn split_into_names_more_names_than_fields() {
    let names = vec!["X".to_string(), "Y".to_string(), "Z".to_string()];
    let r = split_into_names("a b", &names, " \t\n");
    assert_eq!(
        r,
        vec![
            ("X".to_string(), "a".to_string()),
            ("Y".to_string(), "b".to_string()),
            ("Z".to_string(), String::new()),
        ]
    );
}

#[test]
fn split_into_names_custom_ifs_colon() {
    let names = vec!["X".to_string(), "Y".to_string()];
    let r = split_into_names("a:b:c", &names, ":");
    assert_eq!(
        r,
        vec![
            ("X".to_string(), "a".to_string()),
            ("Y".to_string(), "b:c".to_string()),
        ]
    );
}

#[test]
fn split_last_field_readdef_rule() {
    // v350: huck ports bash's read.def last-field splitter. The last variable
    // gets the raw remainder with only trailing IFS-WHITESPACE stripped —
    // EXCEPT when the remainder is exactly one word followed by a single
    // delimiter run consuming to end of line, in which case that sole trailing
    // delimiter is dropped (bash's `get_word_from_string` behavior). Every row
    // below is verified against bash 5.2.21.
    let vals = |s: &str, names: &[&str], ifs: &str| {
        let n = names.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        split_into_names(s, &n, ifs)
            .into_iter()
            .map(|(_, v)| v)
            .collect::<Vec<_>>()
    };

    // Colon (non-ws) IFS, two vars.
    assert_eq!(vals("a:b:", &["x", "y"], ":"), vec!["a", "b"]); // sole trailing dropped
    assert_eq!(vals("a:b::", &["x", "y"], ":"), vec!["a", "b::"]); // double trailing kept
    assert_eq!(vals("a:b:c:", &["x", "y"], ":"), vec!["a", "b:c:"]); // after interior kept
    assert_eq!(vals("a::", &["x", "y"], ":"), vec!["a", ""]); // empty last, delim dropped
    assert_eq!(vals("a:::", &["x", "y"], ":"), vec!["a", "::"]); // extra trailing kept
    assert_eq!(vals(":::", &["x", "y"], ":"), vec!["", "::"]);

    // Mixed IFS (colon + space).
    assert_eq!(vals("a:b: ", &["x", "y"], ": "), vec!["a", "b"]); // trailing ws stripped too
    assert_eq!(vals("a: :b", &["x", "y"], ": "), vec!["a", ":b"]);
    assert_eq!(vals("a: :b", &["x", "y"], ":"), vec!["a", " :b"]); // space NOT in IFS
    assert_eq!(vals("::", &["x", "y"], ": "), vec!["", ""]);
    assert_eq!(vals(":a:", &["x", "y"], ": "), vec!["", "a"]);

    // Three vars.
    assert_eq!(vals(":a:b:", &["x", "y", "z"], ":"), vec!["", "a", "b"]);
    assert_eq!(vals("a:b::", &["x", "y", "z"], ":"), vec!["a", "b", ""]);
    assert_eq!(vals("a:::", &["x", "y", "z"], ":"), vec!["a", "", ""]);

    // Leading-ws IFS: interior ws-run preserved in the last field.
    assert_eq!(vals("a  b  c", &["x", "y"], " :"), vec!["a", "b  c"]);

    // Default ws-IFS: trailing whitespace IS trimmed from the last field.
    assert_eq!(vals("a b  ", &["x", "y"], " \t\n"), vec!["a", "b"]);
}

#[test]
fn split_read_fields_default_ws() {
    assert_eq!(split_read_fields("a b c", " \t\n"), vec!["a", "b", "c"]);
    assert_eq!(split_read_fields("  x   y  ", " \t\n"), vec!["x", "y"]); // trim + collapse
    assert_eq!(split_read_fields("", " \t\n"), Vec::<String>::new()); // empty -> none
}

#[test]
fn split_read_fields_nonws_ifs() {
    assert_eq!(split_read_fields("a:b:c", ":"), vec!["a", "b", "c"]);
    assert_eq!(split_read_fields("x:y:", ":"), vec!["x", "y"]); // trailing delim: NO empty
    assert_eq!(split_read_fields(":x", ":"), vec!["", "x"]); // leading delim: empty first
    assert_eq!(split_read_fields("x::y", ":"), vec!["x", "", "y"]); // adjacent: empty between
}

#[test]
fn split_read_fields_mixed_and_empty_ifs() {
    assert_eq!(split_read_fields("x : y", " :"), vec!["x", "y"]); // ws around nonws collapses
    assert_eq!(split_read_fields("a b c", ""), vec!["a b c"]); // empty IFS -> one field
    assert_eq!(split_read_fields("", ""), Vec::<String>::new()); // empty IFS + empty -> none
}
