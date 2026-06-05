# `command CMD` Bare Form Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `command CMD [args]` runs CMD suppressing shell-function/alias lookup (builtins + `$PATH` still resolve). Unblocks nvm.sh's 167 `command …` uses.

**Architecture:** Intercept `command` in `run_exec_single` (the executor's simple-command dispatch), before the resolution chain: scan flags (`-v`/`-V` → leave to the existing `builtin_command` introspection; `-p` accepted; bare CMD → rewrite `resolved.program`/`args`, set `bypass_functions`), then gate ONLY the function-lookup arm on `!bypass_functions`. Builtin + PATH-exec arms unchanged.

**Tech Stack:** Rust (binary crate `huck`). Unit `cargo test --bin huck`; integration `cargo test --test <name>`; bash-diff harness under `tests/scripts/`.

---

## File Structure

- `src/executor.rs` — `run_exec_single`: `command`-bare-form interception + gate the function arm.
- `src/builtins.rs` — unchanged behavior (keep `builtin_command` for `-v`/`-V`/no-args); review the `command_bare_form_errors` unit test.
- `tests/command_bare_form_integration.rs`, `tests/scripts/command_bare_form_diff_check.sh` — NEW.
- `docs/bash-divergences.md`, `README.md` — M-85 `[fixed v99]` + sub-divergence notes + changelog + README row.

---

### Task 1: `command CMD` bare form (executor interception)

**Files:** `src/executor.rs` (+ the `command_bare_form_errors` unit test in src/builtins.rs if it needs updating)

- [ ] **Step 1: Write the failing integration test**

Create `tests/command_bare_form_integration.rs`:

```rust
//! v99: `command CMD` bare form (bypass function/alias lookup). M-85.
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

fn run(script: &str) -> (String, i32) {
    let mut child = Command::new(huck_bin())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn huck");
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(), out.status.code().unwrap_or(-1))
}

#[test]
fn command_bypasses_function_for_builtin() {
    // A function shadowing the `echo` builtin is bypassed by `command echo`.
    assert_eq!(run("echo() { printf FUNC; }\ncommand echo hi\n").0, "hi\n");
}

#[test]
fn command_bypasses_function_for_external() {
    // A function shadowing an external is bypassed; the real command runs.
    let (_o, rc) = run("true() { return 7; }\ncommand true\necho rc=$?\n");
    assert_eq!(run("true() { return 7; }\ncommand true; echo rc=$?\n").0, "rc=0\n");
    let _ = rc;
}

#[test]
fn command_runs_builtin() {
    assert_eq!(run("command echo hi\n").0, "hi\n");
}

#[test]
fn command_runs_external() {
    assert_eq!(run("command printf '%s\\n' external\n").0, "external\n");
}

#[test]
fn command_double_collapses() {
    assert_eq!(run("command command echo nested\n").0, "nested\n");
}

#[test]
fn command_not_found_127() {
    let (_o, rc) = run("command no_such_cmd_xyz_123\necho rc=$?\n");
    assert_eq!(run("command no_such_cmd_xyz_123 2>/dev/null; echo rc=$?\n").0, "rc=127\n");
    let _ = rc;
}

#[test]
fn command_inline_assignment_applies() {
    // FOO=v command env -> the external sees FOO=v.
    assert_eq!(run("command true && echo ok\n").0, "ok\n");
}

#[test]
fn command_dash_v_unchanged() {
    assert_eq!(run("command -v echo\n").0, "echo\n");
}

#[test]
fn command_no_operand_zero() {
    assert_eq!(run("command\necho rc=$?\n").0, "rc=0\n");
}

#[test]
fn command_dash_p_accepts() {
    assert_eq!(run("command -p echo hi\n").0, "hi\n");
}
```
Before relying on each expected output, run the fragment through bash and confirm (esp. `command -v echo` → bash prints `echo`; `command true` after a `true(){...}` → rc 0). Adjust to bash's actual output. (Clean up the slightly redundant assertions — keep the ones that assert the real behavior.)

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test --test command_bare_form_integration 2>&1 | tail -20`
Expected: FAIL — bare `command echo` currently errors `bare form … not supported`.

- [ ] **Step 3: Add the `command` interception in `run_exec_single`**

In `src/executor.rs`, `run_exec_single` (`:2495`), after `let resolved = match resolve(cmd, shell) { … }` (`:2497`), make `resolved` mutable and add the interception + a `bypass_functions` flag:

```rust
    let mut resolved = match resolve(cmd, shell) { /* … unchanged … */ };

    // `command CMD args` (bare form): run CMD suppressing shell-FUNCTION lookup
    // (builtins + $PATH still resolve). `-v`/`-V` introspection is left to the
    // `command` builtin (not intercepted here).
    let mut bypass_functions = false;
    while resolved.program == "command" {
        // Scan leading flags in resolved.args.
        let mut idx = 0;
        let mut introspect = false;
        loop {
            match resolved.args.get(idx).map(String::as_str) {
                Some("-v") | Some("-V") => { introspect = true; break; }
                Some("-p") => { idx += 1; }            // accept; v99 uses current $PATH
                Some("--") => { idx += 1; break; }
                Some(s) if s.starts_with('-') && s.len() > 1 => {
                    eprintln!("huck: command: {s}: invalid option");
                    return ExecOutcome::Continue(2);
                }
                _ => break,                            // first operand (or end)
            }
        }
        if introspect {
            break; // leave program=="command" -> dispatch runs builtin_command (-v/-V)
        }
        // Bare form: the operand at `idx` (if any) becomes the new program.
        match resolved.args.get(idx) {
            None => return ExecOutcome::Continue(0),   // `command` / `command -p` alone
            Some(_) => {
                let new_program = resolved.args[idx].clone();
                let new_args = resolved.args[idx + 1..].to_vec();
                resolved.program = new_program;
                resolved.args = new_args;
                resolved.decl_args = None;             // inner CMD is not the outer `command`'s decl form
                bypass_functions = true;
                // loop: collapse `command command …`
            }
        }
    }
```
Notes:
- `resolved.decl_args` was computed for the outer `command` (always `None` for `command` since it isn't a declaration command) — set it to `None` after rewrite so the inner CMD takes the normal (non-decl) path. (`command declare …` compound-RHS is a documented edge per the spec; the common nvm case is externals.)
- The `while` collapses `command command ls`; each pass re-scans flags.

- [ ] **Step 4: Gate the function-lookup arm**

At `src/executor.rs:2552`, change:
```rust
    } else if let Some(body) = shell.functions.get(&resolved.program).cloned() {
```
to:
```rust
    } else if !bypass_functions && let Some(body) = shell.functions.get(&resolved.program).cloned() {
```
(If `let`-chains in `else if` aren't enabled on this toolchain, restructure: `} else if !bypass_functions && shell.functions.contains_key(&resolved.program) { let body = shell.functions.get(&resolved.program).cloned().unwrap(); … }` — match the existing style.) Leave the control-builtin, builtin, and PATH-exec arms unchanged.

- [ ] **Step 5: Review the `command_bare_form_errors` unit test**

`src/builtins.rs:~8403` `command_bare_form_errors` asserts the OLD "bare form errors" behavior by calling `builtin_command` directly. Since interception now happens in the executor (the builtin still errors when called directly), decide:
- Either repurpose it to document the defensive path: rename to `command_builtin_bare_form_still_errors_when_called_directly` with a comment that `run_exec_single` intercepts the bare form before the builtin is reached, OR
- Remove it and rely on the integration tests.
Pick one; ensure the suite is consistent. Keep all `command -v`/`-V` unit tests (`command_dash_v_*`) passing unchanged.

- [ ] **Step 6: Build + run integration + full suite + clippy**

Run: `cargo build --bin huck && cargo test --test command_bare_form_integration 2>&1 | tail -20` (all pass).
Run: `cargo test --bin huck 2>&1 | tail -5` and `cargo test 2>&1 | grep -E 'test result' | grep -v 'ok\.' | head` (no failures — especially the `command -v`/`-V` tests + general dispatch).
Run: `cargo clippy --all-targets 2>&1 | tail -3` (clean).
Manual: `printf 'echo() { printf FUNC; }\ncommand echo hi\n' | ./target/debug/huck` → `hi` (function bypassed). `printf 'command sort <<EOF\nb\na\nEOF\n' | ./target/debug/huck` → `a\nb` (external).

- [ ] **Step 7: Commit**

```bash
git add src/executor.rs src/builtins.rs tests/command_bare_form_integration.rs
git commit -m "feat: command CMD bare form — bypass function lookup, run builtin/PATH (M-85)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```
Trailer mandatory/canonical, exactly as shown.

---

### Task 2: bash-diff harness (24th)

**Files:** `tests/scripts/command_bare_form_diff_check.sh` (NEW)

- [ ] **Step 1: Create the harness**

Mirror `tests/scripts/dbracket_multiline_diff_check.sh`'s `check` helper. Deterministic fragments:
```bash
check "bypass builtin"  'echo() { printf FUNC; }; command echo hi'
check "run builtin"     'command echo hi'
check "run external"    "command printf '%s\\n' ext"
check "double command"  'command command echo nested'
check "not found"       'command no_such_cmd_zzz 2>/dev/null; echo $?'
check "bypass external" 'true() { return 7; }; command true; echo $?'
check "dash-v builtin"  'command -v echo'
check "no operand"      'command; echo $?'
check "dash-p"          'command -p echo hi'
check "external sorted" $'command sort <<EOF\nb\na\nc\nEOF'
```
Run each through bash first; confirm well-formed (the `echo(){...}; command echo` fragment relies on `command` finding the echo BUILTIN — verify bash prints `hi` not `FUNC`).

- [ ] **Step 2: Run the harness**

Run: `cargo build --bin huck && bash tests/scripts/command_bare_form_diff_check.sh 2>&1 | tail -20`
Expected: every line PASS, `Fail: 0`. Investigate any FAIL (bash is the oracle); real bug → STOP and report.

- [ ] **Step 3: Commit**

```bash
git add tests/scripts/command_bare_form_diff_check.sh
git commit -m "test: bash-diff harness for command CMD bare form (24th)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```
Trailer mandatory/canonical, exactly as shown.

---

### Task 3: Documentation

**Files:** `docs/bash-divergences.md`, `README.md`

- [ ] **Step 1: Read structure**

`grep -n 'M-85\|command\|^## Change log\|Missing features (Tier 2)\|Low-impact\|2026-06-05\|^- \*\*L-[0-9]' docs/bash-divergences.md | head -25` and `grep -n '| v9' README.md`. Match v97/v98 style; the M-70 (`command -v/-V`) entry references bare-form as deferred — update it.

- [ ] **Step 2: Flip M-85 to fixed**

Update M-85 from `[deferred]` to `[fixed v99]`: bare `command CMD args` runs CMD suppressing shell-function lookup (builtins + `$PATH` resolve), via interception in `run_exec_single` + a `bypass_functions` gate on the function arm; `command command …` collapses; `-p` accepted (current `$PATH`). Cross-reference the M-70 entry (its "bare form … deferred" note → now fixed). Bump the Tier-2 count.

- [ ] **Step 3: Low-impact note(s)**

Add an `L-` note (next free) for: (a) `command -p` uses the current `$PATH` rather than bash's guaranteed default PATH (`getconf PATH`); (b) `command <declaration-builtin>` with compound RHS (`command declare a=(…)`) is best-effort (the outer `command`'s decl-arg pre-parse doesn't re-apply). `[intentional]`/low. Bump the Tier-4 count.

- [ ] **Step 4: Change-log + README row**

`2026-06-05` v99 change-log entry (the interception mechanism; bypass-functions; `-p` accept; nvm.sh — 167 `command …` uses now run; the L-note sub-divergences; 24th harness). v99 README row after v98.

- [ ] **Step 5: Verify + commit**

`grep -n 'v99\|fixed v99\|M-85' docs/bash-divergences.md README.md` (confirm, no placeholders).
```bash
git add docs/bash-divergences.md README.md
git commit -m "docs: v99 command bare form fixed (M-85) — changelog, README, L-note

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```
Trailer mandatory/canonical, exactly as shown.

---

## Self-Review

- **Spec coverage:** §1 executor interception → Task 1; §2 builtin unchanged → Task 1 Step 5; testing → Tasks 1/2; M-85 flip + sub-divergences → Task 3. Covered.
- **Placeholder scan:** none — the interception block is shown in full; the function-arm gate is a one-line change at the named site.
- **Type consistency:** `resolved: ResolvedCommand { program: String, args: Vec<String>, decl_args, … }` made `mut`; `bypass_functions: bool`; gate at executor.rs:2552. Reuses the unchanged control-builtin/builtin/PATH-exec arms.
- **Edge cases:** `command` finds builtins (ungated builtin arm); `command command` collapses (while loop); not-found→127 (PATH-exec arm); `-v`/`-V` unchanged (introspect break); no-operand→0; inline assignments/redirects bind to rewritten `resolved`; `decl_args` reset on rewrite; non-`command` dispatch byte-unchanged.
