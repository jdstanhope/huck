# `read` builtin cluster — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix four `read` divergences (B-02 EOF status + var clear, B-03 last-field trailing IFS delimiter, M-162 `-n`/`-N`, M-163 `-t`) behind one record-reader refactor.

**Architecture:** Replace `read_one_line` with a config-driven `read_record` that reports a stop reason (`Delim`/`Count`/`Eof`/`Timeout`); `builtin_read` maps that to exit code + assignment. B-03 is isolated in `split_into_names`.

**Tech Stack:** Rust, `libc` (poll/termios), `crates/huck-engine/src/builtins.rs`.

**Spec:** `docs/superpowers/specs/2026-07-09-read-builtin-cluster-design.md` — read it for the derivations and the exact behavior tables.

## Global Constraints

- Per-crate tests ONLY: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`. NEVER `--workspace` (OOM on this box). huck-syntax similarly.
- Build binary: `cargo build -p huck` (debug) / `cargo build --release --bin huck` (suite).
- Diff harnesses: guard with `ulimit -v 1500000` + `timeout`.
- Commit trailer: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- Do NOT push to main without confirmation.
- All new `read` behavior must match bash 5.2 BYTE-for-byte (stdout + rc) for pipe/file/redirect input. TTY interactive timing is best-effort (spec §6); `read -t 0` pipe-race is non-deterministic (spec §5).
- The timeout exit code is `128 + libc::SIGALRM` (the libc constant, NOT a hardcoded 142).
- All key interfaces live in `crates/huck-engine/src/builtins.rs`: `builtin_read` (~2633), `read_one_line` (~2187), `split_into_names` (~2237), `RawFdReader` (~2420, has private `fd: RawFd`), `take_opt_value` (~2462, returns `Result<String,i32>`).

---

### Task 1: `read_record` reader refactor (behavior-preserving)

Introduce the config-driven reader and route `builtin_read` through it with feature flags OFF, so output is byte-identical to today but the stop reason is now available.

**Files:**
- Modify: `crates/huck-engine/src/builtins.rs` — replace `read_one_line` (~2187–2226); add `ReadCfg`/`ReadStop`; add `RawFdReader::raw_fd`; update the `builtin_read` call site (~2776).

**Interfaces:**
- Produces:
  - `struct ReadCfg { raw: bool, delim: u8, delim_active: bool, max_chars: Option<usize>, deadline: Option<std::time::Instant> }`
  - `enum ReadStop { Delim, Count, Eof, Timeout }`
  - `fn read_record<R: std::io::Read>(r: &mut R, cfg: &ReadCfg, poll_fd: Option<std::os::unix::io::RawFd>) -> std::io::Result<(String, ReadStop, bool)>` — returns (decoded string, stop reason, any_byte_read).
  - `RawFdReader::raw_fd(&self) -> std::os::unix::io::RawFd`

- [ ] **Step 1: Write failing unit tests** (append to the builtins test module — find it via `grep -n "mod tests" crates/huck-engine/src/builtins.rs`; use `use super::*;`):

```rust
#[test]
fn read_record_stops_at_delim() {
    let mut c = std::io::Cursor::new(b"abc\ndef".to_vec());
    let cfg = ReadCfg { raw: false, delim: b'\n', delim_active: true, max_chars: None, deadline: None };
    let (s, stop, any) = read_record(&mut c, &cfg, None).unwrap();
    assert_eq!(s, "abc");
    assert!(matches!(stop, ReadStop::Delim));
    assert!(any);
}

#[test]
fn read_record_eof_partial_reports_eof() {
    let mut c = std::io::Cursor::new(b"abc".to_vec());
    let cfg = ReadCfg { raw: false, delim: b'\n', delim_active: true, max_chars: None, deadline: None };
    let (s, stop, any) = read_record(&mut c, &cfg, None).unwrap();
    assert_eq!(s, "abc");
    assert!(matches!(stop, ReadStop::Eof));
    assert!(any);
}

#[test]
fn read_record_eof_empty_reports_not_any() {
    let mut c = std::io::Cursor::new(Vec::<u8>::new());
    let cfg = ReadCfg { raw: false, delim: b'\n', delim_active: true, max_chars: None, deadline: None };
    let (s, stop, any) = read_record(&mut c, &cfg, None).unwrap();
    assert_eq!(s, "");
    assert!(matches!(stop, ReadStop::Eof));
    assert!(!any);
}

#[test]
fn read_record_backslash_continuation_and_escape() {
    // "a\<newline>b\c" -> line continuation joins, \c -> c
    let mut c = std::io::Cursor::new(b"a\\\nb\\c\n".to_vec());
    let cfg = ReadCfg { raw: false, delim: b'\n', delim_active: true, max_chars: None, deadline: None };
    let (s, stop, _) = read_record(&mut c, &cfg, None).unwrap();
    assert_eq!(s, "abc");
    assert!(matches!(stop, ReadStop::Delim));
}

#[test]
fn read_record_raw_keeps_backslash() {
    let mut c = std::io::Cursor::new(b"a\\c\n".to_vec());
    let cfg = ReadCfg { raw: true, delim: b'\n', delim_active: true, max_chars: None, deadline: None };
    let (s, _, _) = read_record(&mut c, &cfg, None).unwrap();
    assert_eq!(s, "a\\c");
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p huck-engine --jobs 1 --lib read_record -- --test-threads 1`
Expected: FAIL (cannot find `read_record`/`ReadCfg`/`ReadStop`).

- [ ] **Step 3: Add the types + `read_record`, and `RawFdReader::raw_fd`.** Delete `read_one_line` and add:

```rust
#[derive(Clone)]
struct ReadCfg {
    raw: bool,
    delim: u8,
    delim_active: bool,
    max_chars: Option<usize>,
    deadline: Option<std::time::Instant>,
}

enum ReadStop { Delim, Count, Eof, Timeout }

/// Reads one `read`-record byte-at-a-time (the shared-fd-0 reason still applies —
/// see RawFdReader). Honors `-r` backslash processing, a custom `delim`, an
/// optional character-count cap (`-n`/`-N`), and an optional `-t` deadline
/// (polled via `poll_fd`). Returns the decoded string, why it stopped, and
/// whether any byte was read at all.
fn read_record<R: std::io::Read>(
    r: &mut R,
    cfg: &ReadCfg,
    poll_fd: Option<std::os::unix::io::RawFd>,
) -> std::io::Result<(String, ReadStop, bool)> {
    let mut out: Vec<u8> = Vec::new();
    let mut any = false;
    let mut chars: usize = 0;
    // A count cap of 0 (`read -n 0`) reads nothing and succeeds via Count.
    if cfg.max_chars == Some(0) {
        return Ok((String::new(), ReadStop::Count, false));
    }
    loop {
        // -t timeout: poll before each byte. On expiry stop with what we have.
        #[cfg(unix)]
        if let (Some(deadline), Some(fd)) = (cfg.deadline, poll_fd) {
            let now = std::time::Instant::now();
            if now >= deadline {
                return Ok((String::from_utf8_lossy(&out).into_owned(), ReadStop::Timeout, any));
            }
            let ms = (deadline - now).as_millis().min(i32::MAX as u128) as i32;
            let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
            let pr = unsafe { libc::poll(&mut pfd, 1, ms) };
            if pr == 0 {
                return Ok((String::from_utf8_lossy(&out).into_owned(), ReadStop::Timeout, any));
            }
            // pr < 0 (EINTR) or > 0: fall through and attempt the read.
        }
        let mut byte = [0u8; 1];
        let n = r.read(&mut byte)?;
        if n == 0 {
            return Ok((String::from_utf8_lossy(&out).into_owned(), ReadStop::Eof, any));
        }
        any = true;
        let b = byte[0];
        if cfg.delim_active && b == cfg.delim {
            return Ok((String::from_utf8_lossy(&out).into_owned(), ReadStop::Delim, any));
        }
        if !cfg.raw && b == b'\\' {
            let mut nxt = [0u8; 1];
            let m = r.read(&mut nxt)?;
            if m == 0 {
                out.push(b'\\'); // trailing backslash at EOF
                return Ok((String::from_utf8_lossy(&out).into_owned(), ReadStop::Eof, any));
            }
            if nxt[0] == b'\n' {
                continue; // line continuation — no char committed
            }
            out.push(nxt[0]); // \X -> X, one char committed
            chars += 1;
            if cfg.max_chars == Some(chars) {
                return Ok((String::from_utf8_lossy(&out).into_owned(), ReadStop::Count, any));
            }
            continue;
        }
        out.push(b);
        // Count a character only when this byte COMPLETES a UTF-8 scalar (or is a
        // lone/invalid byte). A continuation byte (0b10xx_xxxx) mid-sequence does
        // not bump the count.
        if is_char_boundary_complete(&out) {
            chars += 1;
            if cfg.max_chars == Some(chars) {
                return Ok((String::from_utf8_lossy(&out).into_owned(), ReadStop::Count, any));
            }
        }
    }
}

/// True if `out` ends on a complete UTF-8 scalar boundary (so the last pushed
/// byte finished a character). Uses the fact that a valid trailing sequence ends
/// exactly when `from_utf8` succeeds on the final 1–4 bytes; a lone invalid byte
/// also counts as one character (huck is lossy elsewhere).
fn is_char_boundary_complete(out: &[u8]) -> bool {
    let last = out[out.len() - 1];
    if last < 0x80 { return true; }                 // ASCII
    if last & 0b1100_0000 == 0b1000_0000 {          // continuation byte
        // Complete iff it finishes the expected sequence length.
        let mut i = out.len();
        let mut cont = 0;
        while i > 0 && out[i - 1] & 0b1100_0000 == 0b1000_0000 { i -= 1; cont += 1; }
        if i == 0 { return true; } // dangling continuations: count each
        let lead = out[i - 1];
        let need = if lead >= 0xF0 { 3 } else if lead >= 0xE0 { 2 } else if lead >= 0xC0 { 1 } else { return true };
        cont == need
    } else {
        // A lead byte just pushed: a 1-byte "character" only if it's a lone
        // invalid lead (0xC0.. with a multibyte need) — treat as incomplete so
        // the following continuation completes it. But a stray >=0x80 non-cont
        // non-lead is its own char.
        last < 0xC0
    }
}
```

Add the accessor next to `RawFdReader`:

```rust
impl RawFdReader {
    fn raw_fd(&self) -> std::os::unix::io::RawFd { self.fd }
}
```

- [ ] **Step 4: Update the `builtin_read` call site.** Replace the `read_one_line(&mut handle, raw, delim)` call (~2776) with:

```rust
    let poll_fd = Some(handle.raw_fd());
    let cfg = ReadCfg { raw, delim, delim_active: true, max_chars: None, deadline: None };
    let (line, stop, any_read) = match read_record(&mut handle, &cfg, poll_fd) {
        Ok(t) => t,
        Err(e) => { /* keep the existing error arm: restore echo, print bash_io_error, return Continue(1) */ }
    };
```

Keep the existing error handling (echo restore + `bash_io_error`). For THIS task, immediately after, preserve today's semantics with a shim so behavior is unchanged:

```rust
    // Task-1 shim (removed in Task 2): map EOF-with-nothing to the old None path.
    let line = if !any_read && matches!(stop, ReadStop::Eof) {
        return ExecOutcome::Continue(1);
    } else { line };
```

(The `line_opt`/`Some(l)`/`None` block at ~2809 is replaced by this. Assignment code below stays as-is for now — exit stays 0 on a successful assign, matching today. `stop`/`any_read` are consumed in Task 2.)

- [ ] **Step 5: Run tests**

Run: `cargo test -p huck-engine --jobs 1 --lib read -- --test-threads 1`
Expected: PASS (new `read_record` tests + all existing `read` tests).

- [ ] **Step 6: Behavior parity spot-check.** Build `cargo build -p huck` and confirm no change vs bash on existing behavior:

```bash
H=target/debug/huck
printf 'a b c\n' | $H -c 'read x y; echo "[$x][$y]"'          # [a][b c]
printf 'a\\\nb\n'  | $H -c 'read x; echo "[$x]"'               # [ab]
printf 'x\n'       | $H -c 'read -r x; echo "[$x]"'            # [x]
```

- [ ] **Step 7: Commit** (`git add -A && git commit`).

---

### Task 2: B-02 — EOF exit status + variable clearing

**Files:** Modify `crates/huck-engine/src/builtins.rs` — `builtin_read` status/assignment block (~2809–2838).

**Interfaces:** Consumes `stop: ReadStop`, `any_read: bool` from Task 1.

- [ ] **Step 1: Write failing diff-harness rows** (start `tests/scripts/read_cluster_diff_check.sh`; model it on `tests/scripts/star_at_modifier_diff_check.sh` — a `check` helper that runs a fragment through `bash` and `$HUCK_BIN`, both via stdin pipe, comparing `stdout` + `EXIT:` code). Add:

```bash
check "eof-partial rc"     'printf abc | { read x; echo "rc=$? [$x]"; }'   # bash rc1 [abc]
check "eof-empty clears"   'printf "" | { x=OLD; read x; echo "rc=$? [$x]"; }' # bash rc1 []
check "eof-multi clears"   'printf "" | { x=A y=B; read x y; echo "rc=$? [$x][$y]"; }' # rc1 [][]
check "full-line rc0"      'printf "abc\n" | { read x; echo "rc=$? [$x]"; }'  # rc0 [abc]
```

Run: `HUCK_BIN=$PWD/target/debug/huck bash tests/scripts/read_cluster_diff_check.sh` → these 3 FAIL (rc/clear).

- [ ] **Step 2: Rewrite the status/assignment tail.** Replace the Task-1 shim + the old `line_opt` match + the assignment block with:

```rust
    // Base exit status from the stop reason (bash): 0 iff a delimiter or the
    // -n/-N count was reached; 1 on EOF (even with partial data); 128+SIGALRM
    // on -t timeout.
    let base_exit = match stop {
        ReadStop::Delim | ReadStop::Count => 0,
        ReadStop::Eof => 1,
        ReadStop::Timeout => 128 + libc::SIGALRM,
    };

    // Assignment ALWAYS runs (even on EOF/empty) so named vars are cleared to
    // empty — bash sets them, it does not leave stale values. `line` is "" on a
    // pure EOF.
    let ifs = shell.ifs();
    if let Some(arr) = array_name {
        let fields = split_read_fields(&line, &ifs);
        let map: std::collections::BTreeMap<usize, String> = fields.into_iter().enumerate().collect();
        if shell.replace_indexed(&arr, map).is_err() {
            return ExecOutcome::Continue(1);
        }
        return ExecOutcome::Continue(base_exit);
    }
    let assignments: Vec<(String, String)> = if names.is_empty() {
        vec![("REPLY".to_string(), line)]
    } else {
        split_into_names(&line, &names, &ifs)
    };
    let mut exit = base_exit;
    for (name, value) in assignments {
        if shell.try_set(&name, value).is_err() {
            crate::sh_error_to!(shell, err, None, "read: {name}: readonly variable");
            exit = 1;
        }
    }
    ExecOutcome::Continue(exit)
```

Remove the now-dead Task-1 shim and the `line_opt`/`None => return Continue(1)` block. `any_read` is no longer needed by builtin_read (the reader still returns it; a leading `_` binding is fine) — keep it bound as `_any_read` unless a later task needs it. NOTE: `libc::SIGALRM` is an i32; `128 + libc::SIGALRM` is the `i32` exit code.

- [ ] **Step 3: Run harness + unit tests**

Run: `HUCK_BIN=$PWD/target/debug/huck bash tests/scripts/read_cluster_diff_check.sh` (after `cargo build -p huck`) → the 4 B-02 rows PASS.
Run: `cargo test -p huck-engine --jobs 1 --lib read -- --test-threads 1` → PASS.

- [ ] **Step 4: Commit.**

---

### Task 3: B-03 — last-field trailing IFS delimiter

**Files:** Modify `crates/huck-engine/src/builtins.rs` — `split_into_names` last-field block (~2327–2334).

**Interfaces:** none new.

- [ ] **Step 1: Write failing unit tests** (builtins test module):

```rust
#[test]
fn split_last_field_strips_sole_trailing_delim() {
    let n = vec!["x".to_string(), "y".to_string(), "z".to_string()];
    let g = |s: &str| split_into_names(s, &n, ":").into_iter().map(|(_, v)| v).collect::<Vec<_>>();
    assert_eq!(g(":a:b:"),  vec!["", "a", "b"]);     // sole trailing ':' stripped
    assert_eq!(g(":a:b::"), vec!["", "a", "b::"]);   // two trailing -> kept
    assert_eq!(g("a:b:c:d"), vec!["a", "b", "c:d"]); // interior kept
    let n2 = vec!["x".to_string(), "y".to_string()];
    let g2 = |s: &str| split_into_names(s, &n2, ":").into_iter().map(|(_, v)| v).collect::<Vec<_>>();
    assert_eq!(g2("a"),     vec!["a", ""]);
    assert_eq!(g2("a:"),    vec!["a", ""]);
    assert_eq!(g2("a::"),   vec!["a", ""]);
    assert_eq!(g2("a:::"),  vec!["a", "::"]);
    assert_eq!(g2("a:b:"),  vec!["a", "b"]);
    assert_eq!(g2("a:b::"), vec!["a", "b::"]);
}
```

Run: `cargo test -p huck-engine --jobs 1 --lib split_last_field -- --test-threads 1` → FAIL (`a::` gives `:`, `a:b:` gives `b:`).

- [ ] **Step 2: Replace the last-field computation.** The current code is:

```rust
    // Last field: rest of line from position i, with trailing ws-IFS stripped.
    let mut end = bytes.len();
    while end > i && is_ws(bytes[end - 1]) {
        end -= 1;
    }
    let last = String::from_utf8_lossy(&bytes[i..end]).into_owned();
    fields.push(last);
```

Replace with (strip trailing ws-IFS, then strip ONE trailing non-ws IFS delimiter iff it is the sole trailing delimiter, then strip trailing ws-IFS again):

```rust
    // Last field: remainder from `i`. Strip trailing ws-IFS; then strip ONE
    // trailing non-ws IFS delimiter IFF it is the sole trailing delimiter (the
    // char before it is not itself a non-ws IFS delimiter). See spec §4.
    let mut end = bytes.len();
    while end > i && is_ws(bytes[end - 1]) {
        end -= 1;
    }
    if end > i && is_nonws(bytes[end - 1]) && !(end - 1 > i && is_nonws(bytes[end - 2])) {
        end -= 1;
        while end > i && is_ws(bytes[end - 1]) {
            end -= 1;
        }
    }
    let last = String::from_utf8_lossy(&bytes[i..end]).into_owned();
    fields.push(last);
```

- [ ] **Step 3: Run unit tests** → PASS.

- [ ] **Step 4: Add harness rows** to `read_cluster_diff_check.sh`:

```bash
check "b03 :a:b: 3v"   'printf ":a:b:\n"  | { IFS=: read x y z; echo "[$x][$y][$z]"; }'
check "b03 :a:b:: 3v"  'printf ":a:b::\n" | { IFS=: read x y z; echo "[$x][$y][$z]"; }'
check "b03 a:b:c:d 3v" 'printf "a:b:c:d\n"| { IFS=: read x y z; echo "[$x][$y][$z]"; }'
check "b03 a: 2v"      'printf "a:\n"     | { IFS=: read x y; echo "[$x][$y]"; }'
check "b03 a:: 2v"     'printf "a::\n"    | { IFS=: read x y; echo "[$x][$y]"; }'
check "b03 a::: 2v"    'printf "a:::\n"   | { IFS=: read x y; echo "[$x][$y]"; }'
check "b03 a:b:: 2v"   'printf "a:b::\n"  | { IFS=: read x y; echo "[$x][$y]"; }'
check "b03 mixed"      'printf "a:b: \n"  | { IFS=": " read x y; echo "[$x][$y]"; }'
check "b03 single var" 'printf "a:b:\n"   | { IFS=: read x; echo "[$x]"; }'
check "b03 ws trail"   'printf "a b  \n"  | { read x y; echo "[$x][$y]"; }'
```

Run the harness (after build) → all B-03 rows PASS.

- [ ] **Step 5: Commit.**

---

### Task 4: M-162 — `read -n N` / `read -N N`

**Files:** Modify `crates/huck-engine/src/builtins.rs` — `builtin_read` option loop (~2658) + cfg construction (~2776).

**Interfaces:** Consumes `ReadCfg { max_chars, delim_active }` from Task 1.

- [ ] **Step 1: Write failing harness rows** in `read_cluster_diff_check.sh`:

```bash
check "n3 count"     'printf "hello" | { read -n 3 x; echo "rc=$? [$x]"; }'       # rc0 [hel]
check "n5 stop-nl"   'printf "ab\ncd" | { read -n 5 x; echo "rc=$? [$x]"; }'      # rc0 [ab]
check "N5 across-nl" 'printf "ab\ncd" | { read -N 5 x; echo "rc=$?"; echo "[$x]"; }'  # x="ab\ncd"
check "n0"           'printf "hi" | { read -n 0 x; echo "rc=$? [$x]"; }'          # rc0 []
check "n10 short"    'printf "hi" | { read -n 10 x; echo "rc=$? [$x]"; }'         # rc1 [hi]
check "n3 leftover"  'printf "abcdef\n" | { read -n 3 x; read y; echo "[$x][$y]"; }' # [abc][def]
check "n3 two vars"  'printf "a b c d" | { read -n 3 x y; echo "[$x][$y]"; }'     # [a][b]
check "N3 utf8"      'printf "h\xc3\xa9llo" | { read -N 3 x; echo "[$x]"; }'      # [hél]
check "n3 utf8"      'printf "h\xc3\xa9llo" | { read -n 3 x; echo "[$x]"; }'      # [hél]
check "rn3 backslash" 'printf "a\\\\bc" | { read -rn 3 x; echo "[$x]"; }'         # [a\b]
check "bad-n"        'printf "x\n" | { read -n abc y; echo "rc=$?"; }'            # rc1
```

Run → FAIL (`-n`/`-N` = invalid option).

- [ ] **Step 2: Parse `-n` / `-N` in the option loop.** Add two match arms before the catch-all `c =>` arm (~2711). Add two locals near the top of `builtin_read` (with the other `let mut` flags, ~2645): `let mut max_chars: Option<usize> = None; let mut nchars_active_delim = true;`

```rust
                b'n' | b'N' => {
                    let upper = bytes[j] == b'N';
                    let v = match take_opt_value(args, &mut i, bytes, j, "read", bytes[j] as char, err, shell) {
                        Ok(v) => v,
                        Err(rc) => return ExecOutcome::Continue(rc),
                    };
                    match v.trim().parse::<usize>() {
                        Ok(k) => { max_chars = Some(k); nchars_active_delim = !upper; }
                        Err(_) => {
                            crate::sh_error_to!(shell, err, None, "read: {v}: invalid number");
                            return ExecOutcome::Continue(1);
                        }
                    }
                    break;
                }
```

- [ ] **Step 3: Thread into `ReadCfg`.** Update the cfg built at the read call site (Task 1 Step 4 site):

```rust
    let cfg = ReadCfg {
        raw,
        delim,
        delim_active: nchars_active_delim,
        max_chars,
        deadline: None,
    };
```

- [ ] **Step 4: Run harness** (after build) → all M-162 rows PASS. Then `cargo test -p huck-engine --jobs 1 --lib read -- --test-threads 1` → PASS.

- [ ] **Step 5: Commit.**

---

### Task 5: M-163 — `read -t TIMEOUT`

**Files:** Modify `crates/huck-engine/src/builtins.rs` — option loop + cfg + a `-t 0` probe branch before the main read.

**Interfaces:** Consumes `ReadCfg { deadline }` + `read_record`'s poll path from Task 1.

- [ ] **Step 1: Write failing harness rows** in `read_cluster_diff_check.sh` (timeouts are wall-clock; keep them short):

```bash
check "t-data"        'printf "line\n" | { read -t 5 x; echo "rc=$? [$x]"; }'    # rc0 [line]
check "t0-file-ready" 'read -t 0 x < /etc/hostname; echo "rc=$?"'                # rc0 (regular file always ready)
check "bad-t"         'printf "x\n" | { read -t abc y; echo "rc=$?"; }'          # rc1
check "t-frac-data"   'printf "z\n" | { read -t 0.5 x; echo "rc=$? [$x]"; }'     # rc0 [z]
# timeout-expiry cases run as bespoke rc checks (see Step 4) — not via `check`.
```

Run → FAIL (`-t` = invalid option / rc 2).

- [ ] **Step 2: Parse `-t`.** Add locals (~2645): `let mut timeout: Option<f64> = None;`. Add a match arm before the catch-all:

```rust
                b't' => {
                    let v = match take_opt_value(args, &mut i, bytes, j, "read", 't', err, shell) {
                        Ok(v) => v,
                        Err(rc) => return ExecOutcome::Continue(rc),
                    };
                    match v.trim().parse::<f64>() {
                        Ok(t) if t >= 0.0 && t.is_finite() => timeout = Some(t),
                        _ => {
                            crate::sh_error_to!(shell, err, None, "read: {v}: invalid timeout specification");
                            return ExecOutcome::Continue(1);
                        }
                    }
                    break;
                }
```

- [ ] **Step 3: `-t 0` probe + deadline construction.** Just BEFORE building `cfg`/reading (after the fd-validity check ~2744, after `handle` is created ~2772), insert:

```rust
    // `-t 0`: availability probe — poll once with 0 timeout, read nothing.
    #[cfg(unix)]
    if timeout == Some(0.0) {
        let fd = handle.raw_fd();
        let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        let pr = unsafe { libc::poll(&mut pfd, 1, 0) };
        #[cfg(unix)] if let Some(s) = saved_term { unsafe { silent_restore_echo(tty_fd, s); } }
        return ExecOutcome::Continue(if pr > 0 { 0 } else { 1 });
    }
    let deadline = timeout.and_then(|t| {
        if t > 0.0 {
            Some(std::time::Instant::now() + std::time::Duration::from_secs_f64(t))
        } else { None }
    });
```

(Place this so `saved_term`/`tty_fd`/`handle` are already in scope — they are created at ~2758–2775. If ordering requires, move the probe just after `handle` is built.)

Then set `deadline` in the `ReadCfg`:

```rust
    let cfg = ReadCfg { raw, delim, delim_active: nchars_active_delim, max_chars, deadline };
```

- [ ] **Step 4: Bespoke timeout-expiry checks** (append to `read_cluster_diff_check.sh` as a raw compare, since `check` can't express a slow producer). Add a second helper:

```bash
check_rc_only() { local l="$1" f="$2" b h
  b=$(bash -c "$f" 2>/dev/null; echo "EXIT:$?")
  h=$("$HUCK_BIN" -c "$f" 2>/dev/null; echo "EXIT:$?")
  if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$l"; PASS=$((PASS+1))
  else printf 'FAIL: %s\n' "$l"; diff <(echo "$b") <(echo "$h")|sed 's/^/  /'; FAIL=$((FAIL+1)); fi; }

check_rc_only "t-timeout"   '( sleep 2; echo late ) | { read -t 1 x; echo "rc=$? [$x]"; }'   # rc142 []
check_rc_only "t-partial"   '( printf par; sleep 2 ) | { read -t 1 x; echo "rc=$? [$x]"; }'   # rc142 [par]
check_rc_only "t-frac-to"   '( sleep 1; echo l ) | { read -t 0.3 x; echo "rc=$?"; }'          # rc142
```

- [ ] **Step 5: Run the full harness** (guard: `ulimit -v 1500000; timeout 120 bash tests/scripts/read_cluster_diff_check.sh`) → ALL rows PASS (incl. timeout=142 cases). Then `cargo test -p huck-engine --jobs 1 --lib read -- --test-threads 1` → PASS.

- [ ] **Step 6: Commit.**

---

### Task 6: Regression, bash-suite check, docs

**Files:** Modify `docs/bash-divergences.md`; verify only elsewhere.

- [ ] **Step 1: Full per-crate suites.**

Run: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1` → green (~1780+).
Run: `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` → green (~437).

- [ ] **Step 2: bash-suite runner.** Build release (`cargo build --release --bin huck`) and run:

```bash
env -u TMPDIR BASH_SOURCE_DIR=/tmp/bash-5.2.21 HUCK_TEST_TIMEOUT=30 bash tests/bash-test-suite/runner.sh
```

Expected: PASS ≥ 15 (no regression). Record before/after diff-line counts for `read`, `redir`, `vredir`, `procsub` (currently TIMEOUT/FAIL). `read -t` may unstick a `read2.sub` hang — note movement; a flip is a bonus, not required.

- [ ] **Step 3: Update `docs/bash-divergences.md`.** DELETE the resolved entries **B-02, B-03, M-162, M-163** and decrement the summary counts (Bugs 4→2, Missing 11→9). Add a `[deferred]` note ONLY for a residual actually observed (e.g. interactive-TTY `-n`/`-t` timing, or `read -t 0` pipe-race) — word it as low-impact, matching the doc's style.

- [ ] **Step 4: Commit** the docs change.

---

## Notes for the implementer

- Read the SPEC first (`docs/superpowers/specs/2026-07-09-read-builtin-cluster-design.md`) — it has the derivation tables and the two accepted caveats.
- The `read` byte-at-a-time reader exists for a REAL reason (shared fd-0 BufReader with rustyline). Do NOT switch to buffered reads.
- `libc::poll` / `libc::SIGALRM` / `libc::pollfd` are all in the already-used `libc` crate; guard timeout/probe code with `#[cfg(unix)]` like the existing termios code.
- Timeouts in the harness are wall-clock — keep sleeps ≤ 2s and always run the harness under `timeout 120`.
