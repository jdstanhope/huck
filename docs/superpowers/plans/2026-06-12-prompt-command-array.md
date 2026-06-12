# huck v148 — PROMPT_COMMAND array execution Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax. Short single-task iteration.

**Goal:** Run EVERY element of an array `PROMPT_COMMAND` (in index order, non-empty), not just element `[0]`, so oh-my-posh's prompt renders.

**Architecture:** `fire_prompt_command` (src/shell.rs:561) collects the commands to run — an indexed array → each non-empty element in index order (via `get_array`); a scalar → the scalar (unchanged) — then runs each via `process_line`, propagating Exit and updating `$?` from the last.

**Tech Stack:** Rust; `src/shell.rs`.

**Reference:** spec at `docs/superpowers/specs/2026-06-12-prompt-command-array-design.md`.

**GIT SAFETY:** Do NOT `git checkout <sha>`. Stay on `v148-prompt-command-array`. Commit trailer: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

**Verified facts:** `fire_prompt_command(&mut Shell) -> Option<i32>` at src/shell.rs:561. `Shell::get_array(name) -> Option<&BTreeMap<usize,String>>` (Indexed only; scalar/assoc → None). `lookup_var(name) -> Option<&str>`. `set_last_status`. `replace_array(name, BTreeMap<usize,String>) -> Result<(),AssignErr>`. `process_line(line, &mut Shell, expand_aliases: bool) -> ExecOutcome`. Test helper `interactive_shell()` + `shell.get(name) -> Option<&str>` exist in the shell.rs mod tests.

---

### Task 1: array branch in `fire_prompt_command` + tests

**Files:**
- Modify: `src/shell.rs` (`fire_prompt_command` + mod tests)

- [ ] **Step 1: Write the failing tests** — add to `src/shell.rs` `mod tests` (alongside the existing scalar `fire_prompt_command` tests at ~725):
```rust
fn arr(elems: &[&str]) -> std::collections::BTreeMap<usize, String> {
    elems.iter().enumerate().map(|(i, s)| (i, s.to_string())).collect()
}

#[test]
fn array_runs_all_elements_in_order() {
    let mut shell = interactive_shell();
    // literal element strings — NOT pre-expanded; each runs as a command later.
    shell.replace_array("PROMPT_COMMAND", arr(&["ORDER=${ORDER}a", "ORDER=${ORDER}b"])).unwrap();
    assert_eq!(fire_prompt_command(&mut shell), None);
    assert_eq!(shell.get("ORDER"), Some("ab"), "both elements ran, in order");
}

#[test]
fn array_skips_empty_elements() {
    let mut shell = interactive_shell();
    shell.replace_array("PROMPT_COMMAND", arr(&["MA=1", "", "MB=1"])).unwrap();
    assert_eq!(fire_prompt_command(&mut shell), None);
    assert_eq!(shell.get("MA"), Some("1"));
    assert_eq!(shell.get("MB"), Some("1"));
}

#[test]
fn array_propagates_exit_and_stops() {
    let mut shell = interactive_shell();
    shell.replace_array("PROMPT_COMMAND", arr(&["MA=1", "exit 7", "MB=1"])).unwrap();
    assert_eq!(fire_prompt_command(&mut shell), Some(7));
    assert_eq!(shell.get("MA"), Some("1"), "element before exit ran");
    assert_eq!(shell.get("MB"), None, "element after exit did NOT run");
}

#[test]
fn array_last_status_reflects_last_element() {
    let mut shell = interactive_shell();
    shell.replace_array("PROMPT_COMMAND", arr(&["true", "false"])).unwrap();
    assert_eq!(fire_prompt_command(&mut shell), None);
    assert_eq!(shell.last_status(), 1, "last element (false) sets $?");
}
```

- [ ] **Step 2: Run — verify failure**
`cargo test --bin huck array_runs_all_elements 2>&1 | tail -12` (and the other 3) → FAIL: only `[0]` runs (e.g. `ORDER` = `a` not `ab`; `MB` unset). Record.

- [ ] **Step 3: Implement** — replace the body of `fire_prompt_command` (src/shell.rs:561-577):
```rust
pub fn fire_prompt_command(shell: &mut Shell) -> Option<i32> {
    if !shell.is_interactive {
        return None;
    }
    // An indexed-array PROMPT_COMMAND runs each NON-EMPTY element in index order
    // (bash 5.1+); a scalar runs as-is. Collect owned strings first so the
    // immutable borrow ends before the `&mut shell` process_line calls.
    let commands: Vec<String> = if let Some(map) = shell.get_array("PROMPT_COMMAND") {
        map.values().filter(|s| !s.is_empty()).cloned().collect()
    } else {
        match shell.lookup_var("PROMPT_COMMAND") {
            Some(s) if !s.is_empty() => vec![s.to_string()],
            _ => return None,
        }
    };
    if commands.is_empty() {
        return None;
    }
    for cmd in commands {
        match process_line(&cmd, shell, true) {
            ExecOutcome::Exit(code) => return Some(code),
            ExecOutcome::Continue(status) => shell.set_last_status(status),
            _ => {}
        }
    }
    None
}
```
(Keep the doc comment above the fn; update it to mention the array case. `get_array`/`lookup_var`/`set_last_status` are all reachable on `Shell`.)

- [ ] **Step 4: Build, test, regression, clippy**
`cargo build 2>&1 | tail -5`
`cargo test --bin huck fire_prompt_command 2>&1 | tail -6` AND `cargo test --bin huck array_runs_all_elements 2>&1 | tail -6` → the 4 new tests pass; the existing scalar tests (`fires_when_set`/`last_status_reflects_pc`/`no_op_when_unset`/`no_op_when_empty`/`propagates_exit`/`silent_when_non_interactive`) still pass.
`cargo test 2>&1 | grep -E "test result: FAILED|[1-9][0-9]* failed|error\[" | head || echo NONE` → NONE.
`cargo clippy --all-targets 2>&1 | tail -6` → clean.

- [ ] **Step 5: Commit**
```bash
git add src/shell.rs
git commit -m "$(printf 'fix: run every element of an array PROMPT_COMMAND, not just [0]\n\nbash 5.1+ runs each non-empty array element in index order; huck read only\nthe scalar view ([0]). Fixes oh-my-posh prompt not rendering (mise + omp both\nregister PROMPT_COMMAND hooks as array elements).\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 2: regression + payoff verification (verification only)

- [ ] **Step 1:** all bash-diff harnesses green: `cargo build 2>&1 | tail -2 && for f in tests/scripts/*_diff_check.sh; do printf '== %s == ' "$f"; bash "$f" >/dev/null 2>&1 && echo OK || echo "FAIL ($f)"; done` → every OK.
- [ ] **Step 2: Payoff** — a non-interactive proxy can't fire the prompt, but assert the building block: `target/debug/huck -c 'PROMPT_COMMAND=(a b); echo "${#PROMPT_COMMAND[@]}"'` → `2` (array intact). The real payoff (oh-my-posh glyph prompt after `source ~/.bashrc`) is a manual interactive check — note it for the user.
- [ ] **Step 3:** No commit (verification only) unless a fix was needed.

---

## Notes for the implementer
- **Borrow discipline:** `get_array`/`lookup_var` borrow `shell` immutably; collect the element strings into an owned `Vec<String>` BEFORE the `for` loop that calls `process_line(&mut shell, …)`. The plan's code does this.
- **Scalar path unchanged** — `get_array` returns `None` for a scalar/associative/unset PROMPT_COMMAND, so the existing scalar behavior (and all its tests) is preserved.
- No new bash-diff harness (PROMPT_COMMAND firing is interactive-only; unit tests are the gate).
