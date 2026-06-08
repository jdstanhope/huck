# v110 — genuinely zero-error `mise activate` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix the two residual issues that keep `mise activate bash` from sourcing through huck with zero errors — M-90's combined `>file 2>&1` builtin-stderr leak and M-105's unquoted `${x+alt}` spurious-empty-field bug.

**Architecture:** Two independent fixes. **Part A** (`src/executor.rs`): generalize the v109 builtin-stderr `2>&1` dup so it can target a *redirected stdout file's* fd (not only the real fd 1), removing the `files.stdout.is_none()` gate. **Part B** (`src/expand.rs`): make the `ExpansionResult::Empty` arm in `expand()` quoted-aware so an unquoted empty parameter substitution contributes no field. No parser/AST change.

**Tech Stack:** Rust. `src/executor.rs`, `src/expand.rs`. Tests: `cargo test --bin huck`, `cargo test --test mise_zero_errors_integration`, `bash tests/scripts/mise_zero_errors_diff_check.sh`.

**Spec:** `docs/superpowers/specs/2026-06-08-mise-zero-errors-design.md`. Read it first.

**Commit trailer (MANDATORY, canonical — every commit):**
```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

Anchors (verify exact lines — code shifts):
- `prepare_builtin_stderr(stderr: Option<File>, dup_to_stdout: bool)` ~`src/executor.rs:2290`; `BuiltinStderrGuard` ~`:2266`; `stderr_dups_to_stdout` ~`:2329`.
- Control-builtin arm `dup_stderr_to_stdout` block ~`src/executor.rs:2721`; regular-builtin arm ~`:2772`. Both have the identical 4-line `let dup_stderr_to_stdout = files.stderr.is_none() && files.stdout.is_none() && matches!(sink, StdoutSink::Terminal) && stderr_dups_to_stdout(cmd, shell);` then `let stderr_guard = prepare_builtin_stderr(files.stderr.take(), dup_stderr_to_stdout);` then `match files.stdout { … }` then `drop(stderr_guard);`.
- `RawFd` already imported at `src/executor.rs:3`; `sink: &mut StdoutSink` param on `run_exec_single` (`:2574`); `StdoutSink::{Terminal, Capture(&mut Vec<u8>)}` (`:17`). `as_raw_fd()` needs `use std::os::unix::io::AsRawFd;` (add locally).
- `expand()` `ExpansionResult::Empty` arm: `src/expand.rs:898-900` (`Empty => { has_emitted = true; }`). `expand_assignment()` Empty arm ~`:1015` is `=> {}` (leave as-is).

---

## Task 1: Part A — builtin `>file 2>&1` routes stderr to the file

**Files:**
- Modify: `src/executor.rs` (`prepare_builtin_stderr` signature + both builtin arms)
- Create: `tests/mise_zero_errors_integration.rs`

- [ ] **Step 1: Write the failing integration tests**

Create `tests/mise_zero_errors_integration.rs` with the `run` helper copied verbatim from `tests/bashrc_zero_errors_integration.rs` (returns `(stdout, stderr, exit_code)`), plus the Part-A tests:

```rust
//! v110: genuinely zero-error mise activate.
//! Part A (M-90 combined `>file 2>&1`) + Part B (M-105 spurious empty field).
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

/// Returns (stdout, stderr, exit_code).
fn run(script: &str) -> (String, String, i32) {
    let mut child = Command::new(huck_bin())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().expect("spawn huck");
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(),
     String::from_utf8_lossy(&out.stderr).into_owned(),
     out.status.code().unwrap_or(-1))
}

// --- Part A: M-90 combined `>file 2>&1` ---

#[test]
fn combined_redirect_suppresses_builtin_stderr() {
    // `declare -p NOPE >/dev/null 2>&1` (mise line 29 shape): the builtin's
    // error must go to /dev/null, not leak to the real stderr.
    let (out, err, _c) = run("declare -p NOPEA >/dev/null 2>&1\necho ok\n");
    assert_eq!(out, "ok\n", "out: {out}");
    assert!(!err.contains("NOPEA"), "builtin stderr leaked: {err}");
}

#[test]
fn file_redirect_still_suppresses() {
    // v109 file path must still work.
    let (out, err, _c) = run("declare -p NOPEB 2>/dev/null\necho ok\n");
    assert_eq!(out, "ok\n", "out: {out}");
    assert!(!err.contains("NOPEB"), "stderr leaked: {err}");
}

#[test]
fn bare_2to1_still_pipes() {
    // bare `2>&1` (no stdout file) must still route builtin stderr into the pipe.
    let (out, _err, _c) = run("{ declare -p NOPEC 2>&1; } | grep -c NOPEC\n");
    assert_eq!(out, "1\n", "out: {out}");
}

#[test]
fn unredirected_builtin_error_still_reaches_stderr() {
    // No stderr redirect → error still hits the real fd 2 (must-not-regress).
    let (_o, err, _c) = run("declare -p NOPED\n");
    assert!(err.contains("NOPED"), "unredirected error should reach stderr: {err}");
}
```

- [ ] **Step 2: Build the binary, run the tests to confirm `combined_redirect_suppresses_builtin_stderr` FAILS**

Run: `cargo test --test mise_zero_errors_integration 2>&1 | tail -20`
Expected: `combined_redirect_suppresses_builtin_stderr` FAILS (stderr still contains `NOPEA`); the other three PASS.

- [ ] **Step 3: Change `prepare_builtin_stderr`'s signature to take an `Option<RawFd>` dup target**

In `src/executor.rs`, change the function (`~:2290`) from:
```rust
fn prepare_builtin_stderr(stderr: Option<File>, dup_to_stdout: bool) -> Option<BuiltinStderrGuard> {
    use std::os::unix::io::IntoRawFd;
    let new_fd: RawFd = match stderr {
        Some(file) => file.into_raw_fd(),
        None if dup_to_stdout => {
            // `2>&1`: duplicate fd 1 so we dup2 a copy onto fd 2 and can close it.
            let d = unsafe { libc::dup(libc::STDOUT_FILENO) };
            if d < 0 {
                return None;
            }
            d
        }
        None => return None,
    };
```
to:
```rust
fn prepare_builtin_stderr(stderr: Option<File>, dup_target: Option<RawFd>) -> Option<BuiltinStderrGuard> {
    use std::os::unix::io::IntoRawFd;
    let new_fd: RawFd = match stderr {
        Some(file) => file.into_raw_fd(),
        None => match dup_target {
            // `2>&1`: duplicate the target fd (the real fd 1, or a redirected
            // stdout file's fd) so we dup2 a copy onto fd 2 and can close it.
            Some(target) => {
                let d = unsafe { libc::dup(target) };
                if d < 0 {
                    return None;
                }
                d
            }
            None => return None,
        },
    };
```
Leave the rest of the function (the `dup`/`dup2`/guard body, `~:2304-2322`) unchanged. Update the doc comment's `dup_to_stdout` mention to describe `dup_target` (the fd `2>&1` should follow — real fd 1 for a Terminal sink, or the redirected stdout file's fd for `>file 2>&1`).

- [ ] **Step 4: Update the control-builtin arm to compute the dup target**

In `src/executor.rs` (control-builtin arm, ~`:2721`), replace:
```rust
        let dup_stderr_to_stdout = files.stderr.is_none()
            && files.stdout.is_none()
            && matches!(sink, StdoutSink::Terminal)
            && stderr_dups_to_stdout(cmd, shell);
        let stderr_guard = prepare_builtin_stderr(files.stderr.take(), dup_stderr_to_stdout);
```
with:
```rust
        // `2>&1` on a builtin: follow wherever the builtin's stdout actually
        // goes. With `>file` stdout the builtin writes to a Rust File (not fd 1),
        // so dup the FILE's fd onto fd 2; with a bare `2>&1` under a Terminal
        // sink, dup the real fd 1; a Capture sink has no fd (L-25 residual).
        let dup_target: Option<RawFd> = if files.stderr.is_none() && stderr_dups_to_stdout(cmd, shell) {
            if let Some(file) = files.stdout.as_ref() {
                use std::os::unix::io::AsRawFd;
                Some(file.as_raw_fd())
            } else if matches!(sink, StdoutSink::Terminal) {
                Some(libc::STDOUT_FILENO)
            } else {
                None
            }
        } else {
            None
        };
        let stderr_guard = prepare_builtin_stderr(files.stderr.take(), dup_target);
```

- [ ] **Step 5: Update the regular-builtin arm identically**

In `src/executor.rs` (regular-builtin arm, ~`:2772`), replace the same 4-line `dup_stderr_to_stdout` block + `prepare_builtin_stderr` call with the identical code from Step 4 (the comment + `dup_target` computation + `prepare_builtin_stderr(files.stderr.take(), dup_target)`). The two arms remain parallel.

- [ ] **Step 6: Build and run the Part-A tests**

Run: `cargo build --bin huck 2>&1 | tail -3 && cargo test --test mise_zero_errors_integration 2>&1 | tail -10`
Expected: all four Part-A tests PASS.

- [ ] **Step 7: Verify byte-identical to bash + no regression of v109 M-90 tests**

```bash
cargo build --bin huck
for f in 'declare -p NOPE >/dev/null 2>&1; echo ok' \
         'declare -p NOPE 2>/dev/null; echo ok' \
         '{ declare -p NOPE 2>&1; } | grep -c NOPE'; do
  b=$(printf '%s\n' "$f" | bash --norc --noprofile 2>&1; echo "EXIT:$?")
  h=$(printf '%s\n' "$f" | ./target/debug/huck 2>&1; echo "EXIT:$?")
  [ "$b" = "$h" ] && echo "MATCH: $f" || { echo "DIFF: $f"; echo " b=[$b]"; echo " h=[$h]"; }
done
cargo test --test bashrc_zero_errors_integration 2>&1 | tail -3
```
Expected: three `MATCH` lines; `bashrc_zero_errors_integration` still all-pass.

- [ ] **Step 8: Run the broader suite + clippy for Part A**

Run: `cargo test --bin huck 2>&1 | tail -3 && cargo clippy --bin huck 2>&1 | tail -3`
Expected: unit suite all-pass; clippy clean (no new warnings).

- [ ] **Step 9: Commit**

```bash
git add src/executor.rs tests/mise_zero_errors_integration.rs
git commit -m "$(cat <<'EOF'
fix: builtin >file 2>&1 routes stderr to the redirected file (M-90 combined)

v109's builtin-stderr 2>&1 dup was gated off whenever stdout was also
redirected (files.stdout.is_none()), so `declare -p X >/dev/null 2>&1` (mise
line 29) still leaked the builtin's error. prepare_builtin_stderr now takes an
Option<RawFd> dup target; both builtin arms select it: the redirected stdout
file's fd for `>file 2>&1`, the real fd 1 for a bare `2>&1` under a Terminal
sink, or None for a Capture sink (L-25 residual). The file writer and fd 2
share the open file description, so both land in the file like bash.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 1 report
DONE/BLOCKED, commit SHA, the exact code added (signature + both arms), the test names + pass line, the three MATCH lines, the v109-M-90 regression pass line, clippy status.

---

## Task 2: Part B — unquoted `${x+alt}` no longer emits a spurious empty field

**Files:**
- Modify: `src/expand.rs:898-900` (the `ExpansionResult::Empty` arm in `expand()`)
- Test: `tests/mise_zero_errors_integration.rs` (ADD to the file from Task 1)

- [ ] **Step 1: Write the failing Part-B tests**

ADD to `tests/mise_zero_errors_integration.rs`:
```rust
// --- Part B: M-105 unquoted `${x+alt}` spurious empty field ---

#[test]
fn unquoted_empty_alt_no_spurious_field() {
    // `${u+X}` unset, unquoted, followed by more words: must NOT inject an
    // empty leading field. bash: $# == 2.
    let (out, _e, _c) = run("set -- ${u+X} a b\necho $#\n");
    assert_eq!(out, "2\n", "out: {out}");
}

#[test]
fn empty_array_alt_no_spurious_field() {
    // The mise shape: empty array + `${arr[@]+"${arr[@]}"}` -> nothing.
    let (out, _e, _c) = run("f=()\nset -- ${f[@]+\"${f[@]}\"} -s bash\necho $#\n");
    assert_eq!(out, "2\n", "out: {out}");
}

#[test]
fn quoted_empty_alt_still_one_field() {
    // A QUOTED empty must still be one field. bash: $# == 2.
    let (out, _e, _c) = run("set -- \"${u+x}\" a\necho $#\n");
    assert_eq!(out, "2\n", "out: {out}");
}

#[test]
fn quoted_empty_field_printf() {
    // `printf '<%s>' "${u+x}"` (unset, whole-quoted) -> one empty field `<>`.
    let (out, _e, _c) = run("printf '<%s>' \"${u+x}\"\necho\n");
    assert_eq!(out, "<>\n", "out: {out}");
}

#[test]
fn set_array_idiom_unchanged() {
    // v109 behavior must be unchanged: a SET array still yields its elements.
    let (out, _e, _c) = run("a=(1 2)\nprintf '<%s>' \"${a[@]+\"${a[@]}\"}\"\necho\n");
    assert_eq!(out, "<1><2>\n", "out: {out}");
}
```

- [ ] **Step 2: Run the tests to confirm the unquoted ones FAIL**

Run: `cargo test --test mise_zero_errors_integration 2>&1 | tail -20`
Expected: `unquoted_empty_alt_no_spurious_field` and `empty_array_alt_no_spurious_field` FAIL (huck prints `3`, expected `2`); the three quoted/idiom tests PASS.

- [ ] **Step 3: Make the `Empty` arm quoted-aware**

In `src/expand.rs` (`~:898`), change:
```rust
                    crate::param_expansion::ExpansionResult::Empty => {
                        has_emitted = true;
                    }
```
to:
```rust
                    crate::param_expansion::ExpansionResult::Empty => {
                        // A QUOTED empty expansion (`"${u+x}"` when unset) still
                        // contributes one empty field; an UNQUOTED one vanishes
                        // (contributes no field), matching bash. Setting
                        // has_emitted unconditionally injected a spurious empty
                        // field for unquoted `${x+alt}` / `${arr[@]+…}` (M-105).
                        if *quoted {
                            has_emitted = true;
                        }
                    }
```
Do NOT touch the `expand_assignment()` Empty arm (`~:1015`, already `=> {}`).

- [ ] **Step 4: Run the Part-B tests**

Run: `cargo test --test mise_zero_errors_integration 2>&1 | tail -10`
Expected: all Part-B tests PASS (and the Task-1 Part-A tests still PASS — 9 total in the file).

- [ ] **Step 5: Verify byte-identical to bash**

```bash
cargo build --bin huck
for f in 'set -- ${u+X} a b; echo $#' \
         'f=(); set -- ${f[@]+"${f[@]}"} -s bash; echo $#' \
         'set -- "${u+x}" a; echo $#' \
         'printf "<%s>" "${u+x}"; echo' \
         'a=(1 2); printf "<%s>" "${a[@]+"${a[@]}"}"; echo' \
         'set -- ${u-} a b; echo $#'; do
  b=$(printf '%s\n' "$f" | bash --norc --noprofile 2>&1; echo "EXIT:$?")
  h=$(printf '%s\n' "$f" | ./target/debug/huck 2>&1; echo "EXIT:$?")
  [ "$b" = "$h" ] && echo "MATCH: $f" || { echo "DIFF: $f"; echo " b=[$b]"; echo " h=[$h]"; }
done
```
Expected: six `MATCH` lines.

- [ ] **Step 6: Full regression — this touches the core field-emission path**

Run: `cargo test 2>&1 | grep -E "test result: FAILED" ; cargo test 2>&1 | grep -cE "test result: ok"`
Expected: NO `FAILED` lines; the suite is green. If any test regresses, the `Empty` arm changed an expansion that other tests pin — investigate that specific test against bash before proceeding (do NOT mask a real regression). Also run `cargo clippy --bin huck 2>&1 | tail -3` → clean.

- [ ] **Step 7: Commit**

```bash
git add src/expand.rs tests/mise_zero_errors_integration.rs
git commit -m "$(cat <<'EOF'
fix: unquoted ${x+alt} yielding nothing emits no field (M-105)

expand()'s ExpansionResult::Empty arm set has_emitted=true unconditionally, so
an UNQUOTED parameter substitution that expanded to nothing (${u+X} unset,
${arr[@]+"${arr[@]}"} on an empty array) injected a spurious empty field —
`set -- ${u+X} a b; echo $#` gave 3 vs bash 2, and broke mise hook-env with an
empty '' arg. The arm is now quoted-aware: a quoted empty still emits one field,
an unquoted one vanishes, matching bash. Pre-existing scalar bug exposed for
arrays by v109's M-87.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 2 report
DONE/BLOCKED, commit SHA, the exact arm code, test names + pass line, the six MATCH lines, the full-suite green confirmation (no FAILED), clippy status. Flag any regressed test.

---

## Task 3: 34th bash-diff harness + payoff smoke

**Files:**
- Create: `tests/scripts/mise_zero_errors_diff_check.sh`

- [ ] **Step 1: Write the harness**

Create `tests/scripts/mise_zero_errors_diff_check.sh`, modeled on `tests/scripts/bashrc_zero_errors_diff_check.sh` (same `set -u`, `HUCK_BIN`, `check()` combined-stdout+stderr+exit pattern):
```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v110: the last 2 mise-activation leaks.
#   * M-90 combined `>file 2>&1` — a builtin's error follows a redirected stdout
#     file (mise line 29 `declare -p chpwd_functions >/dev/null 2>&1`).
#   * M-105 — an unquoted `${x+alt}` that expands to nothing emits NO field
#     (was a spurious empty arg, breaking `mise hook-env`).
# Each fragment's combined stdout+stderr+exit is compared verbatim.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash --norc --noprofile 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# --- M-90 combined `>file 2>&1` ---
check "combined >/dev/null 2>&1 suppress" 'declare -p NOPE >/dev/null 2>&1; echo ok'
check "bare 2>&1 into pipe"               '{ declare -p NOPE 2>&1; } | grep -c NOPE'
check "file 2>/dev/null still suppress"   'declare -p NOPE 2>/dev/null; echo ok'
check "combined rc preserved"             'declare -p NOPE >/dev/null 2>&1 && echo yes || echo no'

# --- M-105 unquoted ${x+alt} spurious empty field ---
check "unquoted +alt no spurious field"   'set -- ${u+X} a b; echo $#'
check "empty-array +idiom no spurious"    'f=(); set -- ${f[@]+"${f[@]}"} -s bash; echo $#'
check "quoted empty still one field"      'set -- "${u+x}" a; echo $#'
check "quoted empty printf field"         'printf "<%s>" "${u+x}"; echo'
check "set array idiom unchanged"         'a=(1 2); printf "<%s>" "${a[@]+"${a[@]}"}"; echo'
check "unquoted -default still vanishes"  'set -- ${u-} a b; echo $#'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 2: Make it executable and run it**

Run: `chmod +x tests/scripts/mise_zero_errors_diff_check.sh && cargo build --bin huck && bash tests/scripts/mise_zero_errors_diff_check.sh`
Expected: `Total: 10, Pass: 10, Fail: 0`. If any FAIL, the fragment is not byte-identical — fix the underlying code (Task 1/2) or, if the fragment itself is non-deterministic, replace it with a deterministic one (do NOT weaken a real divergence into a passing test).

- [ ] **Step 3: Run ALL harnesses (confirm 34 total, all green)**

Run:
```bash
export HUCK_BIN="$(pwd)/target/debug/huck"
echo "count: $(ls tests/scripts/*_diff_check.sh | wc -l)"
for f in tests/scripts/*_diff_check.sh; do bash "$f" >/dev/null 2>&1 || echo "FAIL $f"; done
echo "done"
```
Expected: `count: 34`; no `FAIL` lines (silent = all pass).

- [ ] **Step 4: Payoff smoke — the gate**

Run (records the before/after error count):
```bash
if command -v mise >/dev/null 2>&1; then
  mise activate bash > /tmp/v110_mise.sh 2>/dev/null
  ./target/debug/huck -c 'source /tmp/v110_mise.sh' 2>/tmp/v110_err.txt
  echo "mise activate error lines: $(grep -c . /tmp/v110_err.txt)"
  cat /tmp/v110_err.txt
else
  echo "mise not installed — synthetic smoke:"
  ./target/debug/huck -c '__MISE_FLAGS=(); declare -p chpwd_functions >/dev/null 2>&1; set -- ${__MISE_FLAGS[@]+"${__MISE_FLAGS[@]}"} -s bash; printf "args=%s\n" "$#"' 2>&1
fi
```
Expected: **0 error lines** from `mise activate` (v109 showed 4). If a real `mise` is present and any error remains, report it verbatim — it is either a NEW gap (log it) or a Task 1/2 miss (fix before proceeding). The synthetic fallback must print `args=2` with no `declare`/`unexpected argument` errors.

- [ ] **Step 5: Commit**

```bash
git add tests/scripts/mise_zero_errors_diff_check.sh
git commit -m "$(cat <<'EOF'
test: 34th bash-diff harness for v110 zero-error mise activate (M-90/M-105)

10 byte-identical bash<->huck fragments: builtin `>file 2>&1`/bare `2>&1`/file
redirect + unquoted `${x+alt}` field-count (spurious-empty-field gone, quoted
empty still one field, set-array idiom unchanged).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 3 report
DONE/BLOCKED, commit SHA, the `Total: 10, Pass: 10` line, the `count: 34` + no-FAIL all-harness line, and the **payoff smoke error-line count (before 4 → after ?)** verbatim.

---

## Task 4: Documentation

**Files:** `docs/bash-divergences.md`, `README.md`.

- [ ] **Step 1: Read the structures to update**

```bash
grep -n 'Last updated:\|M-90: builtin\|### M-105\|Bugs (Tier 1) |\|^## Change log\|2026-06-08.*v109' docs/bash-divergences.md | head
grep -n 'v109' README.md | head
```
Read the M-90 entry (`[fixed v109 partial]`), the M-105 entry (`### M-105 …`, `[deferred]`), the Tier-1 summary row, the v109 change-log entry, and the v109 README row.

- [ ] **Step 2: Flip M-90 and M-105 to `[fixed v110]`**

In `docs/bash-divergences.md`:
- M-90 entry: change `[fixed v109 partial]` → `[fixed v110]`. In its "Still deferred (to v110)" text, the combined `>/dev/null 2>&1` item is now FIXED — rewrite to say: the combined `>file 2>&1` case now dups the **redirected stdout file's fd** onto fd 2 (`prepare_builtin_stderr` takes an `Option<RawFd>` target; both builtin arms select file-fd / real-fd-1 / None). Keep the capture-mode `$(builtin 2>&1)` (L-25) and the `2>&1 >out` ordering as the remaining documented residuals.
- M-105 entry (`### M-105 …`): change Status `[deferred]` → `[fixed v110]`. Update the body: the fix is `expand()`'s `Empty` arm made quoted-aware (`if *quoted { has_emitted = true }`) — an unquoted empty PE now contributes no field, a quoted empty still contributes one. Keep the **converse** (`${u+"$u"}` set-but-null) as the remaining deferred sub-divergence on this entry.

- [ ] **Step 3: Update Tier-1 count + summary note**

In the Summary table `Bugs (Tier 1)` row: change `17` → `17` (count unchanged — M-105 stays a Tier-1 entry, now fixed) and edit the note `M-105 … added v109, deferred to v110` → `M-105 unquoted ${x+alt} spurious-empty-field fixed v110`. In the `Missing features (Tier 2)` note, append after the v109 M-90 clause: `; M-90 combined >file 2>&1 fixed v110` (the M-90 backlog mention).

- [ ] **Step 4: Add the change-log entry + README row**

`docs/bash-divergences.md` change log (after the v109 entry): a `2026-06-08` v110 entry covering both parts (the `Option<RawFd>` dup-target generalization + the quoted-aware `Empty` arm), the **payoff** (`mise activate bash` through huck now emits 0 errors, was 4), the 34th harness + integration tests + test count from Task 3's full-suite run, and the remaining residuals (capture-mode L-25, `2>&1 >out` ordering, converse-M-105). Add a v110 README iteration row after v109 in the same compact style. Use the REAL test count (run `cargo test 2>&1 | awk '/test result:/{s+=$4} END{print s}'`).

- [ ] **Step 5: Verify no placeholders, then commit**

```bash
grep -n 'M-105\|fixed v110\|v110' docs/bash-divergences.md README.md | head
git add docs/bash-divergences.md README.md
git commit -m "$(cat <<'EOF'
docs: v110 — zero-error mise activate (M-90 combined + M-105 fixed)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

### Task 4 report
DONE/BLOCKED, commit SHA, the `grep` output proving real M-numbers/version (no placeholders), and the test count used.

---

## Final (after all tasks)
- [ ] Whole-branch review: `git log --oneline main..HEAD`, `git diff --stat main..HEAD`.
- [ ] `cargo test 2>&1 | grep -cE 'test result: ok'` (green, no FAILED), `cargo clippy --all-targets 2>&1 | tail -2` (clean).
- [ ] All harnesses: `export HUCK_BIN="$(pwd)/target/debug/huck"; for f in tests/scripts/*_diff_check.sh; do bash "$f" >/dev/null 2>&1 || echo "FAIL $f"; done` (silent = pass; 34 files).
- [ ] **Payoff gate**: re-run Task 3 Step 4 — `mise activate` through huck = **0** error lines.
- [ ] AskUserQuestion merge gate, then `git merge --no-ff` + push + delete branch, then update memory files (`project_huck_iterations.md` + `MEMORY.md`).
