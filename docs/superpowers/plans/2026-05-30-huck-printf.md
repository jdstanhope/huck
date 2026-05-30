# huck v56 — `printf` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add bash's `printf` builtin — POSIX core +
`%s %d %i %u %o %x %X %c %% %b`, flags, width, precision,
escape sequences, format-cycling, `-v VAR`.

**Architecture:** All code in `src/builtins.rs`. Format-string
tokenizer produces `Vec<FormatPart>` (`Literal` | `Conv(ConvSpec)`).
`format_one` consumes one conv-spec + arg → bytes into a sink.
`builtin_printf` parses `-v`, loops over `FormatPart`s, cycles
while args remain, then either writes to `out` or `try_set`s the
captured buffer.

**Tech Stack:** Rust. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-05-30-huck-printf-design.md`

**Branch:** `v56-printf` (created in preamble step P.1).

**Commit trailer convention:**

```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Preamble: Create the working branch

- [ ] **Step P.1: Create branch from main**

```bash
git checkout main
git pull --ff-only
git checkout -b v56-printf
```

Spec + this plan are committed as first commit on branch by the
controller before Task 1 begins.

---

## Task 1: builtin_printf + helpers + 20 unit tests

**Files:**
- Modify `src/builtins.rs` — add tokenizer, formatter, integer
  parser, escape decoder, `builtin_printf`, `"printf"` to
  `BUILTIN_NAMES`, dispatch arm, `mod printf_tests`.

### Step 1.1: Add `"printf"` to `BUILTIN_NAMES`

Current (post-v55), `src/builtins.rs:18-26`:

```rust
pub const BUILTIN_NAMES: &[&str] = &[
    "cd", "exit", "pwd", "echo", "export", "unset", "jobs",
    "wait", "fg", "bg", "kill", "disown", "history", "test", "[",
    "break", "continue", "return", "trap", "alias", "unalias",
    "set", "shift", ".", "source", "local",
    ":", "true", "false", "command",
    "readonly", "read",
];
```

Append `"printf"`:

```rust
pub const BUILTIN_NAMES: &[&str] = &[
    "cd", "exit", "pwd", "echo", "export", "unset", "jobs",
    "wait", "fg", "bg", "kill", "disown", "history", "test", "[",
    "break", "continue", "return", "trap", "alias", "unalias",
    "set", "shift", ".", "source", "local",
    ":", "true", "false", "command",
    "readonly", "read", "printf",
];
```

DO NOT add to `is_special_builtin` (POSIX classifies regular).

- [ ] **Step 1.1**

### Step 1.2: Add data types

Insert near `builtin_read`'s helpers (or just before
`builtin_printf`):

```rust
#[derive(Debug, Clone, PartialEq)]
enum FormatPart {
    Literal(Vec<u8>),     // raw bytes (escape-decoded already)
    Conv(ConvSpec),
}

#[derive(Debug, Clone, PartialEq, Default)]
struct ConvFlags {
    left_align: bool,
    sign: bool,
    space_sign: bool,
    alt: bool,
    zero_pad: bool,
}

#[derive(Debug, Clone, PartialEq)]
struct ConvSpec {
    flags: ConvFlags,
    width: Option<usize>,
    precision: Option<usize>,
    conv: ConvChar,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ConvChar {
    S, D, I, U, O, X, BigX, C, B, Percent,
}
```

- [ ] **Step 1.2**

### Step 1.3: Add `decode_printf_escape`

Full code in spec §5 ("Escape decoder"). Insert before
`parse_format`.

- [ ] **Step 1.3**

### Step 1.4: Add `decode_printf_b_arg`

```rust
/// Decodes escape sequences in a %b argument. Returns the decoded
/// bytes and a bool: true if a \c was encountered (caller halts
/// output).
fn decode_printf_b_arg(arg: &str) -> (Vec<u8>, bool) {
    let bytes = arg.as_bytes();
    let mut out: Vec<u8> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            // \c halts.
            if bytes[i + 1] == b'c' {
                return (out, true);
            }
            let (dec, used) = decode_printf_escape(&bytes[i + 1..]);
            out.extend_from_slice(&dec);
            i += 1 + used;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    (out, false)
}
```

- [ ] **Step 1.4**

### Step 1.5: Add `parse_format`

```rust
fn parse_format(fmt: &str) -> Result<Vec<FormatPart>, String> {
    let bytes = fmt.as_bytes();
    let mut parts: Vec<FormatPart> = Vec::new();
    let mut lit: Vec<u8> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'\\' {
            let (dec, used) = decode_printf_escape(&bytes[i + 1..]);
            lit.extend_from_slice(&dec);
            i += 1 + used;
            continue;
        }
        if b != b'%' {
            lit.push(b);
            i += 1;
            continue;
        }
        // Flush literal.
        if !lit.is_empty() {
            parts.push(FormatPart::Literal(std::mem::take(&mut lit)));
        }
        i += 1; // past '%'

        // Parse spec: [flags][width][.precision][conv]
        let mut flags = ConvFlags::default();
        // Flags can repeat in any order until a non-flag byte.
        loop {
            if i >= bytes.len() { return Err("missing conversion character".into()); }
            match bytes[i] {
                b'-' => flags.left_align = true,
                b'+' => flags.sign = true,
                b' ' => flags.space_sign = true,
                b'#' => flags.alt = true,
                b'0' => flags.zero_pad = true,
                _ => break,
            }
            i += 1;
        }
        // Width (decimal digits — no runtime `*` in v56).
        let mut width: Option<usize> = None;
        let mut wstr = String::new();
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            wstr.push(bytes[i] as char);
            i += 1;
        }
        if !wstr.is_empty() {
            width = Some(wstr.parse().unwrap_or(0));
        }
        // Precision.
        let mut precision: Option<usize> = None;
        if i < bytes.len() && bytes[i] == b'.' {
            i += 1;
            let mut pstr = String::new();
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                pstr.push(bytes[i] as char);
                i += 1;
            }
            precision = Some(if pstr.is_empty() { 0 } else { pstr.parse().unwrap_or(0) });
        }
        // Conversion char.
        if i >= bytes.len() {
            return Err("missing conversion character".into());
        }
        let conv = match bytes[i] {
            b's' => ConvChar::S,
            b'd' => ConvChar::D,
            b'i' => ConvChar::I,
            b'u' => ConvChar::U,
            b'o' => ConvChar::O,
            b'x' => ConvChar::X,
            b'X' => ConvChar::BigX,
            b'c' => ConvChar::C,
            b'b' => ConvChar::B,
            b'%' => ConvChar::Percent,
            c => return Err(format!("`%{}': invalid directive", c as char)),
        };
        i += 1;
        parts.push(FormatPart::Conv(ConvSpec { flags, width, precision, conv }));
    }
    if !lit.is_empty() {
        parts.push(FormatPart::Literal(lit));
    }
    Ok(parts)
}
```

- [ ] **Step 1.5**

### Step 1.6: Add `parse_printf_int`

```rust
fn parse_printf_int(s: &str) -> (i64, Option<String>) {
    let trimmed = s.trim_start();
    if trimmed.is_empty() {
        return (0, None);
    }
    let bytes = trimmed.as_bytes();
    // Char-literal form: leading ' or ".
    if bytes[0] == b'\'' || bytes[0] == b'"' {
        if bytes.len() == 1 {
            return (0, None);
        }
        let v = bytes[1] as i64;
        let extra = if bytes.len() > 2 {
            Some(format!("warning: `{s}': character(s) following character constant have been ignored"))
        } else {
            None
        };
        return (v, extra);
    }
    // Signed prefix.
    let (sign, rest) = match bytes[0] {
        b'+' => (1i64, &trimmed[1..]),
        b'-' => (-1i64, &trimmed[1..]),
        _ => (1i64, trimmed),
    };
    // Hex / octal / decimal.
    let (radix, digits) = if rest.starts_with("0x") || rest.starts_with("0X") {
        (16u32, &rest[2..])
    } else if rest.starts_with('0') && rest.len() > 1 {
        (8u32, &rest[1..])
    } else {
        (10u32, rest)
    };
    if digits.is_empty() {
        return (0, None);
    }
    // Consume all valid digits; report trailing garbage.
    let mut end = 0;
    for (j, c) in digits.char_indices() {
        if c.is_digit(radix) { end = j + c.len_utf8(); } else { break; }
    }
    let parsed = i64::from_str_radix(&digits[..end], radix).unwrap_or(0);
    let err = if end < digits.len() {
        Some(format!("`{s}': invalid number"))
    } else {
        None
    };
    (sign.saturating_mul(parsed), err)
}
```

- [ ] **Step 1.6**

### Step 1.7: Add `format_one`

```rust
fn format_one(
    spec: &ConvSpec,
    arg: &str,
    out: &mut Vec<u8>,
) -> Result<bool, String> {
    // Helper: pad/truncate `s` per spec.width/precision/flags.
    let pad_string = |s: &[u8], spec: &ConvSpec| -> Vec<u8> {
        let truncated: &[u8] = if let Some(p) = spec.precision {
            &s[..s.len().min(p)]
        } else {
            s
        };
        let width = spec.width.unwrap_or(0);
        if truncated.len() >= width {
            return truncated.to_vec();
        }
        let pad_len = width - truncated.len();
        let mut v = Vec::with_capacity(width);
        if spec.flags.left_align {
            v.extend_from_slice(truncated);
            v.extend(std::iter::repeat(b' ').take(pad_len));
        } else {
            v.extend(std::iter::repeat(b' ').take(pad_len));
            v.extend_from_slice(truncated);
        }
        v
    };

    let pad_number = |digits: &[u8], spec: &ConvSpec, prefix: &[u8]| -> Vec<u8> {
        // Precision = min digit count (zero-pad to precision).
        let prec = spec.precision.unwrap_or(1);
        let digit_part: Vec<u8> = if digits.len() >= prec {
            digits.to_vec()
        } else {
            let mut v = Vec::with_capacity(prec);
            v.extend(std::iter::repeat(b'0').take(prec - digits.len()));
            v.extend_from_slice(digits);
            v
        };
        let body_len = prefix.len() + digit_part.len();
        let width = spec.width.unwrap_or(0);
        if body_len >= width {
            let mut v = Vec::with_capacity(body_len);
            v.extend_from_slice(prefix);
            v.extend_from_slice(&digit_part);
            return v;
        }
        let pad_len = width - body_len;
        // Zero-pad only when no precision AND not left-aligned.
        let use_zero = spec.flags.zero_pad && !spec.flags.left_align && spec.precision.is_none();
        let pad_char = if use_zero { b'0' } else { b' ' };
        let mut v = Vec::with_capacity(width);
        if spec.flags.left_align {
            v.extend_from_slice(prefix);
            v.extend_from_slice(&digit_part);
            v.extend(std::iter::repeat(b' ').take(pad_len));
        } else if use_zero {
            // Sign/0x prefix before zeros: prefix then zeros then digits.
            v.extend_from_slice(prefix);
            v.extend(std::iter::repeat(pad_char).take(pad_len));
            v.extend_from_slice(&digit_part);
        } else {
            v.extend(std::iter::repeat(pad_char).take(pad_len));
            v.extend_from_slice(prefix);
            v.extend_from_slice(&digit_part);
        }
        v
    };

    match spec.conv {
        ConvChar::S => {
            out.extend_from_slice(&pad_string(arg.as_bytes(), spec));
            Ok(true)
        }
        ConvChar::C => {
            // First byte (or empty).
            let bytes = arg.as_bytes();
            let body: &[u8] = if bytes.is_empty() { &[] } else { &bytes[..1] };
            out.extend_from_slice(&pad_string(body, spec));
            Ok(true)
        }
        ConvChar::D | ConvChar::I => {
            let (val, err) = parse_printf_int(arg);
            let abs = val.unsigned_abs();
            let digits = abs.to_string().into_bytes();
            let mut prefix: Vec<u8> = Vec::new();
            if val < 0 {
                prefix.push(b'-');
            } else if spec.flags.sign {
                prefix.push(b'+');
            } else if spec.flags.space_sign {
                prefix.push(b' ');
            }
            out.extend_from_slice(&pad_number(&digits, spec, &prefix));
            err.map_or(Ok(true), Err)
        }
        ConvChar::U => {
            let (val, err) = parse_printf_int(arg);
            let unsigned = val as u64;
            let digits = unsigned.to_string().into_bytes();
            out.extend_from_slice(&pad_number(&digits, spec, &[]));
            err.map_or(Ok(true), Err)
        }
        ConvChar::O => {
            let (val, err) = parse_printf_int(arg);
            let unsigned = val as u64;
            let s = format!("{unsigned:o}");
            let prefix: &[u8] = if spec.flags.alt && !s.starts_with('0') {
                b"0"
            } else {
                b""
            };
            out.extend_from_slice(&pad_number(s.as_bytes(), spec, prefix));
            err.map_or(Ok(true), Err)
        }
        ConvChar::X => {
            let (val, err) = parse_printf_int(arg);
            let unsigned = val as u64;
            let s = format!("{unsigned:x}");
            let prefix: &[u8] = if spec.flags.alt && unsigned != 0 { b"0x" } else { b"" };
            out.extend_from_slice(&pad_number(s.as_bytes(), spec, prefix));
            err.map_or(Ok(true), Err)
        }
        ConvChar::BigX => {
            let (val, err) = parse_printf_int(arg);
            let unsigned = val as u64;
            let s = format!("{unsigned:X}");
            let prefix: &[u8] = if spec.flags.alt && unsigned != 0 { b"0X" } else { b"" };
            out.extend_from_slice(&pad_number(s.as_bytes(), spec, prefix));
            err.map_or(Ok(true), Err)
        }
        ConvChar::B => {
            let (decoded, halted) = decode_printf_b_arg(arg);
            out.extend_from_slice(&pad_string(&decoded, spec));
            Ok(!halted)
        }
        ConvChar::Percent => {
            // Caller treats `%%` specially (no arg consumed); shouldn't reach.
            out.push(b'%');
            Ok(true)
        }
    }
}
```

- [ ] **Step 1.7**

### Step 1.8: Add `builtin_printf`

Full code in spec §4 ("Main builtin").

- [ ] **Step 1.8**

### Step 1.9: Add dispatch arm

In `run_builtin`'s match block:

```rust
"printf" => builtin_printf(args, out, shell),
```

Position near `"read"` / `"echo"`.

- [ ] **Step 1.9**

### Step 1.10: Build

`cargo build`. Expected: clean.

- [ ] **Step 1.10**

### Step 1.11: Append `mod printf_tests` (20 tests)

At end of `src/builtins.rs` (after `mod read_tests`). All 20
tests from spec §"Test plan":

```rust
#[cfg(test)]
mod printf_tests {
    use super::*;

    // ── escape decoder ─────────────────────────────────────────

    #[test]
    fn escape_basic() {
        assert_eq!(decode_printf_escape(b"n"),  (b"\n".to_vec(), 1));
        assert_eq!(decode_printf_escape(b"t"),  (b"\t".to_vec(), 1));
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
        let spec = ConvSpec { flags: ConvFlags::default(), width: None, precision: None, conv: ConvChar::S };
        format_one(&spec, "hi", &mut out).unwrap();
        assert_eq!(out, b"hi");
    }

    #[test]
    fn format_s_width() {
        let mut out = Vec::new();
        let spec = ConvSpec { flags: ConvFlags::default(), width: Some(5), precision: None, conv: ConvChar::S };
        format_one(&spec, "hi", &mut out).unwrap();
        assert_eq!(out, b"   hi");
    }

    #[test]
    fn format_s_left_align() {
        let mut out = Vec::new();
        let spec = ConvSpec {
            flags: ConvFlags { left_align: true, ..ConvFlags::default() },
            width: Some(5),
            precision: None,
            conv: ConvChar::S,
        };
        format_one(&spec, "hi", &mut out).unwrap();
        assert_eq!(out, b"hi   ");
    }

    #[test]
    fn format_s_precision_truncates() {
        let mut out = Vec::new();
        let spec = ConvSpec { flags: ConvFlags::default(), width: None, precision: Some(3), conv: ConvChar::S };
        format_one(&spec, "hello", &mut out).unwrap();
        assert_eq!(out, b"hel");
    }

    #[test]
    fn format_d_basic() {
        let mut out = Vec::new();
        let spec = ConvSpec { flags: ConvFlags::default(), width: None, precision: None, conv: ConvChar::D };
        format_one(&spec, "42", &mut out).unwrap();
        assert_eq!(out, b"42");
    }

    #[test]
    fn format_d_zero_pad() {
        let mut out = Vec::new();
        let spec = ConvSpec {
            flags: ConvFlags { zero_pad: true, ..ConvFlags::default() },
            width: Some(5),
            precision: None,
            conv: ConvChar::D,
        };
        format_one(&spec, "42", &mut out).unwrap();
        assert_eq!(out, b"00042");
    }

    #[test]
    fn format_x_alt_form() {
        let mut out = Vec::new();
        let spec_x = ConvSpec {
            flags: ConvFlags { alt: true, ..ConvFlags::default() },
            width: None,
            precision: None,
            conv: ConvChar::X,
        };
        format_one(&spec_x, "255", &mut out).unwrap();
        assert_eq!(out, b"0xff");

        let mut out2 = Vec::new();
        let spec_bigx = ConvSpec {
            flags: ConvFlags { alt: true, ..ConvFlags::default() },
            width: None,
            precision: None,
            conv: ConvChar::BigX,
        };
        format_one(&spec_bigx, "255", &mut out2).unwrap();
        assert_eq!(out2, b"0XFF");
    }

    #[test]
    fn format_b_arg_escapes() {
        let mut out = Vec::new();
        let spec = ConvSpec { flags: ConvFlags::default(), width: None, precision: None, conv: ConvChar::B };
        format_one(&spec, "a\\tb", &mut out).unwrap();
        assert_eq!(out, b"a\tb");
    }
}
```

- [ ] **Step 1.11**

### Step 1.12: Run unit tests

```bash
cargo test --bin huck printf_tests
```

Expected: 20 pass.

If any fail, debug carefully — the spec's behavior table is the
authoritative reference.

- [ ] **Step 1.12**

### Step 1.13: Full unit suite

`cargo test --bin huck`. Expected: green.

- [ ] **Step 1.13**

### Step 1.14: Clippy

`cargo clippy --all-targets -- -D warnings`. Expected: clean.

- [ ] **Step 1.14**

### Step 1.15: Commit Task 1

```bash
git add src/builtins.rs
git commit -m "$(cat <<'EOF'
builtin: printf with POSIX core + common bash (v56 task 1)

Add bash's `printf` builtin (POSIX core + the most common bash
extensions). Conversions: %s %d %i %u %o %x %X %c %% %b. Flags:
-, +, space, #, 0. Width + .N precision. Format-cycling. Escape
sequences in format string and %b arg: \\ \a \b \f \n \r \t \v
\NNN (1-3 octal digits, with optional \0 prefix), \xHH (1-2 hex
digits). %b's \c halts all further output (including subsequent
cycling iterations). -v VAR captures output to VAR instead of
writing to stdout; readonly variables honor the v54 lock via
try_set.

Helpers in src/builtins.rs:
- FormatPart enum (Literal bytes | Conv spec).
- ConvSpec/ConvFlags/ConvChar data types.
- parse_format: walks the format string char-by-char, decoding
  backslash escapes into Literal bytes and parsing %[flags]
  [width][.precision][conv] into ConvSpec.
- decode_printf_escape: shared escape decoder for both
  format-string literals and %b arg values.
- decode_printf_b_arg: %b arg processing that ALSO recognizes
  \c → halt.
- parse_printf_int: integer parsing per POSIX/bash rules
  (decimal, +/-, hex 0x..., octal 0..., char-literal 'A/\"A,
  trailing garbage → error message + value-so-far).
- format_one: emits one conv-spec's worth of bytes into the
  caller's sink, with pad_string (for %s/%c/%b) and pad_number
  (for %d/%i/%u/%o/%x/%X) helpers handling width/precision/
  flags interaction.

builtin_printf:
- Parses leading -v VAR and -- end-of-flags. -v's NAME is
  validated via is_valid_name; invalid → status 1.
- Empty format → usage error + status 2.
- Loops over FormatParts; when args remain AND the format has
  at least one consuming conversion (%% does NOT consume), the
  format is reapplied. %b with \c short-circuits the cycle.
- Missing args: %s → "", %d → 0 (POSIX).
- Invalid integer args produce a stderr "invalid number" but
  output the parsed-prefix value and continue; overall exit
  becomes 1.
- -v captures to a Vec<u8> then String::from_utf8_lossy →
  shell.try_set(VAR, s).

"printf" added to BUILTIN_NAMES; NOT in is_special_builtin
(POSIX regular). Dispatched after "read" in run_builtin.

20 unit tests in mod printf_tests covering: escape decoder
(basic, octal, hex, unknown-preserved, trailing-backslash);
parse_printf_int (decimal, neg/hex/octal, char-literal,
trailing-garbage); parse_format (literal-only, simple-conv,
flags-width-prec); format_one (s-basic, s-width, s-left-align,
s-precision-truncates, d-basic, d-zero-pad, x-alt-form,
b-arg-escapes).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

Stage exactly: `src/builtins.rs`.

- [ ] **Step 1.15**

---

## Task 2: Integration tests

**Files:**
- Create: `tests/printf_integration.rs`

10 binary-driven tests.

### Step 2.1: Create the integration test file

Match the helper shape from `tests/read_integration.rs` etc.

```rust
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run_capture(script: &str) -> (String, String, i32) {
    let mut child = Command::new(huck_binary())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn printf_literal_only() {
    let (out, _, _) = run_capture("printf 'hello\\n'\nexit\n");
    assert!(out.lines().any(|l| l == "hello"), "stdout: {out:?}");
}

#[test]
fn printf_s_cycling() {
    let (out, _, _) = run_capture("printf '%s\\n' a b c\nexit\n");
    let collected: Vec<&str> = out.lines().take(3).collect();
    assert_eq!(collected, vec!["a", "b", "c"], "stdout: {out:?}");
}

#[test]
fn printf_d_width_zero_pad() {
    let (out, _, _) = run_capture("printf '%05d\\n' 42\nexit\n");
    assert!(out.lines().any(|l| l == "00042"), "stdout: {out:?}");
}

#[test]
fn printf_hex_alt_form() {
    let (out, _, _) = run_capture("printf '%#x\\n' 255\nexit\n");
    assert!(out.lines().any(|l| l == "0xff"), "stdout: {out:?}");
}

#[test]
fn printf_b_processes_escapes() {
    let (out, _, _) = run_capture("printf '%b\\n' 'a\\tb'\nexit\n");
    assert!(out.lines().any(|l| l == "a\tb"), "stdout: {out:?}");
}

#[test]
fn printf_b_c_halts_output() {
    // `printf '%b' 'a\cb'; echo X` → stdout begins "a" then "X".
    // No trailing newline from printf (no \n in format), no "b"
    // beyond the \c.
    let (out, _, _) = run_capture(
        "printf '%b' 'a\\cb'\necho X\nexit\n",
    );
    // The `a` and `X` should be on the same line (since printf
    // produced no newline). Echo's newline lands after "X".
    assert!(
        out.starts_with("aX\n") || out.starts_with("aX"),
        "expected stdout to start with `aX`, got: {out:?}",
    );
}

#[test]
fn printf_v_var_captures() {
    let (out, _, _) = run_capture(
        "printf -v X '%d' 42\necho \"[$X]\"\nexit\n",
    );
    assert!(out.lines().any(|l| l == "[42]"), "stdout: {out:?}");
}

#[test]
fn printf_v_readonly_errors() {
    let (out, err, _) = run_capture(
        "readonly X=v\nprintf -v X '%d' 42\nrc=$?\necho \"rc=$rc [$X]\"\nexit\n",
    );
    assert!(err.contains("readonly"), "stderr: {err:?}");
    assert!(out.lines().any(|l| l == "rc=1 [v]"), "stdout: {out:?}");
}

#[test]
fn printf_invalid_int_status_1() {
    let (out, err, _) = run_capture(
        "printf '%d\\n' abc\nrc=$?\necho rc=$rc\nexit\n",
    );
    assert!(err.contains("invalid number"), "stderr: {err:?}");
    // The parsed-prefix value of "abc" is 0; printf emits "0\n".
    assert!(out.lines().any(|l| l == "0"), "stdout: {out:?}");
    assert!(out.lines().any(|l| l == "rc=1"), "stdout: {out:?}");
}

#[test]
fn printf_no_args_usage_error() {
    let (_out, err, rc) = run_capture("printf\nexit\n");
    assert!(err.contains("usage"), "stderr: {err:?}");
    // The script proceeds to `exit`, which inherits last_status — rc
    // should be 2 from printf.
    // Actually the script ends with `exit`, which exits with last
    // status. last_status from printf was 2.
    assert_eq!(rc, 2, "expected exit 2 from usage error; got rc={rc}");
}
```

- [ ] **Step 2.1**

### Step 2.2: Run integration tests

```bash
cargo test --test printf_integration -- --nocapture
```

Expected: 10 pass.

- [ ] **Step 2.2**

### Step 2.3: Full integration suite

`cargo test --tests`. Expected: green (PTY flake tolerated).

- [ ] **Step 2.3**

### Step 2.4: Clippy

`cargo clippy --all-targets -- -D warnings`. Expected: clean.

- [ ] **Step 2.4**

### Step 2.5: Commit Task 2

```bash
git add tests/printf_integration.rs
git commit -m "$(cat <<'EOF'
test: printf integration coverage (v56 task 2)

10 binary-driven tests exercising the new `printf` builtin
end-to-end through the huck binary:

- printf_literal_only — `printf 'hello\\n'` → "hello\n".
- printf_s_cycling — `printf '%s\\n' a b c` → 3 lines.
- printf_d_width_zero_pad — `%05d` of 42 → "00042".
- printf_hex_alt_form — `%#x` of 255 → "0xff".
- printf_b_processes_escapes — `%b 'a\\tb'` → "a\tb".
- printf_b_c_halts_output — `%b 'a\\cb'` truncates at \c; next
  command's output follows on the same line.
- printf_v_var_captures — `-v X '%d' 42` → X=42, no stdout.
- printf_v_readonly_errors — readonly X; -v X → status 1 +
  stderr.
- printf_invalid_int_status_1 — `%d abc` → "0\n" + status 1 +
  stderr "invalid number".
- printf_no_args_usage_error — bare `printf` → status 2 +
  stderr usage line.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 2.5**

---

## Task 3: Docs

**Files:**
- Modify `docs/bash-divergences.md` — add M-73 entry + v56
  change-log entry.
- Modify `README.md` — add v56 row.

### Step 3.1: Add M-73 entry

After M-72 (v55 `read`):

```markdown
- **M-73: `printf`** — `[fixed v56]` high. Regular builtin (POSIX). `printf [-v VAR] FORMAT [ARGS]`. Conversions: `%s %d %i %u %o %x %X %c %% %b`. Flags: `-` (left align), `+` (sign), ` ` (sign-space), `#` (alt form: `0`/`0x`/`0X` prefix on %o/%x/%X), `0` (zero pad — suppressed by `-` and by explicit precision). Width as a decimal integer; precision via `.N` (truncates string conversions; min-digit-count for integers). Format-cycling: when args remain AND the format has at least one consuming conversion, the format is reapplied. Escape sequences in format string AND `%b` arg: `\\ \a \b \f \n \r \t \v` + 1–3 octal digit `\NNN` (with optional leading `\0`) + 1–2 hex digit `\xHH`. `%b`'s `\c` halts ALL further output (including subsequent cycling). Integer args parse decimal / `0x...` hex / leading-`0` octal / char-literal `'A` or `"A`; trailing garbage prints the parsed-prefix value, emits "huck: printf: \`ARG': invalid number", and sets status 1 (but does NOT halt output). Missing args: `%s` → "", `%d` → 0 (POSIX). `-v VAR` captures output to VAR via the v54 `try_set` so readonly variables error with status 1. Deferred: floating point (`%f`/`%e`/`%g`/`%E`/`%G`/`%a`), `%q` (shell-quote), `%(FMT)T` (strftime), runtime `*` for width/precision, Unicode `\u`/`\U` escapes.
```

- [ ] **Step 3.1**

### Step 3.2: Add v56 change-log entry

In `## Change log` after v55:

```markdown
- **2026-05-30**: M-73 (`printf`) shipped as v56. New `builtin_printf` in `src/builtins.rs` with a four-layer architecture: `parse_format` tokenizes the format string into `Vec<FormatPart>` (Literal bytes | ConvSpec); `decode_printf_escape` handles backslash escapes in both format-string literals and `%b` args (`\\ \a \b \f \n \r \t \v \NNN \xHH`); `parse_printf_int` parses integer args per POSIX/bash (decimal, signed, `0x` hex, leading-`0` octal, `'A`/`"A` char-literal, trailing-garbage → error + parsed-prefix value); `format_one` emits one ConvSpec's bytes via `pad_string` (for `%s`/`%c`/`%b`) or `pad_number` (for `%d`/`%i`/`%u`/`%o`/`%x`/`%X`) helpers handling width/precision/flag interaction. `builtin_printf` parses leading `-v VAR` and `--`, loops over the parts cycling while args remain and the format has a consuming conversion, then either writes to `out` or `try_set`s the captured Vec to the named variable (readonly-honoring via v54). `%b`'s `\c` short-circuits the cycle. Missing args follow POSIX defaults: `%s` → "", `%d` → 0. Invalid integer args produce status 1 + stderr "invalid number" but DON'T halt output. `"printf"` added to `BUILTIN_NAMES`; NOT in `is_special_builtin` (regular). 20 unit tests + 10 integration tests. Deferred: floats, `%q`, `%(...)T`, runtime `*`, Unicode escapes. No new L-* divergences.
```

- [ ] **Step 3.2**

### Step 3.3: Add v56 row to README

After the v55 row:

```markdown
| v56       | `printf` (M-73)                                                |
```

Match v55's column padding exactly.

- [ ] **Step 3.3**

### Step 3.4: Full suite

`cargo test --all-targets`. Expected: green (PTY flake
tolerated).

- [ ] **Step 3.4**

### Step 3.5: Clippy

`cargo clippy --all-targets -- -D warnings`. Expected: clean.

- [ ] **Step 3.5**

### Step 3.6: Commit Task 3

```bash
git add docs/bash-divergences.md README.md
git commit -m "$(cat <<'EOF'
docs: add M-73 (printf) fixed v56

New M-73 entry in docs/bash-divergences.md tracks bash's `printf`
builtin as [fixed v56]. Covers conversions (%s/%d/%i/%u/%o/%x/
%X/%c/%%/%b), flags (-+space#0), width, .N precision, escape
sequences (\\\\ \\a \\b \\f \\n \\r \\t \\v \\NNN \\xHH), format-
cycling, %b's \\c halt, POSIX integer parsing, missing-arg
defaults, invalid-int → status 1 + stderr but continues output,
and -v VAR's readonly-honoring via try_set. Lists deferred (floats,
%q, %(FMT)T, runtime *, Unicode \\u/\\U).

Change log: 2026-05-30 v56 entry summarizing the four-layer
architecture (parse_format / decode_printf_escape /
parse_printf_int / format_one) and builtin_printf's flag parsing,
cycling, and -v capture.

README: v56 row added to the version table.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 3.6**

---

## Final verification (controller)

1. `cargo test --all-targets` once more.
2. `cargo clippy --all-targets -- -D warnings`.
3. Branch is four commits ahead of `main`: docs preamble + 3 task
   commits.
4. Dispatch a final cross-task code-reviewer subagent over
   `main..v56-printf`.
5. Merge to `main` with `--no-ff`, push, delete branch, update
   memory.
