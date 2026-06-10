# v126 — Command-Substitution Exit Status in a Bare Assignment (`$?` after `VAR=$(cmd)`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A bare assignment command's `$?` becomes the exit status of the last command substitution in its RHS (or 0 if none) — fixing nvm's `→ N/A`.

**Architecture:** Add a `Shell.last_cmd_sub_status: Option<i32>` field that `run_substitution` records on every command substitution; `run_single`'s `SimpleCommand::Assign` arm resets it per-command and uses it (`unwrap_or(0)`) as the return status on success. The `Exec` path (`local`/`declare`/assignment-prefix-to-a-command) never reads it, so those statuses are unchanged.

**Tech Stack:** Rust.

Spec: `docs/superpowers/specs/2026-06-10-cmdsub-assign-status-design.md`.

**Conventions:**
- Build/test: `cargo build` (debug) → `target/debug/huck`; harness uses it.
- Commit trailer EXACTLY (keep "(1M context)"): `Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`
- Bash-diff harness fragments run as FILE-ARG scripts (L-27).
- Branch: `v126-cmdsub-assign-status` (from `main` before Task 1).

---

## File Structure

| File | Responsibility | Tasks |
|------|----------------|-------|
| `src/shell_state.rs` | `last_cmd_sub_status: Option<i32>` field on `Shell` | 1 |
| `src/expand.rs` | `run_substitution` records the cmd-sub status; unit test | 1 |
| `src/executor.rs` | `run_single` `Assign` arm uses it for the return status | 1 |
| `tests/cmdsub_assign_status_integration.rs` | NEW — probed cases vs bash | 1 |
| `tests/scripts/cmdsub_assign_status_diff_check.sh` | NEW — 49th harness | 1 |
| `README.md` | harness count 48→49 | 2 |

---

### Task 1: Implement the fix + tests

**Files:**
- Modify: `src/shell_state.rs` (`Shell` struct `:233`; `new()` `:366`)
- Modify: `src/expand.rs` (`run_substitution` `:1174`)
- Modify: `src/executor.rs` (`run_single` `SimpleCommand::Assign` arm `:2665`)
- Create: `tests/cmdsub_assign_status_integration.rs`
- Create: `tests/scripts/cmdsub_assign_status_diff_check.sh`

- [ ] **Step 1: Write the failing integration test**

Create `tests/cmdsub_assign_status_integration.rs` (copy the binary-invocation helper idiom from `tests/builtin_stdout_dup_integration.rs`):
```rust
//! v126: a bare assignment's $? = the last command substitution's exit status
//! in its RHS (or 0 if none). File-arg execution (L-27).

use std::io::Write;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static N: AtomicU64 = AtomicU64::new(0);

fn huck_stdout(frag: &str) -> String {
    let path = std::env::temp_dir().join(format!(
        "huck_v126_{}_{}.sh", std::process::id(), N.fetch_add(1, Ordering::SeqCst)
    ));
    let mut f = std::fs::File::create(&path).expect("create temp script");
    f.write_all(frag.as_bytes()).expect("write temp script");
    drop(f);
    let out = Command::new(env!("CARGO_BIN_EXE_huck")).arg(&path).output().expect("run huck");
    let _ = std::fs::remove_file(&path);
    String::from_utf8_lossy(&out.stdout).trim_end().to_string()
}

#[test]
fn cmdsub_false_status() {
    assert_eq!(huck_stdout("x=$(false); echo $?"), "1");
}
#[test]
fn cmdsub_exit7_status() {
    assert_eq!(huck_stdout("x=$(exit 7); echo $?"), "7");
}
#[test]
fn plain_assign_zero() {
    assert_eq!(huck_stdout("x=5; echo $?"), "0");
}
#[test]
fn two_assigns_last_wins() {
    assert_eq!(huck_stdout("x=$(false) y=$(exit 2); echo $?"), "2");
}
#[test]
fn two_subs_one_rhs_last_wins() {
    assert_eq!(huck_stdout(r#"x="$(false)$(exit 5)"; echo $?"#), "5");
}
#[test]
fn dollar_question_in_rhs_reads_previous_status() {
    // Must-not-regress: $? in the RHS reads the PREVIOUS command's status.
    assert_eq!(huck_stdout("false; x=$?; echo $x"), "1");
}
#[test]
fn local_assign_keeps_builtin_status() {
    // `local v=$(exit 9)` -> $? is local's status (0), not the cmd-sub. (Exec path.)
    assert_eq!(huck_stdout("f(){ local v=$(exit 9); echo $?; }; f"), "0");
}
#[test]
fn assign_prefix_to_command_keeps_command_status() {
    // `x=$(exit 3) true` -> $? is true's status (0). (Exec path.)
    assert_eq!(huck_stdout("x=$(exit 3) true; echo $?"), "0");
}
#[test]
fn append_assign_cmdsub_status() {
    assert_eq!(huck_stdout("x=a; x+=$(exit 4); echo $?"), "4");
}
```
Run: `cargo test --test cmdsub_assign_status_integration 2>&1 | tail -20`
Expected: `cmdsub_false_status`, `cmdsub_exit7_status`, `two_assigns_last_wins`, `two_subs_one_rhs_last_wins`, `append_assign_cmdsub_status` FAIL (huck returns 0); the rest may already pass.

- [ ] **Step 2: Add the `Shell` field**

In `src/shell_state.rs`, add to the `Shell` struct (near `last_status: i32,` at `:235`):
```rust
    /// Exit status of the most recent command substitution, used to give a
    /// bare assignment command (`VAR=$(cmd)`) bash's exit status. Set by
    /// `run_substitution`; read+reset by the `SimpleCommand::Assign` arm.
    last_cmd_sub_status: Option<i32>,
```
Then in `new()` (the `Self { … }` literal around `:380`, near `last_status: 0,`), add:
```rust
            last_cmd_sub_status: None,
```
Make the field accessible from `expand.rs` and `executor.rs`: if `Shell`'s fields are `pub(crate)` or accessed directly elsewhere in those modules, match that visibility. If fields are private and accessed via methods, instead add `pub(crate) fn set_last_cmd_sub_status(&mut self, s: Option<i32>)` and `pub(crate) fn last_cmd_sub_status(&self) -> Option<i32>` accessors and use those in Steps 3-4. CHECK how `last_status` is accessed cross-module (it uses `set_last_status`/`last_status()` accessors — mirror that): add the two accessors and use them.

- [ ] **Step 3: Record the status in `run_substitution`**

In `src/expand.rs`, `run_substitution` (`:1174`) currently:
```rust
pub fn run_substitution(seq: &Sequence, shell: &mut Shell) -> String {
    let mut cloned = shell.clone();
    let (output, status) = executor::execute_capturing(seq, &mut cloned);
    shell.set_last_status(status);
    strip_trailing_newlines(&output)
}
```
Add the record right after `set_last_status`:
```rust
    shell.set_last_status(status);
    shell.set_last_cmd_sub_status(Some(status));   // for bare-assignment exit status (v126)
    strip_trailing_newlines(&output)
```
(Use the accessor if you added one in Step 2; otherwise `shell.last_cmd_sub_status = Some(status);`.)

- [ ] **Step 4: Use it in the `SimpleCommand::Assign` arm**

In `src/executor.rs`, `run_single`'s `SimpleCommand::Assign(items)` arm (`:2665`) currently:
```rust
        SimpleCommand::Assign(items) => {
            let mut st = 0;
            for a in items {
                let name = a.target.name();
                if shell.is_readonly(name) {
                    eprintln!("huck: {name}: readonly variable");
                    st = 1;
                    break;
                }
                if apply_one_assignment(a, shell).is_err() {
                    st = 1;
                    break;
                }
            }
            ExecOutcome::Continue(st)
        }
```
Change to reset before the loop and consult the field on success:
```rust
        SimpleCommand::Assign(items) => {
            // Reset so only THIS assignment's RHS command substitutions count.
            shell.set_last_cmd_sub_status(None);
            let mut st = 0;
            for a in items {
                let name = a.target.name();
                if shell.is_readonly(name) {
                    eprintln!("huck: {name}: readonly variable");
                    st = 1;
                    break;
                }
                if apply_one_assignment(a, shell).is_err() {
                    st = 1;
                    break;
                }
            }
            // bash: a bare assignment's status is the last command substitution
            // in its RHS (or 0 if none). A readonly/apply error keeps st=1.
            if st == 0 {
                st = shell.last_cmd_sub_status().unwrap_or(0);
            }
            ExecOutcome::Continue(st)
        }
```
(Use the accessors if added; otherwise `shell.last_cmd_sub_status = None;` and `shell.last_cmd_sub_status.unwrap_or(0)`.)

- [ ] **Step 5: Add a unit test for the record**

In `src/expand.rs`'s `#[cfg(test)] mod tests` (near `expand_command_sub_updates_parent_last_status`, which uses an `exit_sequence(n)` helper), add:
```rust
#[test]
fn run_substitution_records_last_cmd_sub_status() {
    let mut shell = Shell::new();
    let _ = run_substitution(&exit_sequence(7), &mut shell);
    assert_eq!(shell.last_cmd_sub_status(), Some(7));
}
```
(If `last_cmd_sub_status()` is an accessor you added, use it; if the field is directly visible in-module, assert the field. `exit_sequence`/`Shell::new`/`run_substitution` are all in scope in that test module — confirm by reading the neighboring test.)

- [ ] **Step 6: Build + run unit + integration**

Run: `cargo build 2>&1 | tail -5`
Expected: clean. If the build complains about a missing field in another `Shell` constructor literal, add `last_cmd_sub_status: None,` there too (grep `Shell {` / `Self {` in `shell_state.rs`).
Run: `cargo test --test cmdsub_assign_status_integration 2>&1 | tail -15` (all 9 pass).
Run: `cargo test --lib run_substitution_records 2>&1 | tail -5` (passes).

- [ ] **Step 7: Add the 49th bash-diff harness**

Create `tests/scripts/cmdsub_assign_status_diff_check.sh` (model on `tests/scripts/builtin_stdout_dup_diff_check.sh`):
```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v126: a bare assignment's $? = the last
# command substitution in its RHS (or 0). File-arg execution (L-27).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h tf
    tf=$(mktemp)
    printf '%s\n' "$frag" > "$tf"
    b=$(bash --norc --noprofile "$tf" 2>/dev/null; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tf" 2>/dev/null; echo "EXIT:$?")
    rm -f "$tf"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

check "false sub"        'x=$(false); echo $?'
check "exit7 sub"        'x=$(exit 7); echo $?'
check "plain zero"       'x=5; echo $?'
check "two assigns last" 'x=$(false) y=$(exit 2); echo $?'
check "two subs one rhs" 'x="$(false)$(exit 5)"; echo $?'
check "dollarq snapshot" 'false; x=$?; echo $x'
check "local keeps 0"    'f(){ local v=$(exit 9); echo $?; }; f'
check "prefix keeps cmd" 'x=$(exit 3) true; echo $?'
check "append sub"       'x=a; x+=$(exit 4); echo $?'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```
Run: `chmod +x tests/scripts/cmdsub_assign_status_diff_check.sh && cargo build 2>&1|tail -1 && ./tests/scripts/cmdsub_assign_status_diff_check.sh`
Expected: `Total: 9, Pass: 9, Fail: 0`. If any case diverges, debug the fix (do not weaken the harness).

- [ ] **Step 8: clippy + commit**

Run: `cargo clippy --all-targets 2>&1 | tail -5` (clean).
```bash
git add src/shell_state.rs src/expand.rs src/executor.rs tests/cmdsub_assign_status_integration.rs tests/scripts/cmdsub_assign_status_diff_check.sh
git commit -m "fix(v126): bare assignment \$? = last command-sub status (fixes nvm ->N/A)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Docs + nvm payoff

**Files:**
- Modify: `README.md` (harness count)

- [ ] **Step 1: Verify the nvm payoff (non-interactive — no `N/A`)**

```bash
cargo build 2>&1 | tail -1
printf '. "$HOME/.nvm/nvm.sh"\nnvm alias 2>/dev/null | sed -n "1,4p"\n' > /tmp/v126_nvm.sh
timeout 30 ./target/debug/huck /tmp/v126_nvm.sh
```
EXPECTED: alias destinations now show REAL versions, e.g. `default -> lts/* (-> v24.16.0)`, `lts/* -> lts/krypton (-> v24.16.0)`, `lts/argon -> v4.9.1 (-> N/A)` (the per-alias `N/A` for genuinely-uninstalled versions is CORRECT and matches bash) — NOT `(-> N/A)` for `default`/`lts/*`/`lts/krypton`. Compare against bash:
```bash
timeout 30 bash --norc /tmp/v126_nvm.sh
```
Capture both. The huck output's `(-> …)` column must match bash. If `default` still shows `(-> N/A)`, report BLOCKED with the output.

- [ ] **Step 2: Verify the nvm ls payoff (interactive PTY)**

Build release (`cargo build --release 2>&1 | tail -1`) and run a python PTY harness (reuse v125's pattern) that sources `~/.nvm/nvm.sh`, runs `nvm ls`, and confirms it completes with real versions in the alias section (no `(-> N/A)` for the installed ones, no `∞`). Do NOT source the user's `~/.bashrc` (PG* creds). Capture the alias lines. (The `[N] … Done … &` job-notification noise is EXPECTED to remain — that's L-28, deferred to v127; not a failure here.)

- [ ] **Step 3: Bump the README harness count**

In `README.md`, change "**48 bash-diff harnesses**" → "**49 bash-diff harnesses**". Verify: `grep -n "bash-diff harness" README.md`.

- [ ] **Step 4: Full regression sanity**

```bash
cargo test 2>&1 | grep -E "test result: FAILED|error\[" | head    # none
cargo clippy --all-targets 2>&1 | tail -3                         # clean
for h in tests/scripts/*_diff_check.sh; do bash "$h" >/dev/null 2>&1 || echo "FAIL $h"; done  # silent
```

- [ ] **Step 5: Commit**

```bash
git add README.md
git commit -m "docs(v126): bump harness count to 49

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Final verification (after all tasks)

- [ ] `cargo build` + `cargo clippy --all-targets` clean.
- [ ] `cargo test 2>&1 | grep -E "test result: FAILED|error\["` → none.
- [ ] all 49 harnesses pass.
- [ ] `nvm alias`/`nvm ls`: alias destinations show real versions (no spurious `N/A`); only genuinely-uninstalled versions show `N/A` (matching bash).

## Self-review notes (plan author)
- **Spec coverage:** field → Task 1 Step 2; `run_substitution` record → Step 3; `Assign` arm read → Step 4; unit test → Step 5; integration (all probed cases incl. the `$?`-snapshot must-not-regress, `local`/prefix Exec-path, append) → Step 1; 49th harness → Step 7; docs + payoff → Task 2.
- **Type consistency:** `last_cmd_sub_status: Option<i32>`; accessors `set_last_cmd_sub_status(Option<i32>)` / `last_cmd_sub_status() -> Option<i32>` (mirror `set_last_status`/`last_status()`); used identically in expand.rs + executor.rs.
- **Zero-regression hinges:** the field is reset at the START of the `Assign` arm (so prior cmd-subs don't leak), only read on `st == 0`, and never touches `last_status` (so the `$?`-in-RHS snapshot is unaffected — covered by `dollar_question_in_rhs_reads_previous_status`). `local`/`declare`/prefix go through the `Exec` path which never reads the field (covered by `local_assign_keeps_builtin_status` + `assign_prefix_to_command_keeps_command_status`).
