# v229 — Rust io::Error text leakage Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop huck leaking Rust error text (`(os error N)`, `stream did not contain valid UTF-8`) into messages, matching bash's bare strerror / `cannot execute binary file`, and bundle the error-prologue conversion on the file-IO sites (cd, redirect-open, source) so the category io-error lines shrink.

**Architecture:** A shared `bash_io_error(&io::Error)` helper strips Rust's ` (os error N)` Display suffix; it is applied at every io::Error formatting site. The file-IO error sites (cd, redirect-open, source) additionally route through `Shell::error_prefix(...)`, with the source builtin restructured to bash's four distinct forms (not-found, directory, permission, binary).

**Tech Stack:** Rust (workspace crate `huck-engine`, root `huck` integration tests), bash 5.2.21 as oracle, `tests/scripts/*_diff_check.sh` byte-identical harnesses.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-06-26-io-error-text-leakage-design.md`.
- Commit trailer on EVERY commit, verbatim: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- `bash_io_error(&io::Error) -> String` returns the bare strerror (strips ` (os error N)` when `raw_os_error()` is `Some`); Rust-synthesized errors keep Display. Apply ONLY where `e` is a `std::io::Error`. printf's `{e}` (builtins.rs:3431/3548) is a `parse_format` error — NOT io::Error — leave it.
- The error PROLOGUE (`<name>: line N:`) appears only in non-interactive FILE/`-c` mode; interactive/stdin keeps `huck:`. Prologue tests MUST run in file mode (huck given a script path), NOT stdin. `error_prefix(None)` → `<BASH_SOURCE[0] or $0>: line N: ` / `huck: `; `error_prefix(Some("cd"))` → `<…>: line N: cd: `. Compute the prefix into a local before taking the `err`/`err_writer`.
- Run the full suite with `cargo test --workspace` (plain `cargo test` skips most crates). `funcnest_diff_check.sh` is RELEASE-only (v224 artifact); other harnesses use `target/debug/huck`.
- bash's source-error prefixing: file-OPEN failures (not-found, permission) report like a redirect (NO `.:`); opened-but-unusable files (directory, binary) report WITH `.:`.

---

### Task 1: The `bash_io_error` helper

**Files:**
- Modify: `crates/huck-engine/src/macros.rs` (add the helper)
- Modify: `crates/huck-engine/src/lib.rs` (re-export so callers use `crate::bash_io_error`)
- Test: `crates/huck-engine/src/macros.rs` (a `#[cfg(test)]` module)

**Interfaces:**
- Produces: `pub(crate) fn bash_io_error(e: &std::io::Error) -> String`, re-exported as `crate::bash_io_error`. Consumed by Tasks 2–4 and the bulk apply in Task 4.

- [ ] **Step 1: Write the failing test**

Append to `crates/huck-engine/src/macros.rs`:

```rust
#[cfg(test)]
mod bash_io_error_tests {
    use super::bash_io_error;
    use std::io::{Error, ErrorKind};

    #[test]
    fn os_error_drops_the_rust_suffix() {
        // ENOENT (2): Display is "No such file or directory (os error 2)".
        let e = Error::from_raw_os_error(2);
        assert_eq!(bash_io_error(&e), "No such file or directory");
    }

    #[test]
    fn permission_denied_os_error() {
        let e = Error::from_raw_os_error(13);
        assert_eq!(bash_io_error(&e), "Permission denied");
    }

    #[test]
    fn synthesized_error_keeps_display() {
        // No raw_os_error → keep the Display text unchanged.
        let e = Error::new(ErrorKind::InvalidData, "stream did not contain valid UTF-8");
        assert_eq!(bash_io_error(&e), "stream did not contain valid UTF-8");
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p huck-engine bash_io_error`
Expected: FAIL to compile — `bash_io_error` does not exist yet.

- [ ] **Step 3: Implement the helper**

Add to `crates/huck-engine/src/macros.rs` (after the `e!` macro):

```rust
/// Render an io::Error like bash: the bare strerror string, dropping Rust's
/// ` (os error N)` Display suffix. Rust-synthesized errors (no errno) keep
/// their Display text. The Display of an OS error is the documented
/// `"{strerror} (os error {errno})"`, so stripping that exact suffix yields the
/// same text bash gets from strerror(errno).
pub(crate) fn bash_io_error(e: &std::io::Error) -> String {
    match e.raw_os_error() {
        Some(n) => {
            let s = e.to_string();
            match s.strip_suffix(&format!(" (os error {n})")) {
                Some(stripped) => stripped.to_string(),
                None => s,
            }
        }
        None => e.to_string(),
    }
}
```

In `crates/huck-engine/src/lib.rs`, just after the `mod macros;` line (currently `#[macro_use]\nmod macros;`), add the re-export:

```rust
pub(crate) use macros::bash_io_error;
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p huck-engine bash_io_error`
Expected: PASS (3/3).

- [ ] **Step 5: Commit**

```bash
git add crates/huck-engine/src/macros.rs crates/huck-engine/src/lib.rs
git commit -m "$(cat <<'EOF'
v229 task 1: bash_io_error helper (strips Rust ` (os error N)` suffix)

Renders an io::Error like bash — bare strerror, no Rust Display suffix;
synthesized errors keep Display. Re-exported as crate::bash_io_error for the
io::Error formatting sites converted in later tasks.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: cd + redirect-open (prologue + io)

**Files:**
- Modify: `crates/huck-engine/src/builtins.rs` (cd sites 405, 428, 444)
- Modify: `crates/huck-engine/src/executor.rs` (redirect-open sites 934, 951, 962)
- Test: `tests/io_error_integration.rs` (new)

**Interfaces:**
- Consumes: `crate::bash_io_error` (Task 1); `Shell::error_prefix(Some("cd"))` / `error_prefix(None)`; `err_writer`; `shell` is in scope in cd (`builtin_cd`) and in `apply` (executor.rs:904, `shell: &mut Shell`).

- [ ] **Step 1: Write the failing tests**

Create `tests/io_error_integration.rs`:

```rust
//! v229: io::Error text (no `(os error N)` suffix) + prologue on file-IO sites.
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

/// Run huck with a script FILE so the non-interactive prologue is produced.
fn run_file(script: &str) -> (String, String, i32) {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("huck-ioe-{}-{n}.sh", std::process::id()));
    std::fs::write(&path, script).unwrap();
    let out = Command::new(huck_bin()).arg(&path).output().expect("run huck file");
    let _ = std::fs::remove_file(&path);
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn cd_missing_has_no_os_error_suffix_and_prologue() {
    let (_o, e, _c) = run_file("cd /no/such_xyz\n");
    assert!(e.contains(": line 1: cd: /no/such_xyz: No such file or directory\n"), "stderr: {e:?}");
    assert!(!e.contains("os error"), "leaked Rust suffix: {e:?}");
    assert!(!e.starts_with("huck:"), "file mode should not use huck: prologue: {e:?}");
}

#[test]
fn cd_into_file_reports_not_a_directory() {
    let (_o, e, _c) = run_file("cd /etc/hostname\n");
    assert!(e.contains(": line 1: cd: /etc/hostname: Not a directory\n"), "stderr: {e:?}");
    assert!(!e.contains("os error"), "leaked Rust suffix: {e:?}");
}

#[test]
fn redirect_read_missing_has_prologue_no_suffix() {
    let (_o, e, _c) = run_file("cat < /no/such_xyz\n");
    assert!(e.contains(": line 1: /no/such_xyz: No such file or directory\n"), "stderr: {e:?}");
    assert!(!e.contains("os error"), "leaked Rust suffix: {e:?}");
}

#[test]
fn redirect_write_to_directory_is_a_directory() {
    let (_o, e, _c) = run_file("echo hi > /etc\n");
    assert!(e.contains(": line 1: /etc: Is a directory\n"), "stderr: {e:?}");
    assert!(!e.contains("os error"), "leaked Rust suffix: {e:?}");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --test io_error_integration`
Expected: FAIL — current output is `huck: cd: …: No such file or directory (os error 2)` etc. (wrong prologue + `os error` suffix).

- [ ] **Step 3: Convert the cd sites**

In `crates/huck-engine/src/builtins.rs`, the cd error sites currently read `e!(err, "huck: cd: {target}: {e}")` (405, 428) and `e!(err, "huck: cd: {e}")` (444). Change them to compose `error_prefix(Some("cd"))` and `bash_io_error`:

```rust
// 405 and 428 (with a target):
let prefix = shell.error_prefix(Some("cd"));
e!(err, "{prefix}{target}: {}", crate::bash_io_error(&e));
```

```rust
// 444 (no target):
let prefix = shell.error_prefix(Some("cd"));
e!(err, "{prefix}{}", crate::bash_io_error(&e));
```

(If `shell` is borrowed in the match scrutinee at any of these, bind `let prefix = shell.error_prefix(Some("cd"));` on the line before the `e!`.) Leave the cd "warning: could not read current dir" (411) and `pwd` (502) for Task 4's helper-only sweep.

- [ ] **Step 4: Convert the redirect-open sites**

In `crates/huck-engine/src/executor.rs`, the path-bearing redirect-open errors (934, 962 use `{path}`; 951 uses `resolved_path(&resolved)`) currently read `e!(&mut *err, "huck: {path}: {e}")`. Change each to:

```rust
// 934 / 962:
let prefix = shell.error_prefix(None);
{ let mut err = err_writer(err_sink, sink); e!(&mut *err, "{prefix}{path}: {}", crate::bash_io_error(&e)); }
```

```rust
// 951:
let prefix = shell.error_prefix(None);
{ let mut err = err_writer(err_sink, sink); e!(&mut *err, "{prefix}{}: {}", resolved_path(&resolved), crate::bash_io_error(&e)); }
```

Compute `let prefix = shell.error_prefix(None);` before the `err_writer` block (the `&shell` borrow must end before `err_writer` borrows `err_sink`/`sink`). Leave 996 (`huck: {e}`, no path) and 1031 (heredoc) for Task 4's helper-only sweep.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --test io_error_integration`
Expected: PASS (4/4).

- [ ] **Step 6: Commit**

```bash
git add crates/huck-engine/src/builtins.rs crates/huck-engine/src/executor.rs tests/io_error_integration.rs
git commit -m "$(cat <<'EOF'
v229 task 2: cd + redirect-open errors use prologue + bash_io_error

cd → `<src>: line N: cd: <target>: <strerror>`; path-bearing redirect-open →
`<src>: line N: <path>: <strerror>`. Drops the `(os error N)` suffix and the
huck: prologue on these file-IO sites.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: source (`.`) builtin — four-form restructure + binary-exec

**Files:**
- Modify: `crates/huck-engine/src/builtins.rs` (`source_in_sink`: the resolve→None branch ~6156, the read_to_string Err branch ~6164)
- Test: `tests/io_error_integration.rs` (extend)

**Interfaces:**
- Consumes: `crate::bash_io_error`; `Shell::error_prefix(None)` and `error_prefix(Some("."))`; `resolve_source_path(filename, shell) -> Option<PathBuf>` (returns `None` for non-`is_file()` paths, including directories); `err_writer`.

- [ ] **Step 1: Write the failing tests**

Append to `tests/io_error_integration.rs`:

```rust
#[test]
fn source_not_found_matches_bash() {
    let (_o, e, _c) = run_file(". /no/such_xyz\n");
    // bash: `<src>: line 1: /no/such_xyz: No such file or directory` (no `.:`).
    assert!(e.contains(": line 1: /no/such_xyz: No such file or directory\n"), "stderr: {e:?}");
    assert!(!e.contains(".: /no/such_xyz"), "should not use the `.:` prefix for not-found: {e:?}");
}

#[test]
fn source_a_directory_is_a_directory() {
    let (_o, e, _c) = run_file(". /etc\n");
    // bash: `<src>: line 1: .: /etc: is a directory` (WITH `.:`).
    assert!(e.contains(": line 1: .: /etc: is a directory\n"), "stderr: {e:?}");
    assert!(!e.contains("file not found"), "old wrong message: {e:?}");
}

#[test]
fn source_a_binary_cannot_execute() {
    let (_o, e, _c) = run_file(". /bin/true\n");
    // bash: `<src>: line 1: .: /bin/true: cannot execute binary file` (WITH `.:`).
    assert!(e.contains(": line 1: .: /bin/true: cannot execute binary file\n"), "stderr: {e:?}");
    assert!(!e.contains("valid UTF-8"), "leaked Rust UTF-8 error: {e:?}");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --test io_error_integration source_`
Expected: FAIL — current output is `huck: .: …: file not found` (not-found/dir) and `huck: .: …: stream did not contain valid UTF-8` (binary).

- [ ] **Step 3: Restructure the resolve→None branch (6156)**

Replace the `None =>` arm (currently `e!(&mut *err, "huck: .: {filename}: file not found")`):

```rust
None => {
    // bash distinguishes a directory (opened, unusable → `.:` prefix) from a
    // genuinely-missing file (open fails → no `.:`, redirect-style).
    {
        let mut err = crate::executor::err_writer(err_sink, sink);
        if std::path::Path::new(filename).is_dir() {
            let prefix = shell.error_prefix(Some("."));
            e!(&mut *err, "{prefix}{filename}: is a directory");
        } else {
            let prefix = shell.error_prefix(None);
            e!(&mut *err, "{prefix}{filename}: No such file or directory");
        }
    }
    shell.posix_fatal(1);
    return ExecOutcome::Continue(1);
}
```

(Compute each `prefix` from `shell` before the `e!`; the `err` writer does not borrow `shell`. If the borrow checker objects to `shell.error_prefix(...)` while `err` is alive, compute the prefix string before opening the `{ let mut err … }` block.)

- [ ] **Step 4: Restructure the read_to_string Err branch (6164)**

Replace the `Err(e) =>` arm (currently `e!(&mut *errw, "huck: .: {}: {e}", path.display())`):

```rust
Err(e) => {
    let mut errw = crate::executor::err_writer(err_sink, sink);
    if e.kind() == std::io::ErrorKind::InvalidData {
        // Non-UTF-8 content: bash reports `.: <path>: cannot execute binary file`.
        let prefix = shell.error_prefix(Some("."));
        e!(&mut *errw, "{prefix}{}: cannot execute binary file", path.display());
    } else {
        // Open/read io error (permission, …): bash reports `<path>: <strerror>`
        // (redirect-style, no `.:`).
        let prefix = shell.error_prefix(None);
        e!(&mut *errw, "{prefix}{}: {}", path.display(), crate::bash_io_error(&e));
    }
    return ExecOutcome::Continue(1);
}
```

(Again, compute the `prefix` before/around the `errw` borrow as the borrow checker requires — `error_prefix` takes `&shell`, `err_writer` borrows the sinks, no conflict, but bind `prefix` first to be safe.)

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --test io_error_integration`
Expected: PASS (all 7 — the 4 from Task 2 plus these 3). If running as root, the permission case is not exercised here (it's harness-gated in Task 4); these three do not depend on permissions.

- [ ] **Step 6: Commit**

```bash
git add crates/huck-engine/src/builtins.rs tests/io_error_integration.rs
git commit -m "$(cat <<'EOF'
v229 task 3: source (.) builtin matches bash's four error forms

resolve→None now splits directory (`.: <p>: is a directory`) from not-found
(`<p>: No such file or directory`); read_to_string Err splits binary
(InvalidData → `.: <p>: cannot execute binary file`) from open io errors
(`<p>: <strerror>` via bash_io_error). Prologue via error_prefix.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: Apply the helper at the remaining io sites + harness + sweep

**Files:**
- Modify: `crates/huck-engine/src/builtins.rs`, `crates/huck-engine/src/executor.rs`, `crates/huck-engine/src/expand.rs`, `crates/huck-engine/src/history.rs` (remaining io::Error `{e}` sites)
- Create: `tests/scripts/io_error_diff_check.sh`

**Interfaces:**
- Consumes: `crate::bash_io_error` (Task 1); the cd/redirect/source conversions (Tasks 2–3).

- [ ] **Step 1: Apply `bash_io_error` at the remaining io::Error sites**

These sites keep their existing `huck:`/label prologue (their prologue conversion is deferred) but must stop leaking the `(os error N)` suffix. For each, replace the trailing `{e}` with `{}` + a `crate::bash_io_error(&e)` argument. **Verify each `e` is a `std::io::Error` before converting; skip any that is not** (e.g. a custom enum). Transformation pattern, e.g. `e!(err, "huck: read: {e}")` → `e!(err, "huck: read: {}", crate::bash_io_error(&e))`.

Candidate sites (verify io::Error):
- builtins.rs: cd warning (411), pwd (502), echo (522, 528), unset (753, 762), export (979), readonly (1621), mapfile (2523, 2544), read (2703), jobs (3675), pushd (7267), popd (7326), dirs (7392)
- executor.rs: pipe (579, 596), fork (617), redirect path-less (996), heredoc (1031)
- expand.rs: 430, 794, 816, 847, 1237 (redirection / process-substitution io errors)
- history.rs: 144, 166

Do NOT touch printf (3431, 3548 — `parse_format` error). For any site where `e` is not an io::Error, leave it unchanged and note it in the report.

- [ ] **Step 2: Build and quick-check the sweep compiles + the helper sites are clean**

Run: `cargo build -p huck-engine 2>&1 | tail -3`
Expected: compiles clean (no unused-import / type errors). Then a spot check:
```bash
cargo build --bin huck && printf 'read x < /no/such_xyz\n' > /tmp/v229probe.sh && ./target/debug/huck /tmp/v229probe.sh 2>&1
```
Expected: the message contains `No such file or directory` with NO `(os error 2)` suffix.

- [ ] **Step 3: Create the file-mode diff harness**

Create `tests/scripts/io_error_diff_check.sh`:

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v229: io::Error text (no `(os error N)`
# suffix) + bash prologue on the file-IO error sites (cd, redirect-open, source).
# File mode on the SAME temp path for both shells so the prologue matches.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
checkf() {
    local label="$1" body="$2" tmp b h
    tmp=$(mktemp "${TMPDIR:-/tmp}/huck-ioe.XXXXXX")
    printf '%s\n' "$body" > "$tmp"
    b=$(bash "$tmp" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tmp" 2>&1; echo "EXIT:$?")
    rm -f "$tmp"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

checkf "cd missing"          'cd /no/such_xyz'
checkf "cd into file"        'cd /etc/hostname'
checkf "redir read missing"  'cat < /no/such_xyz'
checkf "redir write to dir"  'echo hi > /etc'
checkf "source not found"    '. /no/such_xyz'
checkf "source a directory"  '. /etc'
checkf "source a binary"     '. /bin/true'

# The source-unreadable (permission) case can't be reproduced as root (root
# bypasses mode 000), so gate it on a non-root uid.
if [[ "$(id -u)" -ne 0 ]]; then
    tmp=$(mktemp "${TMPDIR:-/tmp}/huck-ioe-np.XXXXXX"); chmod 000 "$tmp"
    checkf "source unreadable" ". $tmp"
    rm -f "$tmp"
fi

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 4: Run the new harness — all PASS**

```bash
cargo build --bin huck
HUCK_BIN="$(pwd)/target/debug/huck" bash tests/scripts/io_error_diff_check.sh
```
Expected: every line `PASS:`, final `Fail: 0`.

- [ ] **Step 5: Full regression sweep**

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
Expected: `cargo test --workspace` → `0 failed` (count rose by the new tests); `all harnesses pass`.

- [ ] **Step 6: Confirm the broad shrink — io-error lines drop from the categories**

```bash
for c in alias execscript errors dirstack builtins; do
  BASH_SOURCE_DIR=/tmp/bash-5.2.21 HUCK_BASH_TEST_HELPERS=/tmp/bash-test-helpers \
    HUCK_BASH_TEST_CATEGORY=$c bash tests/bash-test-suite/runner.sh 2>/dev/null | grep -E "\| $c \|"
done
```
Expected: each row `| <cat> | FAIL |` (no flip; they keep other blockers). The runner prints `Scratch dir (full diffs):` — record that the `os error` / `did not contain valid UTF-8` / `file not found`-for-a-directory lines are gone from at least one category's `.diff` (e.g. `grep -c "os error" <scratch>/errors.diff` → 0).

- [ ] **Step 7: Commit**

```bash
git add crates/huck-engine/src tests/scripts/io_error_diff_check.sh
git commit -m "$(cat <<'EOF'
v229 task 4: apply bash_io_error at remaining io sites + io_error_diff_check

Strips `(os error N)` at the remaining io::Error sites (pwd/read/mapfile/jobs/
pushd/popd/dirs/echo/expand/pipe/fork/heredoc/history; their prologue stays
deferred). New file-mode harness asserts byte-identity vs bash for cd/redirect/
source. Full sweep: workspace 0 failed, all harnesses pass; the io-error lines
drop from the alias/execscript/errors/dirstack/builtins diffs.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Notes for the final whole-branch review

- The helper is applied broadly; confirm no non-io::Error site (printf) was converted and no `{e}` io site was missed in a category diff.
- Source (Task 3) is the intricate part: verify the directory-vs-not-found split (6156) and the InvalidData-vs-other split (6164) match bash's `.:`-prefix rule.
- No category flips (expected — broad-shrink); the deliverable is io-text correctness + diff shrink.
- The non-file-IO sites keep `huck:` prologue (deferred); record that as the staged-prologue follow-on in the memory/divergences at merge. The source `is a directory` distinction uses `Path::new(filename).is_dir()`, which does not cover a directory found via PATH search (a rare edge) — note it.
