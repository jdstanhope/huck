# huck v55 — `read` builtin (M-72)

## Goal

Add bash's `read` builtin so scripts can read a line of input from
stdin into one or more variables. POSIX core plus a few high-value
bash flags.

After v55:

- `read NAME` — read a line, assign to NAME (trailing newline
  stripped).
- `read NAME1 NAME2` — read a line, IFS-split, assign fields
  left-to-right; the LAST name gets the remaining unsplit tail.
- `read` (no name) — assign the line to `REPLY`.
- `read -r NAME` — raw mode: backslash is literal; no
  line-continuation; no escape removal.
- `read -p PROMPT NAME` — write PROMPT to stderr first (only if
  stdin is a tty; matches bash).
- `read -s NAME` — silent: while reading from a tty, disable
  ECHO via termios.
- `read -d DELIM NAME` — read until first character of DELIM
  instead of `\n`. Empty DELIM (`-d ''`) → NUL byte.
- Combinations: `read -rsp 'Password: ' PW`, `read -rd '' VAR`,
  etc.
- Exit: 0 on success; 1 on EOF before any input was read.
- Readonly-honoring: writing to a readonly NAME fires the v54
  readonly diagnostic and returns status 1.

New tracked divergence: **M-72: `read`**.

## Scope decisions (locked via AskUserQuestion)

1. **POSIX core + `-r` / `-p` / `-s` / `-d`**.
2. **Deferred**: `-n N` / `-N N` (char counts — need raw-mode tty
   handling); `-t TIMEOUT` (needs `poll`/`select`); `-u FD` (fd
   plumbing); `-a ARRAY` (huck has no arrays); `-e` / `-i`
   (readline editing for `read` is its own subsystem).

## Out of scope (deferred)

- `-n` / `-N` (read N characters; tty-raw-mode complexity).
- `-t` (timeout; needs poll/select).
- `-u` (alternate fd for input).
- `-a` (array).
- `-e` / `-i` (readline).
- `IFS` containing non-default chars: works for any single-byte
  IFS, but multi-byte IFS chars (rare) are not in scope.

## Architecture

Three plumbing components in `src/builtins.rs`:

### 1. `read_one_line` helper

Reads a single logical line from a `BufRead`, honoring `raw` and
the terminator byte. Returns `Ok(Some(String))` on a successful
read, `Ok(None)` on EOF before any byte was read, `Err` on I/O
error.

```rust
fn read_one_line<R: std::io::BufRead>(
    r: &mut R,
    raw: bool,
    delim: u8,
) -> std::io::Result<Option<String>> {
    let mut out = Vec::<u8>::new();
    let mut any_byte_read = false;
    loop {
        let mut byte = [0u8; 1];
        let n = r.read(&mut byte)?;
        if n == 0 {
            // EOF
            if !any_byte_read {
                return Ok(None);
            }
            break;
        }
        any_byte_read = true;
        let b = byte[0];
        if b == delim {
            // End of record (do not include delim).
            break;
        }
        if !raw && b == b'\\' {
            // Read the next byte.
            let mut nxt = [0u8; 1];
            let m = r.read(&mut nxt)?;
            if m == 0 {
                // Trailing backslash at EOF — keep it literal.
                out.push(b'\\');
                break;
            }
            any_byte_read = true;
            if nxt[0] == b'\n' {
                // Line continuation: discard backslash + newline.
                continue;
            }
            // Escape removal: \X → X.
            out.push(nxt[0]);
            continue;
        }
        out.push(b);
    }
    // Lossy UTF-8 → String (bash treats input as bytes; huck stores
    // String. Invalid sequences become U+FFFD.)
    Ok(Some(String::from_utf8_lossy(&out).into_owned()))
}
```

Notes:
- For `-r` (raw=true): no escape processing, no line continuation;
  the backslash is just a byte.
- The delim byte is consumed but not appended to the result.
- EOF with NO bytes read → `Ok(None)` → `read` exits 1.
- EOF with bytes read → `Ok(Some(partial))` → `read` exits 0 and
  assigns what was collected.

### 2. `split_into_names` helper

POSIX field-splitting honoring IFS.

```rust
fn split_into_names(line: &str, names: &[String], ifs: &str) -> Vec<(String, String)> {
    // Returns Vec<(name, assigned_value)>.
    // If `names.is_empty()`, caller assigns to REPLY (handles
    // whitespace-strip).
    // Implementation outline:
    // - whitespace = chars in IFS that are ' ', '\t', or '\n'.
    // - non_ws    = chars in IFS that are NOT whitespace.
    // - If IFS is entirely whitespace (the common default case),
    //   collapse runs and strip leading/trailing whitespace.
    // - Otherwise, treat any whitespace as field separators AND
    //   each non-whitespace IFS char as an explicit single
    //   field-separator (no run collapse, no leading strip).
    //
    // Assign first names.len()-1 fields one-to-one; LAST name gets
    // the remainder of the line starting at the field boundary
    // AFTER the (N-1)th delimiter, with TRAILING IFS whitespace
    // stripped (matches bash).
    // ...
}
```

Detailed algorithm in plan step 1.5.

### 3. `builtin_read`

```rust
fn builtin_read(
    args: &[String],
    _out: &mut dyn std::io::Write,
    shell: &mut Shell,
) -> ExecOutcome {
    // Parse flags. -r/-s flags can cluster: -rs, -sr, etc.
    // -p and -d each take a value: -p PROMPT, -p"PROMPT", -d DELIM.
    let mut raw = false;
    let mut silent = false;
    let mut prompt: Option<String> = None;
    let mut delim: u8 = b'\n';
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" { i += 1; break; }
        if !arg.starts_with('-') || arg.len() < 2 {
            break;
        }
        // Walk the cluster.
        let bytes = arg.as_bytes();
        let mut j = 1;
        while j < bytes.len() {
            match bytes[j] {
                b'r' => raw = true,
                b's' => silent = true,
                b'p' => {
                    // -p PROMPT — value is rest-of-arg OR next arg.
                    if j + 1 < bytes.len() {
                        prompt = Some(String::from_utf8_lossy(&bytes[j+1..]).into_owned());
                        j = bytes.len();
                    } else {
                        i += 1;
                        if i >= args.len() {
                            eprintln!("huck: read: -p: option requires an argument");
                            return ExecOutcome::Continue(2);
                        }
                        prompt = Some(args[i].clone());
                    }
                    break;
                }
                b'd' => {
                    let d_val: &str;
                    let owned: String;
                    if j + 1 < bytes.len() {
                        owned = String::from_utf8_lossy(&bytes[j+1..]).into_owned();
                        d_val = &owned;
                        j = bytes.len();
                    } else {
                        i += 1;
                        if i >= args.len() {
                            eprintln!("huck: read: -d: option requires an argument");
                            return ExecOutcome::Continue(2);
                        }
                        d_val = &args[i];
                    }
                    // Empty DELIM means NUL byte.
                    delim = d_val.bytes().next().unwrap_or(0u8);
                    break;
                }
                c => {
                    eprintln!("huck: read: -{}: invalid option", c as char);
                    return ExecOutcome::Continue(2);
                }
            }
            j += 1;
        }
        i += 1;
    }
    let names: Vec<String> = args[i..].to_vec();

    // Validate names early (POSIX says invalid name → status 1).
    for name in &names {
        if !is_valid_name(name) {
            eprintln!("huck: read: `{name}': not a valid identifier");
            return ExecOutcome::Continue(1);
        }
    }

    // Prompt — only when stdin is a tty (matches bash).
    if let Some(p) = &prompt {
        use std::io::IsTerminal;
        if std::io::stdin().is_terminal() {
            eprint!("{p}");
            let _ = std::io::Write::flush(&mut std::io::stderr());
        }
    }

    // -s silent: toggle ECHO off on stdin's tty for the duration of
    // the read, then restore.
    let saved_term = if silent {
        unsafe { silent_disable_echo() }
    } else {
        None
    };

    let stdin = std::io::stdin();
    let mut handle = stdin.lock();
    let line_opt = match read_one_line(&mut handle, raw, delim) {
        Ok(opt) => opt,
        Err(e) => {
            eprintln!("huck: read: {e}");
            if let Some(s) = saved_term { unsafe { silent_restore_echo(s); } }
            return ExecOutcome::Continue(1);
        }
    };

    if let Some(s) = saved_term { unsafe { silent_restore_echo(s); } }
    // If we suppressed echo, emit the newline the user expected but
    // never saw.
    if silent {
        eprintln!();
    }

    let line = match line_opt {
        Some(l) => l,
        None => return ExecOutcome::Continue(1),  // EOF, nothing read
    };

    // Assignment.
    let ifs = shell.lookup_var("IFS").unwrap_or_else(|| " \t\n".to_string());
    let assignments: Vec<(String, String)> = if names.is_empty() {
        vec![("REPLY".to_string(), line)]
    } else {
        split_into_names(&line, &names, &ifs)
    };

    let mut exit = 0;
    for (name, value) in assignments {
        if shell.try_set(&name, value).is_err() {
            eprintln!("huck: read: {name}: readonly variable");
            exit = 1;
        }
    }
    ExecOutcome::Continue(exit)
}
```

### 4. silent (termios) helpers

```rust
#[cfg(unix)]
unsafe fn silent_disable_echo() -> Option<libc::termios> {
    use std::os::unix::io::AsRawFd;
    let fd = std::io::stdin().as_raw_fd();
    if libc::isatty(fd) == 0 { return None; }
    let mut t: libc::termios = std::mem::zeroed();
    if libc::tcgetattr(fd, &mut t) != 0 { return None; }
    let saved = t;
    t.c_lflag &= !libc::ECHO;
    libc::tcsetattr(fd, libc::TCSANOW, &t);
    Some(saved)
}

#[cfg(unix)]
unsafe fn silent_restore_echo(saved: libc::termios) {
    use std::os::unix::io::AsRawFd;
    let fd = std::io::stdin().as_raw_fd();
    let _ = libc::tcsetattr(fd, libc::TCSANOW, &saved);
}
```

### 5. Dispatch + `BUILTIN_NAMES`

- `"read"` added to `BUILTIN_NAMES`. **NOT** added to
  `is_special_builtin` (POSIX classifies `read` as regular).
- `"read" => builtin_read(args, out, shell)` arm in `run_builtin`.

## POSIX field-splitting algorithm (the heart of multi-name `read`)

This needs care. Three cases:

### Case A: `read NAME` (single name)

Whitespace-strip the line (leading + trailing) IF IFS contains
only whitespace chars (the default). Otherwise leave the line as
is. Assign whole to NAME.

Wait — actually for single-name read, POSIX says: split the line
into fields per IFS, assign the JOIN of all those fields back to
NAME (preserving original separator chars? No — joined by the
first IFS char). Hmm, this is subtle.

Actually re-reading POSIX more carefully: `read` does NOT split if
there's only one name. It strips leading and trailing IFS
whitespace and assigns the result. (Re-splitting and re-joining
would lose info.)

Let me just match bash: for single name, strip leading +
trailing IFS-whitespace, assign whole line. Confirmed bash
behavior. Match.

### Case B: `read NAME1 NAME2 NAME3` (multiple names)

Walk the line left-to-right with a small state machine:

1. Skip any leading run of IFS-whitespace.
2. Take chars until we hit either a non-whitespace-IFS char OR an
   IFS-whitespace char. That run is one field.
3. If the separator was an IFS-whitespace, skip the whole run of
   IFS-whitespace and any single trailing non-whitespace-IFS.
4. Otherwise (non-whitespace-IFS): skip just that one char (no
   trailing whitespace skip).
5. After N-1 fields are consumed, the LAST name gets EVERYTHING
   FROM THE START OF FIELD N TO THE END OF THE LINE (no further
   splitting, no joining). Strip trailing IFS-whitespace from the
   last field. (This matches bash precisely.)

### Case C: `read` (no names)

Assign the WHOLE LINE verbatim (no IFS strip, no escape changes
besides what `read_one_line` already did) to `REPLY`.

This is bash + POSIX behavior. Confirmed by `bash -c 'read; echo
"[$REPLY]"' <<< '  hi  '` → `[  hi  ]` — leading/trailing
whitespace preserved.

## Behavior table

| Input script | Behavior |
|---|---|
| `read X <<< 'hello'` | X=hello, status 0 |
| `read X Y <<< 'a b c d'` | X=a, Y='b c d' |
| `read X Y Z <<< 'a b c'` | X=a, Y=b, Z=c |
| `read X Y Z <<< 'a b'` | X=a, Y=b, Z='' |
| `read X <<< '  a  b  '` (IFS default) | X='a  b' (leading/trailing stripped, internal preserved) |
| `read <<< 'hi'` | REPLY=hi |
| `read X` with EOF immediately | status 1, X unchanged |
| `read -r X <<< 'a\b'` | X='a\b' (raw, backslash literal) |
| `read X <<< 'a\b'` (non-raw) | X='ab' (escape removed) |
| `read X <<< 'a\` + next line `b'` (non-raw) | X='ab' (line continued) |
| `read -p 'P: ' X` | "P: " to stderr (only if stdin is a tty), then read X |
| `read -s X` | echo suppressed during read |
| `read -d ':' X <<< 'foo:bar'` | X=foo |
| `read -d '' X <<< $'foo\0bar'` | X=foo (delim is NUL) |
| `read READONLY_X <<< 'x'` (X is readonly) | status 1 + stderr "huck: read: X: readonly variable" |
| `read 1foo <<< 'x'` | status 1 + stderr "not a valid identifier" |
| `read -X` | status 2 + stderr "invalid option" |

## Test plan

### Unit tests in `src/builtins.rs::mod read_tests` (12 tests)

Tests directly exercise `read_one_line` and `split_into_names`
helpers (no stdin needed) plus a few end-to-end through a
`Cursor`-fed `BufRead`. The full `builtin_read` is harder to unit
test because it hardcodes `std::io::stdin()` — those scenarios
move to integration tests.

1. `read_one_line_basic` — `"hello\n"` → "hello".
2. `read_one_line_eof_returns_none` — `""` → None.
3. `read_one_line_eof_partial_returns_some` — `"abc"` → Some("abc").
4. `read_one_line_escape_removal` — `"a\\bc\n"` non-raw → "abc".
5. `read_one_line_line_continuation` — `"a\\\nb\n"` non-raw → "ab".
6. `read_one_line_raw_preserves_backslash` — `"a\\b\n"` raw → "a\\b".
7. `read_one_line_custom_delim` — `"foo:bar\n"` delim `:` → "foo".
8. `read_one_line_nul_delim` — `"foo\0bar"` delim 0 → "foo".
9. `split_into_names_single_name_strip_ws` — line `"  hi  "`, names `["X"]`, IFS default → `[("X", "hi")]` (or with leading/trailing stripped per single-name behavior).
10. `split_into_names_multi_simple` — line `"a b c d"`, names `["X","Y","Z"]`, IFS default → `[("X","a"), ("Y","b"), ("Z","c d")]`.
11. `split_into_names_more_names_than_fields` — line `"a b"`, names `["X","Y","Z"]` → `[("X","a"), ("Y","b"), ("Z","")]`.
12. `split_into_names_custom_ifs_colon` — line `"a:b:c"`, names `["X","Y"]`, IFS `:` → `[("X","a"), ("Y","b:c")]`.

### Integration tests in `tests/read_integration.rs` (8 tests)

1. `read_single_name_via_heredoc` — `read X <<< 'hello'; echo "[$X]"` → `[hello]`.
2. `read_multi_name_split` — `read X Y <<< 'a b c'; echo "[$X][$Y]"` → `[a][b c]`.
3. `read_with_reply_default` — `read <<< 'hi there'; echo "[$REPLY]"` → `[hi there]`.
4. `read_eof_returns_1` — `read X </dev/null; echo $?` → `1`.
5. `read_dash_r_preserves_backslash` — `read -r X <<< 'a\b'; echo "[$X]"` → `[a\b]`.
6. `read_dash_d_custom_delim` — `read -d ':' X <<< 'foo:bar'; echo "[$X]"` → `[foo]`.
7. `read_readonly_var_errors` — `readonly X=v; read X <<< 'new'; echo "[$X]"` → stderr "readonly", `[v]`, status 1.
8. `read_invalid_identifier_errors` — `read 1foo <<< 'x'; echo $?` → `1`, stderr "not a valid identifier".

### Smoke

`cargo test --all-targets` green (PTY flake tolerated).

## Implementation tasks

1. **Builtin + helpers + 12 unit tests** — touch `src/builtins.rs`
   only. Adds `read_one_line`, `split_into_names`, `builtin_read`,
   silent helpers (`silent_disable_echo` / `silent_restore_echo`),
   `"read"` to `BUILTIN_NAMES`, dispatch arm, `mod read_tests`
   with 12 unit tests.

2. **Integration tests** — create `tests/read_integration.rs`
   with the 8 scenarios above.

3. **Docs** — M-72 entry; change-log; README v55 row.

## Acceptance criteria

- 12 unit tests pass.
- 8 integration tests pass.
- `cargo test --all-targets` green (PTY flake tolerated).
- `cargo clippy --all-targets -- -D warnings` clean.
- `read` is regular (NOT in `is_special_builtin`).
- IFS-respecting field splitting works for default whitespace IFS
  AND for custom single-char IFS (e.g. `:`).
- Readonly enforcement works via `try_set`.
- Bare `read` (no names) writes to `REPLY` preserving
  leading/trailing whitespace.
- All flag clusters (`-rs`, `-rsd ':'`, `-rp 'PROMPT'`) parse
  correctly.
- `-s` only manipulates termios when stdin is a tty (no-op when
  reading from a pipe/heredoc).
- EOF before any byte → status 1 + no assignment. EOF mid-line
  → status 0 + assignment of partial line.
