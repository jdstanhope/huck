# umask / ulimit / enable Builtins Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the `umask`, `ulimit`, and `enable` builtins (plus the `times` special builtin that `enable -ps` lists) so the bash-test categories that use them stop emitting `command not found`.

**Architecture:** Three independent builtins added to `BUILTIN_NAMES` + the `run_builtin` dispatch match in `crates/huck-engine/src/builtins.rs`. `umask`/`ulimit`/`times` call libc directly (`umask(2)`, `getrlimit`/`setrlimit`, `times(2)`). `enable` toggles a new `disabled_builtins` set on the shell, consulted by a new `builtin_active(name, shell)` predicate that the executor dispatch and `type`/`command -v` resolution use. New diagnostic sites use the existing `shell.error_prefix(Some(...))` bash prologue.

**Tech Stack:** Rust, `libc` 0.2 (already a dependency), the existing builtin dispatch pattern.

## Global Constraints

- Commit trailer verbatim on every commit: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- Run the FULL test suite with `cargo test --workspace` (plain `cargo test` skips most crates).
- New error diagnostics use `shell.error_prefix(Some("<name>"))` (yields `<src>: line N: <name>: ` in file mode, `huck: <name>: ` interactively). Usage lines are bare program-name form (`<name>: usage: …`), no source-line prologue — matching the v227 getopts/declare split.
- Builtin function shape: `fn builtin_X(args: &[String], out: &mut dyn Write, err: &mut dyn Write, shell: &mut Shell) -> ExecOutcome`. Write stdout via `out`, errors via the `e!(err, …)` macro, return `ExecOutcome::Continue(code)`.
- Diff-check harnesses run fragments as a FILE (temp script passed as an arg) through BOTH bash and huck on the SAME temp path, asserting byte-identical merged stdout+stderr+exit — model exactly on `tests/scripts/io_error_diff_check.sh`.
- NO bash category is expected to flip (broad-shrink iteration). Do not attempt the out-of-scope blockers (set +p, source4/6/7.sub, declare prologue, kill signal-spec, errors prologue rollout, read -u, `{var}>` redirections).

---

### Task 1: `umask` builtin

**Files:**
- Modify: `crates/huck-engine/src/builtins.rs` (BUILTIN_NAMES ~line 34; run_builtin dispatch ~line 97; new `builtin_umask` + symbolic-parse helpers; unit tests)
- Create: `tests/umask_integration.rs`
- Create: `tests/scripts/umask_diff_check.sh`

**Interfaces:**
- Consumes: `Shell::error_prefix(Some(&str))`, the `e!` macro, `ExecOutcome::Continue`.
- Produces: `builtin_umask` wired into dispatch; `"umask"` in `BUILTIN_NAMES`.

- [ ] **Step 1: Write the failing integration test**

Create `tests/umask_integration.rs`:

```rust
//! v230: umask builtin — file-mode (non-interactive) behavior vs bash.
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);
fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

/// Run `script` as a file arg (true non-interactive path). Returns (stdout, stderr, code).
fn run_file(script: &str) -> (String, String, i32) {
    let dir = std::env::temp_dir();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = dir.join(format!("huck_umask_{}_{}.sh", std::process::id(), n));
    { let mut f = std::fs::File::create(&path).unwrap(); f.write_all(script.as_bytes()).unwrap(); }
    let out = Command::new(huck_bin()).arg(&path).stdin(Stdio::null()).output().unwrap();
    let _ = std::fs::remove_file(&path);
    (String::from_utf8_lossy(&out.stdout).into_owned(),
     String::from_utf8_lossy(&out.stderr).into_owned(),
     out.status.code().unwrap_or(-1))
}

#[test]
fn octal_roundtrip() {
    let (o, _, c) = run_file("umask 022\numask\n");
    assert_eq!(o, "0022\n"); assert_eq!(c, 0);
}

#[test]
fn symbolic_print() {
    let (o, _, _) = run_file("umask 022\numask -S\n");
    assert_eq!(o, "u=rwx,g=rx,o=rx\n");
}

#[test]
fn posix_reusable() {
    let (o, _, _) = run_file("umask 022\numask -p\n");
    assert_eq!(o, "umask 0022\n");
}

#[test]
fn posix_symbolic_reusable() {
    let (o, _, _) = run_file("umask 002\numask -p -S\n");
    assert_eq!(o, "umask -S u=rwx,g=rwx,o=rx\n");
}

#[test]
fn set_via_symbolic() {
    let (o, _, _) = run_file("umask -S u=rwx,g=rwx,o=rx\numask\n");
    assert_eq!(o, "0002\n");
}

#[test]
fn octal_out_of_range_keeps_mask() {
    // bad octal must not change the mask; stderr names the bad arg; rc 1.
    let (o, e, c) = run_file("umask 022\numask 09\numask\n");
    assert!(e.contains("umask: 09: octal number out of range"), "stderr: {e}");
    assert_eq!(o, "0022\n", "mask must be unchanged"); assert_eq!(c, 0);
}

#[test]
fn invalid_symbolic_character() {
    let (_, e, _) = run_file("umask g=u\n");
    assert!(e.contains("umask: `u': invalid symbolic mode character"), "stderr: {e}");
}

#[test]
fn invalid_symbolic_operator() {
    let (_, e, _) = run_file("umask u:rwx\n");
    assert!(e.contains("umask: `:': invalid symbolic mode operator"), "stderr: {e}");
}

#[test]
fn invalid_option() {
    let (_, e, _) = run_file("umask -i\n");
    assert!(e.contains("umask: -i: invalid option"), "stderr: {e}");
    assert!(e.contains("umask: usage: umask [-p] [-S] [mode]"), "stderr: {e}");
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --test umask_integration 2>&1 | tail -15`
Expected: FAIL — `umask` is command-not-found, so stdout/stderr don't match.

- [ ] **Step 3: Add `umask` to BUILTIN_NAMES and dispatch**

In `crates/huck-engine/src/builtins.rs`, add `"umask"` to the `BUILTIN_NAMES` array (append after `"bind"` or in the builtins-other cluster — placement does not affect behavior). Add a dispatch arm in `run_builtin`'s match (next to the other `out,err,shell` builtins):

```rust
        "umask" => builtin_umask(args, out, err, shell),
```

- [ ] **Step 4: Implement `builtin_umask` + symbolic parse helpers**

Add to `builtins.rs` (near the other builtins):

```rust
enum SymErr { Char(char), Operator(char) }

/// Parse an octal umask literal (digits 0-7 only). Err on any non-octal digit.
fn parse_octal_umask(s: &str) -> Result<u32, ()> {
    let mut val: u32 = 0;
    for ch in s.chars() {
        let d = ch.to_digit(8).ok_or(())?; // rejects 8,9 and non-digits
        val = val.checked_mul(8).and_then(|v| v.checked_add(d)).ok_or(())?;
    }
    if s.is_empty() { return Err(()); }
    Ok(val & 0o777)
}

/// Parse a symbolic umask string against the current mask. mask bit set = deny.
fn parse_symbolic_umask(s: &str, cur: u32) -> Result<u32, SymErr> {
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    let mut mask = cur & 0o777;
    loop {
        // who
        let mut shifts: Vec<u32> = Vec::new();
        while i < chars.len() && matches!(chars[i], 'u' | 'g' | 'o' | 'a') {
            match chars[i] {
                'u' => shifts.push(6),
                'g' => shifts.push(3),
                'o' => shifts.push(0),
                'a' => { shifts.extend([6, 3, 0]); }
                _ => unreachable!(),
            }
            i += 1;
        }
        if shifts.is_empty() { shifts = vec![6, 3, 0]; }
        // operator
        if i >= chars.len() { return Err(SymErr::Operator('\0')); }
        let op = chars[i];
        if !matches!(op, '=' | '+' | '-') { return Err(SymErr::Operator(op)); }
        i += 1;
        // perms
        let mut perm: u32 = 0;
        while i < chars.len() && matches!(chars[i], 'r' | 'w' | 'x') {
            perm |= match chars[i] { 'r' => 4, 'w' => 2, 'x' => 1, _ => 0 };
            i += 1;
        }
        for sh in &shifts {
            match op {
                '=' => { mask &= !(0o7 << sh); mask |= ((!perm & 0o7) << sh); }
                '+' => { mask &= !(perm << sh); }
                '-' => { mask |= perm << sh; }
                _ => unreachable!(),
            }
        }
        // clause boundary
        if i >= chars.len() { break; }
        if chars[i] == ',' { i += 1; continue; }
        return Err(SymErr::Char(chars[i]));
    }
    Ok(mask & 0o777)
}

/// Symbolic rendering of the ALLOWED perms (complement of mask) as `u=rwx,g=rx,o=rx`.
fn format_symbolic_umask(mask: u32) -> String {
    let mut parts = Vec::new();
    for (cls, sh) in [('u', 6u32), ('g', 3), ('o', 0)] {
        let allowed = (!mask >> sh) & 0o7;
        let mut p = String::new();
        if allowed & 4 != 0 { p.push('r'); }
        if allowed & 2 != 0 { p.push('w'); }
        if allowed & 1 != 0 { p.push('x'); }
        parts.push(format!("{cls}={p}"));
    }
    parts.join(",")
}

fn builtin_umask(args: &[String], out: &mut dyn Write, err: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    let mut symbolic = false;
    let mut posix = false;
    let mut idx = 0;
    while idx < args.len() {
        let a = &args[idx];
        if a == "--" { idx += 1; break; }
        if a.len() > 1 && a.starts_with('-') {
            for c in a[1..].chars() {
                match c {
                    'S' => symbolic = true,
                    'p' => posix = true,
                    other => {
                        let prefix = shell.error_prefix(Some("umask"));
                        e!(err, "{prefix}-{other}: invalid option");
                        e!(err, "umask: usage: umask [-p] [-S] [mode]");
                        return ExecOutcome::Continue(2);
                    }
                }
            }
            idx += 1;
        } else { break; }
    }
    // read current mask without disturbing it
    let cur = (unsafe { let m = libc::umask(0); libc::umask(m); m } as u32) & 0o777;

    if idx < args.len() {
        let mode = &args[idx];
        let first_digit = mode.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false);
        let new_mask = if first_digit {
            match parse_octal_umask(mode) {
                Ok(m) => m,
                Err(()) => {
                    let prefix = shell.error_prefix(Some("umask"));
                    e!(err, "{prefix}{mode}: octal number out of range");
                    return ExecOutcome::Continue(1);
                }
            }
        } else {
            match parse_symbolic_umask(mode, cur) {
                Ok(m) => m,
                Err(se) => {
                    let prefix = shell.error_prefix(Some("umask"));
                    match se {
                        SymErr::Char(ch) => e!(err, "{prefix}`{ch}': invalid symbolic mode character"),
                        SymErr::Operator(ch) => e!(err, "{prefix}`{ch}': invalid symbolic mode operator"),
                    }
                    return ExecOutcome::Continue(1);
                }
            }
        };
        unsafe { libc::umask(new_mask as libc::mode_t); }
        return ExecOutcome::Continue(0);
    }

    let body = if symbolic { format_symbolic_umask(cur) } else { format!("{cur:04o}") };
    let line = match (posix, symbolic) {
        (true, true) => format!("umask -S {body}"),
        (true, false) => format!("umask {body}"),
        (false, _) => body,
    };
    let _ = writeln!(out, "{line}");
    ExecOutcome::Continue(0)
}
```

- [ ] **Step 5: Add unit tests for the parsers**

Add to the `#[cfg(test)]` area of builtins.rs:

```rust
#[test]
fn umask_octal_rejects_nonoctal() {
    assert!(super::parse_octal_umask("09").is_err());
    assert_eq!(super::parse_octal_umask("022").unwrap(), 0o22);
}
#[test]
fn umask_symbolic_roundtrip() {
    assert_eq!(super::format_symbolic_umask(0o022), "u=rwx,g=rx,o=rx");
    // set o to deny write from a clear mask
    assert_eq!(super::parse_symbolic_umask("u=rwx,g=rwx,o=rx", 0).unwrap(), 0o002);
}
#[test]
fn umask_symbolic_errors() {
    assert!(matches!(super::parse_symbolic_umask("g=u", 0), Err(super::SymErr::Char('u'))));
    assert!(matches!(super::parse_symbolic_umask("u:rwx", 0), Err(super::SymErr::Operator(':'))));
    assert!(matches!(super::parse_symbolic_umask("u=rwx:g=rwx", 0), Err(super::SymErr::Char(':'))));
}
```

(If `parse_octal_umask`/`parse_symbolic_umask`/`format_symbolic_umask`/`SymErr` are not `pub`, reference them via the test module's `use super::*;` path; adjust visibility to `pub(crate)` if the test module cannot see them.)

- [ ] **Step 6: Run the integration + unit tests**

Run: `cargo test --test umask_integration 2>&1 | tail -5` then `cargo test -p huck-engine umask 2>&1 | tail -5`
Expected: all PASS.

- [ ] **Step 7: Create the diff-check harness**

Create `tests/scripts/umask_diff_check.sh`:

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v230 umask. File mode on the SAME temp
# path for both shells so the error prologue (`<src>: line N: umask: …`) matches.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
checkf() {
    local label="$1" body="$2" tmp b h
    tmp=$(mktemp "${TMPDIR:-/tmp}/huck-umask.XXXXXX")
    printf '%s\n' "$body" > "$tmp"
    b=$(bash "$tmp" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tmp" 2>&1; echo "EXIT:$?")
    rm -f "$tmp"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

checkf "octal print"      'umask 022; umask'
checkf "symbolic print"   'umask 022; umask -S'
checkf "posix print"      'umask 022; umask -p'
checkf "posix symbolic"   'umask 002; umask -p -S'
checkf "set symbolic"     'umask -S u=rwx,g=rwx,o=rx; umask'
checkf "octal range err"  'umask 09'
checkf "sym char err"     'umask g=u'
checkf "sym op err"       'umask u:rwx'
checkf "sym colon char"   'umask -S u=rwx:g=rwx,o=rx'
checkf "invalid option"   'umask -i'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 8: Build huck and run the harness**

Run: `cargo build --bin huck && HUCK_BIN="$(pwd)/target/debug/huck" bash tests/scripts/umask_diff_check.sh`
Expected: every line `PASS:`, `Fail: 0`. (If a symbolic-error line differs, the parser's char/operator distinction needs to match bash at that position — re-check against the spec's positional rule.)

- [ ] **Step 9: Commit**

```bash
git add crates/huck-engine/src/builtins.rs tests/umask_integration.rs tests/scripts/umask_diff_check.sh
git commit -m "$(printf 'v230 task 1: umask builtin (octal+symbolic, prologue errors)\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 2: `ulimit` builtin

**Files:**
- Modify: `crates/huck-engine/src/builtins.rs` (BUILTIN_NAMES; dispatch; new `builtin_ulimit` + resource table; unit tests)
- Create: `tests/ulimit_integration.rs`
- Create: `tests/scripts/ulimit_diff_check.sh`

**Interfaces:**
- Consumes: `error_prefix`, `e!`, `crate::bash_io_error`, libc `getrlimit`/`setrlimit`.
- Produces: `builtin_ulimit` wired into dispatch; `"ulimit"` in `BUILTIN_NAMES`.

- [ ] **Step 1: Write the failing integration test**

Create `tests/ulimit_integration.rs` (reuse the `run_file` helper from Task 1's file — copy it in; the `COUNTER`/`huck_bin`/`run_file` block, renaming the temp prefix to `huck_ulimit_`):

```rust
//! v230: ulimit builtin — env-independent round-trips + errors vs bash.
// (copy the run_file/COUNTER/huck_bin helper block from umask_integration.rs,
//  changing the temp prefix to "huck_ulimit_")

#[test]
fn nofile_roundtrip() {
    // lowering RLIMIT_NOFILE is always permitted; the soft value reads back.
    let (o, _, c) = run_file("ulimit -n 64\nulimit -n\n");
    assert_eq!(o, "64\n"); assert_eq!(c, 0);
}

#[test]
fn core_soft_roundtrip_within_hard() {
    // raise hard core first so the soft set is permitted regardless of env.
    let (o, _, _) = run_file("ulimit -c unlimited\nulimit -c -S -- 1000\nulimit -c\n");
    assert_eq!(o, "1000\n");
}

#[test]
fn unlimited_query() {
    let (o, _, _) = run_file("ulimit -c unlimited\nulimit -c\n");
    assert_eq!(o, "unlimited\n");
}

#[test]
fn invalid_number() {
    let (_, e, _) = run_file("ulimit -n abc\n");
    assert!(e.contains("ulimit: abc: invalid number"), "stderr: {e}");
}

#[test]
fn invalid_option() {
    let (_, e, _) = run_file("ulimit -Z\n");
    assert!(e.contains("ulimit: -Z: invalid option"), "stderr: {e}");
}
```

Run: `cargo test --test ulimit_integration 2>&1 | tail -10` → FAIL (command not found).

- [ ] **Step 2: Add `ulimit` to BUILTIN_NAMES + dispatch**

Add `"ulimit"` to `BUILTIN_NAMES`; add arm `"ulimit" => builtin_ulimit(args, out, err, shell),`.

- [ ] **Step 3: Implement the resource table + `builtin_ulimit`**

Add to builtins.rs. The multiplier and label for each letter follow bash 5.2.21's `ulimit` man page (1024-byte for -c/-d/-f/-l/-m/-q/-s/-v; raw for -e/-i/-n/-r/-t/-u/-x). `-p` is not an rlimit — report a fixed `8` and accept a set as a no-op.

```rust
struct UlimitRes {
    letter: char,
    resource: libc::__rlimit_resource_t,
    mult: u64,          // value units per limit byte/raw; 1 = unscaled
    label: &'static str, // for `-a`
}

const ULIMIT_TABLE: &[UlimitRes] = &[
    UlimitRes { letter: 'c', resource: libc::RLIMIT_CORE,      mult: 1024, label: "core file size          (blocks, -c)" },
    UlimitRes { letter: 'd', resource: libc::RLIMIT_DATA,      mult: 1024, label: "data seg size           (kbytes, -d)" },
    UlimitRes { letter: 'e', resource: libc::RLIMIT_NICE,      mult: 1,    label: "scheduling priority             (-e)" },
    UlimitRes { letter: 'f', resource: libc::RLIMIT_FSIZE,     mult: 1024, label: "file size               (blocks, -f)" },
    UlimitRes { letter: 'i', resource: libc::RLIMIT_SIGPENDING,mult: 1,    label: "pending signals                 (-i)" },
    UlimitRes { letter: 'l', resource: libc::RLIMIT_MEMLOCK,   mult: 1024, label: "max locked memory       (kbytes, -l)" },
    UlimitRes { letter: 'm', resource: libc::RLIMIT_RSS,       mult: 1024, label: "max memory size         (kbytes, -m)" },
    UlimitRes { letter: 'n', resource: libc::RLIMIT_NOFILE,    mult: 1,    label: "open files                      (-n)" },
    UlimitRes { letter: 'q', resource: libc::RLIMIT_MSGQUEUE,  mult: 1024, label: "POSIX message queues     (bytes, -q)" },
    UlimitRes { letter: 'r', resource: libc::RLIMIT_RTPRIO,    mult: 1,    label: "real-time priority              (-r)" },
    UlimitRes { letter: 's', resource: libc::RLIMIT_STACK,     mult: 1024, label: "stack size              (kbytes, -s)" },
    UlimitRes { letter: 't', resource: libc::RLIMIT_CPU,       mult: 1,    label: "cpu time               (seconds, -t)" },
    UlimitRes { letter: 'u', resource: libc::RLIMIT_NPROC,     mult: 1,    label: "max user processes              (-u)" },
    UlimitRes { letter: 'v', resource: libc::RLIMIT_AS,        mult: 1024, label: "virtual memory          (kbytes, -v)" },
    UlimitRes { letter: 'x', resource: libc::RLIMIT_LOCKS,     mult: 1,    label: "file locks                      (-x)" },
];

fn ulimit_lookup(letter: char) -> Option<&'static UlimitRes> {
    ULIMIT_TABLE.iter().find(|r| r.letter == letter)
}

fn ulimit_get(res: &UlimitRes, hard: bool) -> Option<u64> {
    let mut rl = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    if unsafe { libc::getrlimit(res.resource, &mut rl) } != 0 { return None; }
    let v = if hard { rl.rlim_max } else { rl.rlim_cur };
    if v == libc::RLIM_INFINITY { return Some(u64::MAX); } // sentinel for "unlimited"
    Some((v as u64) / res.mult)
}

/// Returns Err(io::Error) if setrlimit fails.
fn ulimit_set(res: &UlimitRes, raw: u64, set_soft: bool, set_hard: bool) -> std::io::Result<()> {
    let mut rl = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    if unsafe { libc::getrlimit(res.resource, &mut rl) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let scaled = if raw == u64::MAX { libc::RLIM_INFINITY } else { (raw.saturating_mul(res.mult)) as libc::rlim_t };
    if set_soft { rl.rlim_cur = scaled; }
    if set_hard { rl.rlim_max = scaled; }
    if unsafe { libc::setrlimit(res.resource, &rl) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

fn builtin_ulimit(args: &[String], out: &mut dyn Write, err: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    const USAGE: &str = "ulimit: usage: ulimit [-SHabcdefiklmnpqrstuvxPRT] [limit]";
    let mut want_soft = false;
    let mut want_hard = false;
    let mut show_all = false;
    let mut letters: Vec<char> = Vec::new();
    let mut idx = 0;
    while idx < args.len() {
        let a = &args[idx];
        if a == "--" { idx += 1; break; }
        if a.len() > 1 && a.starts_with('-') {
            for c in a[1..].chars() {
                match c {
                    'S' => want_soft = true,
                    'H' => want_hard = true,
                    'a' => show_all = true,
                    'p' => letters.push('p'),
                    other if ulimit_lookup(other).is_some() => letters.push(other),
                    other => {
                        let prefix = shell.error_prefix(Some("ulimit"));
                        e!(err, "{prefix}-{other}: invalid option");
                        e!(err, "{USAGE}");
                        return ExecOutcome::Continue(2);
                    }
                }
            }
            idx += 1;
        } else { break; }
    }
    let value_arg: Option<&String> = args.get(idx);

    if show_all {
        let hard = want_hard && !want_soft;
        for res in ULIMIT_TABLE {
            let v = ulimit_get(res, hard);
            let disp = match v { Some(u64::MAX) => "unlimited".to_string(), Some(n) => n.to_string(), None => "?".to_string() };
            let _ = writeln!(out, "{} {}", res.label, disp);
        }
        return ExecOutcome::Continue(0);
    }

    if letters.is_empty() { letters.push('f'); } // bash default resource

    // `-p` pipe pseudo-resource: bash reports 8 (512-byte blocks), set is a no-op.
    // The set/query loop below special-cases it.
    let do_hard = want_hard;
    let do_soft = want_soft || !want_hard; // query: soft by default; set: both unless one chosen
    let mut status = 0;

    if let Some(val) = value_arg {
        // SET
        let set_soft = want_soft || (!want_soft && !want_hard);
        let set_hard = want_hard || (!want_soft && !want_hard);
        for &lt in &letters {
            if lt == 'p' { continue; } // no-op success
            let res = ulimit_lookup(lt).unwrap();
            let raw = match val.as_str() {
                "unlimited" => u64::MAX,
                s => match s.parse::<u64>() {
                    Ok(n) => n,
                    Err(_) => {
                        let prefix = shell.error_prefix(Some("ulimit"));
                        e!(err, "{prefix}{val}: invalid number");
                        return ExecOutcome::Continue(1);
                    }
                },
            };
            if let Err(e) = ulimit_set(res, raw, set_soft, set_hard) {
                let prefix = shell.error_prefix(Some("ulimit"));
                e!(err, "{prefix}{val}: cannot modify limit: {}", crate::bash_io_error(&e));
                status = 1;
            }
        }
    } else {
        // QUERY
        let hard = do_hard && !do_soft;
        let single = letters.len() == 1;
        for &lt in &letters {
            if lt == 'p' {
                if single { let _ = writeln!(out, "8"); } else { let _ = writeln!(out, "pipe size            (512 bytes, -p) 8"); }
                continue;
            }
            let res = ulimit_lookup(lt).unwrap();
            let disp = match ulimit_get(res, hard) {
                Some(u64::MAX) => "unlimited".to_string(),
                Some(n) => n.to_string(),
                None => { status = 1; continue; }
            };
            if single { let _ = writeln!(out, "{disp}"); }
            else { let _ = writeln!(out, "{} {}", res.label, disp); }
        }
    }
    ExecOutcome::Continue(status)
}
```

Note: `libc::__rlimit_resource_t` is the Linux type for the `getrlimit`/`setrlimit` resource argument. If the build complains about the type on the target, use the type that `libc::RLIMIT_CORE`'s definition uses (check `libc`'s `getrlimit` signature) — keep the table entries as the `libc::RLIMIT_*` constants regardless.

- [ ] **Step 4: Add unit tests**

```rust
#[test]
fn ulimit_table_lookup_and_scale() {
    let c = super::ulimit_lookup('c').unwrap();
    assert_eq!(c.mult, 1024);
    let n = super::ulimit_lookup('n').unwrap();
    assert_eq!(n.mult, 1);
    assert!(super::ulimit_lookup('Z').is_none());
}
```

- [ ] **Step 5: Run integration + unit tests**

Run: `cargo test --test ulimit_integration 2>&1 | tail -8` and `cargo test -p huck-engine ulimit 2>&1 | tail -5`
Expected: PASS. (If `core_soft_roundtrip_within_hard` fails because the CI hard limit can't be raised to unlimited as non-root, the test still holds — raising the SOFT toward an existing hard works; if needed, lower the asserted value. Verify against bash with the same script.)

- [ ] **Step 6: Create the diff-check harness**

Create `tests/scripts/ulimit_diff_check.sh` (same `checkf` structure as umask's; ENV-INDEPENDENT cases only — never `-a` absolute values):

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v230 ulimit. ENV-INDEPENDENT cases only
# (round-trips of values we set in-script, and error forms). NOT `-a` absolutes.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
checkf() {
    local label="$1" body="$2" tmp b h
    tmp=$(mktemp "${TMPDIR:-/tmp}/huck-ulimit.XXXXXX")
    printf '%s\n' "$body" > "$tmp"
    b=$(bash "$tmp" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tmp" 2>&1; echo "EXIT:$?")
    rm -f "$tmp"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

checkf "nofile roundtrip"  'ulimit -n 64; ulimit -n'
checkf "core soft set"     'ulimit -c unlimited; ulimit -c -S -- 1000; ulimit -c'
checkf "unlimited query"   'ulimit -c unlimited; ulimit -c'
checkf "invalid number"    'ulimit -n abc'
checkf "invalid option"    'ulimit -Z'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 7: Build + run harness; manual `-a` spot check**

Run: `cargo build --bin huck && HUCK_BIN="$(pwd)/target/debug/huck" bash tests/scripts/ulimit_diff_check.sh`
Expected: `Fail: 0`. Then a manual (non-pinned) spot-check that `-a` does not crash and roughly matches bash's columns:
`diff <(bash -c 'ulimit -a') <(./target/debug/huck -c 'ulimit -a') || echo "(-a differences are acceptable / not harness-pinned)"`
Record any `-a` label drift in the task report (informational only).

- [ ] **Step 8: Commit**

```bash
git add crates/huck-engine/src/builtins.rs tests/ulimit_integration.rs tests/scripts/ulimit_diff_check.sh
git commit -m "$(printf 'v230 task 2: ulimit builtin (full resource table, real get/setrlimit)\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 3: `disabled_builtins` state + `builtin_active` routing

**Files:**
- Modify: `crates/huck-engine/src/shell_state.rs` (Shell struct field + `Shell::new` init; `use std::collections::BTreeSet;`)
- Modify: `crates/huck-engine/src/builtins.rs` (new `builtin_active`; route `resolve_command_name`/`resolve_command_name_with`/`resolve_command_name_all`)
- Modify: `crates/huck-engine/src/executor.rs` (route the dispatch site ~4389 and `classify_stage` ~7128 through `builtin_active`)

**Interfaces:**
- Produces: `shell.disabled_builtins: BTreeSet<String>` (empty by default); `pub fn builtin_active(name: &str, shell: &Shell) -> bool`. Task 5 (`enable`) populates the set.

- [ ] **Step 1: Write the failing unit test**

In builtins.rs `#[cfg(test)]`:

```rust
#[test]
fn builtin_active_reflects_disabled_set() {
    let mut sh = crate::shell_state::Shell::new();
    assert!(super::builtin_active("test", &sh));      // enabled by default
    sh.disabled_builtins.insert("test".to_string());
    assert!(!super::builtin_active("test", &sh));     // now disabled
    assert!(super::is_builtin("test"));               // still a KNOWN builtin
    assert!(!super::builtin_active("not_a_builtin", &sh));
}
```

Run: `cargo test -p huck-engine builtin_active 2>&1 | tail -8` → FAIL (no field, no fn).

- [ ] **Step 2: Add the `disabled_builtins` field + init**

In `shell_state.rs`: add `use std::collections::BTreeSet;` near the other `use std::collections::*` lines. Add a field to `struct Shell` (e.g. after `coprocs`):

```rust
    pub disabled_builtins: BTreeSet<String>,
```

In `Shell::new`'s struct literal, add (next to `coprocs: Vec::new(),`):

```rust
        disabled_builtins: BTreeSet::new(),
```

(There is one constructor — `Shell::new` — and `Default` delegates to it; no other `Shell { … }` literal exists. Confirm with `grep -n "Shell {" crates/huck-engine/src/*.rs` and initialize any additional literal found.)

- [ ] **Step 3: Add `builtin_active`**

In builtins.rs, next to `is_builtin`:

```rust
/// True if `name` is a known builtin that is currently ENABLED (not turned off
/// via `enable -n`). Command dispatch and `type`/`command -v` use this so a
/// disabled builtin falls through to the external command. `enable`'s validity
/// check and the `builtin` forcing builtin use `is_builtin` (name known) instead.
pub fn builtin_active(name: &str, shell: &Shell) -> bool {
    is_builtin(name) && !shell.disabled_builtins.contains(name)
}
```

- [ ] **Step 4: Route the resolution functions**

In `resolve_command_name`, `resolve_command_name_with`, and `resolve_command_name_all`, change the builtin check from `is_builtin(name)` to `builtin_active(name, shell)` (each already has `shell: &Shell`). Example (resolve_command_name_with):

```rust
    if builtin_active(name, shell) {
        return CommandResolution::Builtin;
    }
```

Make the same substitution in `resolve_command_name` and in the builtin branch of `resolve_command_name_all`.

- [ ] **Step 5: Route the executor dispatch + classify_stage**

In executor.rs:
- The main dispatch (~line 4389): `} else if builtins::is_builtin(&resolved.program) {` → `} else if builtins::builtin_active(&resolved.program, shell) {` (`shell` is in scope).
- `classify_stage` (~line 7128): `&& !builtins::is_builtin(&prog)` → `&& !builtins::builtin_active(&prog, shell)` (`shell: &Shell` is in scope).
- LEAVE the `builtin` forcing-builtin check (~line 4194, `require_builtin && !builtins::is_builtin(...)`) as `is_builtin` — `builtin foo` must run even a disabled builtin.

(Note: declaration commands — `declare`/`local`/etc. — route through `run_declaration_builtin` via `is_declaration_command` BEFORE this dispatch; disabling a declaration builtin via `enable -n` is not wired through that separate path. This is an untested edge; leave it and note it in the report.)

- [ ] **Step 6: Run the unit test + full workspace (no behavior change expected)**

Run: `cargo test -p huck-engine builtin_active 2>&1 | tail -5` → PASS.
Run: `cargo test --workspace 2>&1 | tail -3` → `0 failed` (the disabled set is empty everywhere, so dispatch/type behavior is unchanged).

- [ ] **Step 7: Commit**

```bash
git add crates/huck-engine/src/shell_state.rs crates/huck-engine/src/builtins.rs crates/huck-engine/src/executor.rs
git commit -m "$(printf 'v230 task 3: disabled_builtins state + builtin_active routing\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 4: `times` special builtin

**Files:**
- Modify: `crates/huck-engine/src/builtins.rs` (BUILTIN_NAMES; dispatch; `is_special_builtin` membership; `builtin_times`; update the membership unit test)
- Create: `tests/times_integration.rs`

**Interfaces:**
- Produces: `"times"` in `BUILTIN_NAMES`, in `is_special_builtin`, and dispatched to `builtin_times`. Task 5 relies on `times` being a special builtin so `enable -ps` lists it.

- [ ] **Step 1: Write the failing integration test**

Create `tests/times_integration.rs` (copy the `run_file` helper, prefix `huck_times_`):

```rust
//! v230: times special builtin — two lines, shell then children, %dm%.3fs.
// (copy run_file/COUNTER/huck_bin helper, temp prefix "huck_times_")

#[test]
fn times_prints_two_lines_in_format() {
    let (o, _, c) = run_file("times\n");
    assert_eq!(c, 0);
    let lines: Vec<&str> = o.lines().collect();
    assert_eq!(lines.len(), 2, "times prints exactly two lines, got: {o:?}");
    // each line: "<m>m<s>.<ms>s <m>m<s>.<ms>s"
    for l in &lines {
        let cols: Vec<&str> = l.split(' ').collect();
        assert_eq!(cols.len(), 2, "two columns per line: {l:?}");
        for c in cols { assert!(c.contains('m') && c.ends_with('s'), "format {c:?}"); }
    }
}
```

Run: `cargo test --test times_integration 2>&1 | tail -6` → FAIL (command not found).

- [ ] **Step 2: Add `times` to BUILTIN_NAMES, dispatch, and is_special_builtin**

- Add `"times"` to `BUILTIN_NAMES`.
- Add dispatch arm: `"times" => builtin_times(args, out, err, shell),`.
- Add `times` to `is_special_builtin`'s `matches!`:

```rust
        ":" | "." | "break" | "continue" | "eval" | "exec" | "exit" | "export" | "readonly" | "return"
        | "set" | "shift" | "source" | "times" | "trap" | "unset"
```

- [ ] **Step 3: Implement `builtin_times`**

```rust
fn builtin_times(_args: &[String], out: &mut dyn Write, _err: &mut dyn Write, _shell: &mut Shell) -> ExecOutcome {
    let mut t: libc::tms = unsafe { std::mem::zeroed() };
    unsafe { libc::times(&mut t); }
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    let hz = if hz > 0 { hz as f64 } else { 100.0 };
    let fmt = |ticks: libc::clock_t| -> String {
        let secs = ticks as f64 / hz;
        let m = (secs / 60.0).floor() as u64;
        let s = secs - (m as f64) * 60.0;
        format!("{m}m{s:.3}s")
    };
    let _ = writeln!(out, "{} {}", fmt(t.tms_utime), fmt(t.tms_stime));
    let _ = writeln!(out, "{} {}", fmt(t.tms_cutime), fmt(t.tms_cstime));
    ExecOutcome::Continue(0)
}
```

- [ ] **Step 4: Update the `is_special_builtin` membership unit test**

The existing test `is_special_builtin_recognises_posix_specials` (builtins.rs ~9725) checks a subset; add `times` to its list so the new membership is asserted:

```rust
    for name in ["break", "continue", "exit", "export", "return", "unset", "times"] {
```

- [ ] **Step 5: Run tests**

Run: `cargo test --test times_integration 2>&1 | tail -5` and `cargo test -p huck-engine is_special_builtin 2>&1 | tail -5` → PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/huck-engine/src/builtins.rs tests/times_integration.rs
git commit -m "$(printf 'v230 task 4: times special builtin\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 5: `enable` builtin + harnesses + regression/re-measure

**Files:**
- Modify: `crates/huck-engine/src/builtins.rs` (BUILTIN_NAMES; dispatch; `builtin_enable`; unit tests)
- Create: `tests/enable_integration.rs`
- Create: `tests/scripts/enable_diff_check.sh`

**Interfaces:**
- Consumes: `builtin_active`/`is_builtin`/`is_special_builtin`/`BUILTIN_NAMES` (Tasks 3+4), `shell.disabled_builtins`, `error_prefix`.

- [ ] **Step 1: Write the failing integration test**

Create `tests/enable_integration.rs` (copy `run_file`, prefix `huck_enable_`):

```rust
//! v230: enable builtin — special listing, toggle, errors.
// (copy run_file/COUNTER/huck_bin helper, temp prefix "huck_enable_")

#[test]
fn enable_ps_lists_special_sorted() {
    let (o, _, c) = run_file("enable -ps\n");
    assert_eq!(c, 0);
    assert_eq!(o, "\
enable .
enable :
enable break
enable continue
enable eval
enable exec
enable exit
enable export
enable readonly
enable return
enable set
enable shift
enable source
enable times
enable trap
enable unset
");
}

#[test]
fn enable_nps_empty_when_none_disabled() {
    let (o, _, _) = run_file("enable -nps\n");
    assert_eq!(o, "");
}

#[test]
fn disable_then_type_not_builtin() {
    // enable -n test makes `type -t test` no longer "builtin".
    let (o, _, _) = run_file("enable -n test\ntype -t test\n");
    assert_ne!(o.trim(), "builtin", "got: {o:?}");
}

#[test]
fn reenable_restores_builtin() {
    let (o, _, _) = run_file("enable -n test\nenable test\ntype -t test\n");
    assert_eq!(o.trim(), "builtin");
}

#[test]
fn enable_unknown_errors() {
    let (_, e, c) = run_file("enable sh bash\n");
    assert!(e.contains("enable: sh: not a shell builtin"), "stderr: {e}");
    assert!(e.contains("enable: bash: not a shell builtin"), "stderr: {e}");
    assert_eq!(c, 1);
}
```

Run: `cargo test --test enable_integration 2>&1 | tail -10` → FAIL.

- [ ] **Step 2: Add `enable` to BUILTIN_NAMES + dispatch**

Add `"enable"`; arm `"enable" => builtin_enable(args, out, err, shell),`.

- [ ] **Step 3: Implement `builtin_enable`**

```rust
fn builtin_enable(args: &[String], out: &mut dyn Write, err: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    const USAGE: &str = "enable: usage: enable [-a] [-dnps] [-f filename] [name ...]";
    let mut disable = false; // -n
    let mut all = false;     // -a
    let mut special = false; // -s
    let mut idx = 0;
    while idx < args.len() {
        let a = &args[idx];
        if a == "--" { idx += 1; break; }
        if a.len() > 1 && a.starts_with('-') {
            for c in a[1..].chars() {
                match c {
                    'n' => disable = true,
                    'a' => all = true,
                    's' => special = true,
                    'p' => {} // print format — the listing default
                    other => {
                        let prefix = shell.error_prefix(Some("enable"));
                        e!(err, "{prefix}-{other}: invalid option");
                        e!(err, "{USAGE}");
                        return ExecOutcome::Continue(2);
                    }
                }
            }
            idx += 1;
        } else { break; }
    }
    let names = &args[idx..];

    if names.is_empty() {
        let mut cands: Vec<&str> = BUILTIN_NAMES.iter().copied()
            .filter(|n| !special || is_special_builtin(n))
            .collect();
        cands.sort_unstable();
        for n in cands {
            let is_off = shell.disabled_builtins.contains(n);
            let show = if disable { is_off } else if all { true } else { !is_off };
            if !show { continue; }
            if is_off { let _ = writeln!(out, "enable -n {n}"); }
            else { let _ = writeln!(out, "enable {n}"); }
        }
        return ExecOutcome::Continue(0);
    }

    let mut status = 0;
    for name in names {
        if !is_builtin(name) {
            let prefix = shell.error_prefix(Some("enable"));
            e!(err, "{prefix}{name}: not a shell builtin");
            status = 1;
            continue;
        }
        if disable { shell.disabled_builtins.insert(name.clone()); }
        else { shell.disabled_builtins.remove(name); }
    }
    ExecOutcome::Continue(status)
}
```

- [ ] **Step 4: Add a unit test for listing/toggle**

```rust
#[test]
fn enable_toggle_updates_set() {
    let mut sh = crate::shell_state::Shell::new();
    let mut out: Vec<u8> = Vec::new();
    let mut err: Vec<u8> = Vec::new();
    super::builtin_enable(&["-n".into(), "test".into()], &mut out, &mut err, &mut sh);
    assert!(sh.disabled_builtins.contains("test"));
    super::builtin_enable(&["test".into()], &mut out, &mut err, &mut sh);
    assert!(!sh.disabled_builtins.contains("test"));
}
```

- [ ] **Step 5: Run integration + unit tests**

Run: `cargo test --test enable_integration 2>&1 | tail -8` and `cargo test -p huck-engine enable_toggle 2>&1 | tail -5` → PASS.

- [ ] **Step 6: Create the diff-check harness**

Create `tests/scripts/enable_diff_check.sh`:

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v230 enable. File mode, same temp path
# both shells (so the `not a shell builtin` prologue matches).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
checkf() {
    local label="$1" body="$2" tmp b h
    tmp=$(mktemp "${TMPDIR:-/tmp}/huck-enable.XXXXXX")
    printf '%s\n' "$body" > "$tmp"
    b=$(bash "$tmp" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tmp" 2>&1; echo "EXIT:$?")
    rm -f "$tmp"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

checkf "list special"     'enable -ps'
checkf "list all special" 'enable -aps'
checkf "list disabled"    'enable -nps'
checkf "disable type"     'enable -n test; type -t test'
checkf "reenable type"    'enable -n test; enable test; type -t test'
checkf "unknown builtin"  'enable sh bash'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 7: Build + run the harness**

Run: `cargo build --bin huck && HUCK_BIN="$(pwd)/target/debug/huck" bash tests/scripts/enable_diff_check.sh`
Expected: `Fail: 0`. (If `list special` differs, the huck `is_special_builtin` set or `BUILTIN_NAMES` `times` entry is off — compare against the spec's exact 16-name list.)

- [ ] **Step 8: Full regression sweep**

```bash
cargo test --workspace 2>&1 | tail -3
cargo build --release --bin huck
for f in tests/scripts/*_diff_check.sh; do
  if [ "$(basename "$f")" = "funcnest_diff_check.sh" ]; then
    HUCK_BIN="$(pwd)/target/release/huck" bash "$f" >/dev/null 2>&1 && echo "ok $f" || echo "FAIL $f"
  else
    HUCK_BIN="$(pwd)/target/debug/huck" bash "$f" >/dev/null 2>&1 && echo "ok $f" || echo "FAIL $f"
  fi
done | grep -v '^ok ' || echo "all harnesses pass"
```
Expected: `cargo test --workspace` → `0 failed`; `all harnesses pass`.

- [ ] **Step 9: Category re-measure (confirm the shrink; no flip expected)**

```bash
for c in builtins errors procsub; do
  echo "== $c =="
  BASH_SOURCE_DIR=/tmp/bash-5.2.21 HUCK_BASH_TEST_HELPERS=/tmp/bash-test-helpers \
    HUCK_BASH_TEST_CATEGORY=$c bash tests/bash-test-suite/runner.sh 2>/dev/null | grep -E "\| $c \||Scratch dir"
done
```
Expected: each row `| <cat> | FAIL |` (no flip). Confirm from the printed scratch `<dir>/<cat>.diff` that the `umask`/`ulimit`/`enable: command not found` lines are gone and the `umask` errors + `enable` listing + `ulimit -c`→`1000` now match (e.g. `grep -c "command not found" <scratch>/builtins.diff` dropped). Record the before/after line counts in the task report. (`redir`/`vredir` stay TIMEOUT/FAIL — read -u / `{var}>` are out of scope.)

- [ ] **Step 10: Commit**

```bash
git add crates/huck-engine/src/builtins.rs tests/enable_integration.rs tests/scripts/enable_diff_check.sh
git commit -m "$(printf 'v230 task 5: enable builtin + harnesses; category re-measure\n\nApplies the disabled_builtins set (task 3) via enable -n/enable; lists\nspecial builtins (incl. times) for enable -ps/-aps/-nps. Broad shrink across\nbuiltins/errors/procsub command-not-found lines; no category flip.\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Self-Review

**Spec coverage:** umask octal/symbolic/-p/-S + 4 error forms (Task 1) ✓; ulimit full resource table + soft/hard + round-trips + errors (Task 2) ✓; disabled-builtin infra + builtin_active routing (Task 3) ✓; times special builtin + is_special_builtin (Task 4) ✓; enable listing/toggle/errors (Task 5) ✓; harnesses for all three + re-measure ✓. The spec's `-a` non-pinning, the "don't change mask on error" rule, and the special-list `times` requirement are all reflected.

**Placeholders:** none — every code step shows complete code. The one acknowledged approximation (ulimit `__rlimit_resource_t` type name / `-a` label drift) is called out with a concrete fallback, not left vague.

**Type consistency:** `builtin_active(name: &str, shell: &Shell) -> bool` and `disabled_builtins: BTreeSet<String>` are used identically across Tasks 3/5; `SymErr`/`parse_*_umask`/`ulimit_lookup`/`UlimitRes` names match between their definition and test references; the builtin function signature `(args, out, err, shell)` is consistent with the dispatch arms.

**Note for the implementer / reviewer:** helper-function visibility — the unit tests reference parsers via `super::`; if a parser is module-private and the test module cannot see it, make it `pub(crate)` (do not widen beyond that). Confirm there is exactly one `Shell { … }` constructor before adding the field init.
