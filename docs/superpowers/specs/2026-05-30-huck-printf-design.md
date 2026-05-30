# huck v56 — `printf` builtin (M-73)

## Goal

Add bash's `printf` builtin — POSIX core plus the most common
bash extensions:

- Conversions: `%s %d %i %u %o %x %X %c %% %b`
- Flags: `-` (left align), `+` (sign), ` ` (sign-space),
  `#` (alt form), `0` (zero pad)
- Width: `%5s`, `%-10d`
- Precision: `%.3s`, `%.5d`
- Format-cycling: when args remain, re-apply the format string
- Backslash escapes in format: `\\ \a \b \f \n \r \t \v \NNN \xHH`
- `%b` argument: same escape processing applied to the arg
- `\c` in `%b` arg: halt all further output
- `-v VAR` flag: capture output to VAR (readonly-honoring via
  v54's `try_set`)

New tracked divergence: **M-73: `printf`**.

## Scope decisions (locked via AskUserQuestion)

1. **POSIX + common bash** chosen over minimal and "everything
   but strftime".
2. **Deferred**: floats (`%f`/`%e`/`%g`/`%E`/`%G`) — huck has no
   float infrastructure elsewhere; `%q` (shell-quote);
   `%(FMT)T` (strftime); runtime `*` for width/precision
   (`%*d`); Unicode `\uHHHH`/`\UHHHHHHHH` escapes.

## Out of scope (deferred)

- All floating point.
- `%q` (shell-quote) — would reuse `escape_alias_value` but the
  POSIX field-quoting rules go beyond a single-quote wrap.
- `%(FMT)T` strftime — needs date/time formatting.
- `*` runtime width/precision.
- `\u`/`\U` Unicode escapes in format string and `%b` arg.
- Locale-sensitive output (treat input as bytes throughout).

## Architecture

All new code in `src/builtins.rs`. Three layers:

### 1. Format-string tokenizer (`Vec<FormatPart>`)

```rust
enum FormatPart {
    Literal(String),
    Conv(ConvSpec),
}

struct ConvSpec {
    flags: ConvFlags,        // -, +, ' ', #, 0
    width: Option<usize>,
    precision: Option<usize>,
    conv: ConvChar,          // 's','d','i','u','o','x','X','c','b','%'
}

struct ConvFlags {
    left_align: bool,      // -
    sign: bool,            // +
    space_sign: bool,      // ' '
    alt: bool,             // #
    zero_pad: bool,        // 0
}

enum ConvChar { S, D, I, U, O, X, BigX, C, B, Percent }
```

A parser function `parse_format(fmt: &str) -> Result<Vec<FormatPart>, String>`
walks the format string char-by-char:
- Plain character → append to current `Literal`.
- `\` → consume next, decode escape, append byte(s) to current
  `Literal`.
- `%` → start a `ConvSpec`. Parse flags, width, precision, then
  the conv char. Push `Conv(spec)`.

If the format string ends mid-`%`, that's an error (status 1).

### 2. Argument formatter

```rust
fn format_one(spec: &ConvSpec, arg: &str, sink: &mut Vec<u8>) -> Result<bool, String>
```

Returns `Ok(true)` normally; `Ok(false)` if a `\c` was hit in a
`%b` arg (caller stops output). `Err(msg)` for invalid integer
input (caller still produces output but the overall exit becomes
1).

Per-conv behavior:
- `%s`: arg as string. Width pads (left-pad by default,
  right-pad with `-` flag). Precision truncates: `%.3s` keeps
  first 3 chars.
- `%d` / `%i`: parse arg as integer (`parse_printf_int`).
  Width + sign flags apply. Precision = minimum digit count
  (zero-pad to precision; `%.5d` of 3 → `00003`).
- `%u`: same as %d but errors on negative input? Bash actually
  treats `%u` as printing the value modulo 2^64 if negative.
  Match bash: cast i64 → u64.
- `%o` / `%x` / `%X`: convert to base. `#` flag prepends `0`,
  `0x`, `0X` respectively. Negative as if unsigned (cast to u64).
- `%c`: first byte of arg (or `0` if empty? bash prints nothing
  if empty; match).
- `%%`: literal `%`. (No arg consumed.)
- `%b`: arg is processed through the same escape decoder as the
  format string; `\c` halts.

### 3. Integer parser `parse_printf_int(s: &str) -> (i64, Option<String>)`

Returns `(value, maybe_error_message)`. POSIX/bash rules:
- Strip leading whitespace.
- Optional leading `+` or `-`.
- If starts with `'` or `"`: rest is treated as the character
  whose value is its first byte (or 0 if empty).
- Hex: `0x...` / `0X...`.
- Octal: `0...` (leading zero).
- Decimal otherwise.
- Trailing garbage → error message (but value is what was
  parsed; bash treats this as a warning, not a hard error).
- Empty string → 0, no error.

### 4. Main builtin `builtin_printf`

```rust
fn builtin_printf(
    args: &[String],
    out: &mut dyn std::io::Write,
    shell: &mut Shell,
) -> ExecOutcome {
    // Parse leading flags: -v VAR, -- end-of-flags.
    let mut v_var: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-v" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("huck: printf: -v: option requires an argument");
                    return ExecOutcome::Continue(2);
                }
                if !is_valid_name(&args[i]) {
                    eprintln!("huck: printf: `{}': not a valid identifier", args[i]);
                    return ExecOutcome::Continue(1);
                }
                v_var = Some(args[i].clone());
                i += 1;
            }
            "--" => { i += 1; break; }
            s if s.starts_with('-') && s.len() > 1 && s != "-" => {
                // Bash's printf rejects unknown flags but accepts a
                // lone "-" as a format. We do the same.
                eprintln!("huck: printf: {s}: invalid option");
                return ExecOutcome::Continue(2);
            }
            _ => break,
        }
    }

    if i >= args.len() {
        eprintln!("huck: printf: usage: printf [-v var] format [arguments]");
        return ExecOutcome::Continue(2);
    }

    let format = args[i].clone();
    let rest_args: &[String] = &args[i + 1..];

    let parts = match parse_format(&format) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("huck: printf: {e}");
            return ExecOutcome::Continue(1);
        }
    };

    // Determine whether the format has any consuming conv (anything
    // that pops an arg from `rest_args`). %% does NOT consume.
    let has_consuming_conv = parts.iter().any(|p| match p {
        FormatPart::Conv(c) => !matches!(c.conv, ConvChar::Percent),
        _ => false,
    });

    let mut buf: Vec<u8> = Vec::new();
    let mut exit: i32 = 0;
    let mut arg_idx = 0;
    let mut halted = false;

    loop {
        for part in &parts {
            if halted { break; }
            match part {
                FormatPart::Literal(s) => buf.extend_from_slice(s.as_bytes()),
                FormatPart::Conv(c) if matches!(c.conv, ConvChar::Percent) => {
                    buf.push(b'%');
                }
                FormatPart::Conv(c) => {
                    let arg = if arg_idx < rest_args.len() {
                        rest_args[arg_idx].as_str()
                    } else {
                        // Missing arg: %s → "", %d → 0.
                        ""
                    };
                    arg_idx += 1;
                    match format_one(c, arg, &mut buf) {
                        Ok(true) => {},
                        Ok(false) => halted = true,
                        Err(msg) => {
                            eprintln!("huck: printf: {msg}");
                            exit = 1;
                        }
                    }
                }
            }
        }
        if halted { break; }
        // Cycle iff there's at least one consuming conv AND args remain.
        if !has_consuming_conv { break; }
        if arg_idx >= rest_args.len() { break; }
    }

    // Output.
    if let Some(var) = v_var {
        let s = String::from_utf8_lossy(&buf).into_owned();
        if shell.try_set(&var, s).is_err() {
            eprintln!("huck: printf: {var}: readonly variable");
            return ExecOutcome::Continue(1);
        }
    } else {
        if let Err(e) = out.write_all(&buf) {
            eprintln!("huck: printf: {e}");
            return ExecOutcome::Continue(1);
        }
    }
    ExecOutcome::Continue(exit)
}
```

### 5. Escape decoder

```rust
/// Decodes a backslash-escape starting at the byte AFTER the `\`.
/// Returns Some((decoded_bytes, advance)) where advance is the
/// number of bytes consumed past the backslash. None for unknown
/// escapes — emit the backslash + the next char literally.
fn decode_printf_escape(rest: &[u8]) -> (Vec<u8>, usize) {
    if rest.is_empty() {
        return (b"\\".to_vec(), 0);
    }
    match rest[0] {
        b'\\' => (b"\\".to_vec(), 1),
        b'a'  => (b"\x07".to_vec(), 1),
        b'b'  => (b"\x08".to_vec(), 1),
        b'f'  => (b"\x0C".to_vec(), 1),
        b'n'  => (b"\n".to_vec(), 1),
        b'r'  => (b"\r".to_vec(), 1),
        b't'  => (b"\t".to_vec(), 1),
        b'v'  => (b"\x0B".to_vec(), 1),
        b'/'  => (b"/".to_vec(), 1),
        b'"'  => (b"\"".to_vec(), 1),
        b'\'' => (b"'".to_vec(), 1),
        // \NNN (1–3 octal digits). Also accept \0 prefix → \0NNN.
        c if c.is_ascii_digit() => {
            // Scan 1–3 octal digits. (If first is '0', up to 3 more
            // digits per bash printf.)
            let max = if c == b'0' { 4 } else { 3 };
            let mut n = 0usize;
            let mut v: u32 = 0;
            while n < max && n < rest.len() && (b'0'..=b'7').contains(&rest[n]) {
                v = v * 8 + (rest[n] - b'0') as u32;
                n += 1;
            }
            // Cap at byte.
            (vec![(v & 0xFF) as u8], n)
        }
        b'x' => {
            // 1–2 hex digits after \x.
            let mut n = 1;
            let mut hex = 0u32;
            let mut count = 0;
            while count < 2 && n < rest.len() && (rest[n] as char).is_ascii_hexdigit() {
                hex = hex * 16 + (rest[n] as char).to_digit(16).unwrap();
                n += 1;
                count += 1;
            }
            if count == 0 {
                // \x with no hex digit: emit literally.
                (vec![b'\\', b'x'], 1)
            } else {
                (vec![hex as u8], n)
            }
        }
        // \c inside %b → caller's responsibility (decode_printf_escape
        // ONLY runs against format-string literals; %b handling is
        // separate and handles \c).
        b'c' => (vec![b'\\', b'c'], 1),  // literal in format string
        // Unknown — emit backslash + char literally.
        c => (vec![b'\\', c], 1),
    }
}
```

For `%b` arg processing, write a parallel helper
`decode_printf_b_arg(arg: &str) -> (Vec<u8>, halted: bool)` that
honors `\c` → halt.

### 6. Dispatch + `BUILTIN_NAMES`

- `"printf"` added to `BUILTIN_NAMES`.
- NOT in `is_special_builtin` (POSIX regular).
- `"printf" => builtin_printf(args, out, shell)` arm.

## Behavior table (selected)

| Input | Output / behavior |
|---|---|
| `printf 'hello\n'` | `hello\n` |
| `printf '%s\n' a b c` | `a\nb\nc\n` (cycling) |
| `printf '%d %s\n' 5 hi 6 there` | `5 hi\n6 there\n` (cycling) |
| `printf '%-5s|' a` | `a    \|` |
| `printf '%5d' 3` | `    3` |
| `printf '%05d' 3` | `00003` |
| `printf '%.3s' 'hello'` | `hel` |
| `printf '%x' 255` | `ff` |
| `printf '%#X' 255` | `0XFF` |
| `printf '%o' 8` | `10` |
| `printf '%c' abc` | `a` |
| `printf '%%'` | `%` |
| `printf '%b' 'a\tb\n'` | `a\tb\n` (escapes processed) |
| `printf '%b' 'a\cb'` | `a` (halted; no `\n` even from cycling) |
| `printf '\x41'` | `A` |
| `printf '\101'` | `A` (octal) |
| `printf '%d' abc` | `0` + stderr "abc: invalid number" + status 1 |
| `printf -v X '%d' 42` | X="42", nothing to stdout, status 0 |
| `printf -v X '%d' 42; readonly X; printf -v X '%d' 99` | second printf errors + status 1 |
| `printf -v 1foo '%d' 42` | "not a valid identifier" + status 1 |
| `printf '%d %s' 1` | `1 ` (missing-arg → `%s` → "", `%d` → 0) |
| `printf -v X --` | usage error (no format) |
| `printf` (no args) | usage error |

## Test plan

### Unit tests in `src/builtins.rs::mod printf_tests` (20 tests)

Tests target the helpers; full builtin via integration.

**escape decoder (5):**
1. `escape_basic` — `\n` → newline; `\t` → tab; `\\` → backslash.
2. `escape_octal` — `\101` → "A"; `\0101` → "A" (leading zero accepted, takes 4 digits).
3. `escape_hex` — `\x41` → "A"; `\x4` → first hex only consumed, "\x04".
4. `escape_unknown_preserved` — `\z` → "\\z" (literal).
5. `escape_trailing_backslash` — empty rest after `\` → literal "\\".

**integer parser (4):**
6. `parse_printf_int_decimal` — `"42"` → (42, None).
7. `parse_printf_int_negative_hex_octal` — `"-42"`, `"0x1F"`, `"017"` parse correctly.
8. `parse_printf_int_char_literal` — `"'A"` → (65, None); `"\"A"` → (65, None).
9. `parse_printf_int_trailing_garbage` — `"42abc"` → (42, Some("...")).

**format parser (3):**
10. `parse_format_literal_only` — `"hello\n"` → one Literal with embedded newline.
11. `parse_format_simple_conv` — `"%s"` → one ConvSpec with conv=S, no flags, no width.
12. `parse_format_flags_width_prec` — `"%-5.3d"` → flags.left_align=true, width=5, precision=3, conv=D.

**format_one (8):**
13. `format_s_basic` — `%s` for "hi" → "hi".
14. `format_s_width` — `%5s` for "hi" → "   hi".
15. `format_s_left_align` — `%-5s` for "hi" → "hi   ".
16. `format_s_precision_truncates` — `%.3s` for "hello" → "hel".
17. `format_d_basic` — `%d` for "42" → "42".
18. `format_d_zero_pad` — `%05d` for "42" → "00042".
19. `format_x_alt_form` — `%#x` for "255" → "0xff"; `%#X` → "0XFF".
20. `format_b_arg_escapes` — `%b` for "a\\tb" → "a\tb".

### Integration tests in `tests/printf_integration.rs` (10 tests)

1. `printf_literal_only` — `printf 'hello\n'` → "hello\n".
2. `printf_s_cycling` — `printf '%s\n' a b c` → "a\nb\nc\n".
3. `printf_d_width_zero_pad` — `printf '%05d\n' 42` → "00042\n".
4. `printf_hex_alt_form` — `printf '%#x\n' 255` → "0xff\n".
5. `printf_b_processes_escapes` — `printf '%b\n' 'a\tb'` → "a\tb\n".
6. `printf_b_c_halts_output` — `printf '%b' 'a\cb' && echo X` → stdout has "a" then "X" on next line (printf produced no trailing newline; halted at \c).
7. `printf_v_var_captures` — `printf -v X '%d' 42; echo "[$X]"` → "[42]".
8. `printf_v_readonly_errors` — `readonly X=v; printf -v X '%d' 42` → stderr "readonly", status 1.
9. `printf_invalid_int_status_1` — `printf '%d\n' abc` → stderr "invalid number", status 1, stdout has "0\n".
10. `printf_no_args_usage_error` — `printf` → status 2 + stderr.

### Smoke

`cargo test --all-targets` green (PTY flake tolerated).

## Implementation tasks

1. **builtin_printf + helpers + ~20 unit tests** — touch
   `src/builtins.rs` only. All the parsing, formatting, integer
   parsing, escape decoding, `-v VAR` handling.

2. **Integration tests** — create
   `tests/printf_integration.rs` with the 10 scenarios.

3. **Docs** — M-73 entry; change-log; README v56 row.

## Acceptance criteria

- 20 unit tests pass.
- 10 integration tests pass.
- `cargo test --all-targets` green (PTY flake tolerated).
- `cargo clippy --all-targets -- -D warnings` clean.
- `printf` is regular (NOT in `is_special_builtin`).
- Format-cycling works (more args than conv specs → format
  re-applied).
- Invalid integer args produce status 1 + stderr but DON'T halt
  output.
- `-v` honors v54 readonly enforcement via `try_set`.
- `%b` with `\c` halts ALL further output, including cycling.
- Missing args: `%s` → "", `%d` → 0.
- All existing tests still pass.
