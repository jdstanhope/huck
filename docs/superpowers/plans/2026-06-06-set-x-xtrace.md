# `set -x` (xtrace) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `set -x`/`+x`/`-o xtrace` print each expanded command to stderr (prefixed by `$PS4`, default `+ `) before running it; `x` shows in `$-`. Becomes the diagnostic tool for the nvm hang (v104).

**Architecture:** Mirror v89's `verbose`: a `ShellOptions.xtrace` flag + `$-` letter + `option_get/set` + `builtin_set` `-x`/`+x`; trace emission in `run_exec_single` after `resolve` (expanded) and before dispatch. Flat `$PS4` (depth-repeat + PS4 expansion deferred).

**Tech Stack:** Rust (binary crate `huck`). Unit `cargo test --bin huck`; integration `cargo test --test <name>`; bash-diff harness under `tests/scripts/`.

---

## File Structure

- `src/shell_state.rs` — `ShellOptions.xtrace`; `dollar_dash_value` pushes `x`.
- `src/builtins.rs` — `option_get`/`option_set` `xtrace` arms; `builtin_set` `-x`/`+x`.
- `src/executor.rs` — `run_exec_single`: emit trace when `xtrace`, before dispatch.
- `tests/set_x_integration.rs`, `tests/scripts/set_x_diff_check.sh` — NEW.
- `docs/bash-divergences.md`, `README.md` — M-08 `-x` fixed note + divergences + changelog + README row.

---

### Task 1: Option plumbing + trace emission + tests

**Files:** `src/shell_state.rs`, `src/builtins.rs`, `src/executor.rs`, `tests/set_x_integration.rs` (NEW)

- [ ] **Step 1: Write the failing integration test**

Create `tests/set_x_integration.rs` (the helper must capture STDERR separately):

```rust
//! v103: set -x (xtrace).
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

#[test]
fn traces_simple_command() {
    let (so, se, _) = run("set -x\necho hi\n");
    assert_eq!(so, "hi\n");
    assert!(se.contains("+ echo hi"), "stderr={se:?}");
}

#[test]
fn traces_expanded_form() {
    let (so, se, _) = run("x=hi\nset -x\necho \"$x\" a\n");
    assert_eq!(so, "hi a\n");
    assert!(se.contains("+ echo hi a"), "stderr={se:?}");
}

#[test]
fn enabling_line_not_traced_disabling_is() {
    let (_so, se, _) = run("set -x\necho a\nset +x\necho b\n");
    assert!(se.contains("+ echo a"), "stderr={se:?}");
    assert!(se.contains("+ set +x"), "stderr={se:?}");
    assert!(!se.contains("+ echo b"), "echo b should NOT be traced: {se:?}");
}

#[test]
fn traces_inside_function() {
    let (_so, se, _) = run("f() { echo in; }\nset -x\nf\n");
    assert!(se.contains("+ f"), "stderr={se:?}");
    assert!(se.contains("+ echo in"), "stderr={se:?}");
}

#[test]
fn dollar_dash_has_x() {
    let (so, _se, _) = run("set -x\ncase \"$-\" in *x*) echo on;; *) echo off;; esac\n");
    assert_eq!(so, "on\n");
}

#[test]
fn xtrace_to_stderr_not_captured() {
    let (so, _se, _) = run("r=$(set -x; echo cap)\necho \"[$r]\"\n");
    assert_eq!(so, "[cap]\n");
}

#[test]
fn set_o_xtrace_form() {
    let (_so, se, _) = run("set -o xtrace\necho hi\n");
    assert!(se.contains("+ echo hi"), "stderr={se:?}");
}
```
Verify each against bash first (`printf '…' | bash` — note bash sends the trace to stderr; capture with `2>&1` or separately). Adjust the expected trace strings to bash's actual form (esp. quoting — bash may render `+ echo hi a` without quotes; confirm).

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test --test set_x_integration 2>&1 | tail -20`
Expected: FAIL — `set -x` currently errors `not yet supported`.

- [ ] **Step 3: `ShellOptions.xtrace` + `$-`**

In `src/shell_state.rs`: add `pub xtrace: bool,` to `struct ShellOptions` (`:107`). In `dollar_dash_value` (`:~402`), add after the `verbose`→`'v'` line:
```rust
        if self.shell_options.xtrace { out.push('x'); }
```
(Keep alphabetical order: e, i, u, v, x.)

- [ ] **Step 4: `option_get`/`option_set` + `builtin_set` short flags**

In `src/builtins.rs`:
- `option_get` (`:~3948`): add `"xtrace" => Some(shell.shell_options.xtrace),`.
- `option_set` (`:~3961`): add `"xtrace" => { shell.shell_options.xtrace = value; Ok(()) }`.
- `builtin_set` short-flag handling: add `b'x'` wherever `b'v'` (verbose) is wired, for BOTH `-x` (enable) and `+x` (disable). Grep `b'v'` / `verbose` in `builtin_set` and replicate the pattern exactly (including the `+`-prefix path that handles `set +v`). The `-x` enable goes in the `-`-cluster loop (`src/builtins.rs:~4063`, beside `b'v' => shell.shell_options.verbose = true`).

- [ ] **Step 5: Trace emission in `run_exec_single`**

In `src/executor.rs`, `run_exec_single`, after `let resolved = …resolve…` AND after the v99 `command`-bare-form interception block, BEFORE the dispatch chain (control-builtin/function/builtin/PATH), add:
```rust
    if shell.shell_options.xtrace {
        let ps4 = shell.lookup_var("PS4").unwrap_or_else(|| "+ ".to_string());
        let mut line = String::new();
        if resolved.program.is_empty() {
            // Pure assignment / redirect-only: render the applied assignments
            // without re-expanding (read back the values just set).
            let mut first = true;
            for a in &cmd.inline_assignments {
                if !first { line.push(' '); }
                first = false;
                let n = a.target.name();
                let v = shell.lookup_var(n).unwrap_or_default();
                line.push_str(&format!("{n}={v}"));
            }
        } else {
            line.push_str(&resolved.program);
            for a in &resolved.args { line.push(' '); line.push_str(a); }
        }
        eprintln!("{ps4}{line}");
    }
```
Notes for the implementer:
- This MUST be placed where `resolved` (expanded) is available and BEFORE the command runs (so a hanging command is traced first). If the `command`-interception or inline-assignment snapshot reorders things, ensure the trace fires after expansion+assignment-apply but before dispatch — match the spec's intent; verify with `set -x; sleep 0` shows `+ sleep 0` before completion.
- Use `resolved.program`/`resolved.args` (already expanded) — do NOT re-expand.
- The inline-assignment PREFIX on a command-with-program (`VAR=v cmd`) is OMITTED in this version (trace shows just `cmd`); the pure-assignment case IS handled above. (Document this as a minor divergence in Task 3.) If including the prefix cleanly (from applied values, no re-expansion) is easy, you MAY add it — but do NOT re-run command substitutions.
- Confirm `cmd.inline_assignments` / `a.target.name()` are the correct field/accessor (grep the `SimpleCommand`/`ExecCommand` + `Assignment` shapes); adapt if names differ.

- [ ] **Step 6: Unit tests**

Add unit tests (where the existing `set`/`$-` unit tests live): `set -x` sets `shell_options.xtrace`; `dollar_dash_value` contains `x` after `set -x`; `set +x` clears it; `option_set("xtrace", true/false)` works.

- [ ] **Step 7: Build + run integration + full suite + clippy**

Run: `cargo build --bin huck && cargo test --test set_x_integration 2>&1 | tail -20` (all pass).
Run: `cargo test --bin huck 2>&1 | tail -5` and `cargo test 2>&1 | grep -E 'test result' | grep -v 'ok\.' | head` (no failures — ESPECIALLY set/`$-`/verbose-v89/set_verbose suites; xtrace default off must change nothing).
Run: `cargo clippy --all-targets 2>&1 | tail -3` (clean).
Manual: `printf 'set -x\necho hi\n' | ./target/debug/huck 2>&1` → `+ echo hi` then `hi`.

- [ ] **Step 8: Commit**

```bash
git add src/shell_state.rs src/builtins.rs src/executor.rs tests/set_x_integration.rs
git commit -m "feat: set -x (xtrace) — trace expanded commands to stderr with \$PS4 (M-08)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```
Trailer mandatory/canonical, exactly as shown.

---

### Task 2: bash-diff harness (28th)

**Files:** `tests/scripts/set_x_diff_check.sh` (NEW)

- [ ] **Step 1: Create the harness**

Mirror `tests/scripts/dbracket_multiline_diff_check.sh`'s `check` helper (it
already compares combined stdout+stderr+`EXIT:$?` — perfect, since xtrace is on
stderr). Use ONLY top-level, depth-1, default-`PS4` fragments where huck and bash
agree:
```bash
check "trace echo"     'set -x; echo hi'
check "trace expanded" 'x=hi; set -x; echo "$x" a'
check "enable disable" 'set -x; echo a; set +x; echo b'
check "dash has x"     'set -x; case "$-" in *x*) echo on;; *) echo off;; esac'
check "set -o xtrace"  'set -o xtrace; echo hi'
check "trace true"     'set -x; true; set +x; echo done'
check "trace two args" 'set -x; printf "%s\n" one'
```
After writing, RUN it and confirm each fragment is well-formed AND that bash's
trace form matches huck's for that fragment. IMPORTANT: bash may quote/format the
expanded trace differently for some inputs (e.g. it re-quotes args containing
spaces as `'a b'`). Pick fragments whose trace is unambiguous (`echo hi`, single
plain args). If a chosen fragment's trace genuinely differs between bash and huck
for a TOP-LEVEL simple command (not a PS4-depth/quoting edge), STOP and report —
that's a real divergence to fix in Task 1. If it's a known quoting-of-args
nuance, replace with a no-special-char fragment and note it.

- [ ] **Step 2: Run the harness**

Run: `cargo build --bin huck && bash tests/scripts/set_x_diff_check.sh 2>&1 | tail -20`
Expected: every line PASS, `Fail: 0`.

- [ ] **Step 3: Commit**

```bash
git add tests/scripts/set_x_diff_check.sh
git commit -m "test: bash-diff harness for set -x xtrace (28th)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```
Trailer mandatory/canonical, exactly as shown.

---

### Task 3: Documentation

**Files:** `docs/bash-divergences.md`, `README.md`

- [ ] **Step 1: Read structure**

`grep -n 'M-08\b\|xtrace\|^## Change log\|Missing features (Tier 2)\|2026-06-0\|^- \*\*L-[0-9]' docs/bash-divergences.md | head` and `grep -n '| v10' README.md`. Read the M-08 entry (`:134`) + recent change-log/README rows. Next free `L-` number (highest is L-20).

- [ ] **Step 2: Update M-08 + record divergences**

In the M-08 entry: move `-x` from "Still deferred" to a `[fixed v103]` note — `set -x`/`+x`/`-o xtrace` trace each expanded command to stderr with `$PS4` (default `+ `) before execution; `x` in `$-`. Keep the remaining `-n`/`-f`/`-a`/`-C`/`-b`/`-h` deferred. Add an `L-` note (next free) for the v103 xtrace divergences: flat `$PS4` (no nesting-depth char-repeat, no `$PS4` escape/var expansion); the inline-assignment PREFIX on `VAR=v cmd` is omitted from the trace; finer compound-internal traces (for-iteration var, `(( ))`, `[[ ]]`) not emitted; and (per M-90) `2>/dev/null` doesn't suppress the trace. `[intentional]`/low. Bump the Tier-4 count.

- [ ] **Step 3: Change-log + README row**

`2026-06-06` v103 change-log entry mirroring v101/v102 style (the `ShellOptions.xtrace` + `$-` + `set -x`/`-o xtrace` + the `run_exec_single` trace point; flat `$PS4`; the diagnostic motivation — pinpointing the nvm `nvm_ls_current` hang, to be used in v104; 28th harness). v103 README row after v102.

- [ ] **Step 4: Verify + commit**

`grep -n 'v103\|fixed v103\|xtrace' docs/bash-divergences.md README.md` (confirm, no placeholders).
```bash
git add docs/bash-divergences.md README.md
git commit -m "docs: v103 set -x xtrace (M-08) — changelog, README, L-note

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```
Trailer mandatory/canonical, exactly as shown.

---

## Self-Review

- **Spec coverage:** §1 plumbing → Task 1 Steps 3-4; §2 trace → Task 1 Step 5; testing → Tasks 1/2; M-08 update + divergences → Task 3. Covered.
- **Placeholder scan:** none — the field, `$-`, option arms, and the trace block are shown; the `b'x'` short-flag is "mirror `b'v'`" with the exact site named.
- **Type consistency:** `ShellOptions.xtrace: bool`; `option_get/set` `"xtrace"` arms; trace uses `resolved.program`/`resolved.args` + `cmd.inline_assignments` + `shell.lookup_var("PS4")`; `eprintln!` to stderr.
- **Edge cases:** default off ⇒ zero change; enabling line not traced (xtrace was off when read), disabling line traced; capture-mode trace still on stderr; PS4 depth/expansion + inline-prefix deferred (documented); emit BEFORE execution so a hang is traced.
