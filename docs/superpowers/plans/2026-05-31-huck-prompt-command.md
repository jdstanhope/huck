# huck v61 — `PROMPT_COMMAND` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Small enough that inline implementation by the controller is also acceptable (see v57's similar shape).

**Goal:** Run `$PROMPT_COMMAND` (if set, non-empty, and shell is
interactive) before each PS1 prompt.

**Architecture:** One helper `fire_prompt_command(shell) ->
Option<i32>` in `src/shell.rs` + one call site at the top of
the outer REPL loop. Reuses `process_line` (already used by
trap actions).

**Tech Stack:** Rust. No new deps.

**Spec:** `docs/superpowers/specs/2026-05-31-huck-prompt-command-design.md`

**Branch:** `v61-prompt-command`.

**Commit trailer:**

```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Preamble: Create the working branch

- [ ] **Step P.1**

```bash
git checkout main
git pull --ff-only
git checkout -b v61-prompt-command
```

Spec + this plan are committed as the first commit before Task
1.

---

## Task 1: Helper + wire-in + 6 unit tests

**Files:**
- Modify `src/shell.rs` — add `fire_prompt_command` + wire into
  outer loop + `#[cfg(test)] mod prompt_command_tests`.

### Step 1.1: Add `fire_prompt_command`

Insert near `process_line` (around line 248). The function signature:

```rust
/// Fires `$PROMPT_COMMAND` if set, non-empty, and the shell is
/// interactive. Returns `Some(exit_code)` if PROMPT_COMMAND
/// returned `ExecOutcome::Exit` (e.g. `PROMPT_COMMAND='exit 7'`);
/// the caller is responsible for the shell-exit cleanup
/// (`fire_exit_trap`, `hangup_jobs`, `history.save`). Returns
/// `None` otherwise; updates `shell.last_status` from the
/// Continue case so `\?` in PS1 reflects PROMPT_COMMAND's exit
/// code (matches bash semantics).
pub fn fire_prompt_command(shell: &mut Shell) -> Option<i32> {
    if !shell.is_interactive {
        return None;
    }
    let pc = match shell.lookup_var("PROMPT_COMMAND") {
        Some(s) if !s.is_empty() => s,
        _ => return None,
    };
    match process_line(&pc, shell, true) {
        crate::executor::ExecOutcome::Exit(code) => Some(code),
        crate::executor::ExecOutcome::Continue(status) => {
            shell.set_last_status(status);
            None
        }
        _ => None,
    }
}
```

(Adjust the `ExecOutcome` import path if the file already
imports it differently.)

- [ ] **Step 1.1**

### Step 1.2: Wire into the outer loop

In `src/shell.rs::run` (around line 60-65), find:

```rust
crate::jobs::reap_and_notify(&mut shell);
crate::traps::dispatch_pending_traps(&mut shell);
if let Some(helper) = editor.helper_mut() {
    helper.refresh(&shell);
}
match read_logical_command(&mut editor, &mut shell) {
```

Insert the PROMPT_COMMAND call between the helper refresh and
`read_logical_command`:

```rust
crate::jobs::reap_and_notify(&mut shell);
crate::traps::dispatch_pending_traps(&mut shell);
if let Some(helper) = editor.helper_mut() {
    helper.refresh(&shell);
}
if let Some(exit_code) = fire_prompt_command(&mut shell) {
    crate::traps::fire_exit_trap(&mut shell);
    shell.hangup_jobs();
    shell.history.save();
    return exit_code;
}
match read_logical_command(&mut editor, &mut shell) {
```

- [ ] **Step 1.2**

### Step 1.3: Build

`cargo build`. Expected: clean.

- [ ] **Step 1.3**

### Step 1.4: Append `mod prompt_command_tests`

At end of `src/shell.rs`:

```rust
#[cfg(test)]
mod prompt_command_tests {
    use super::*;
    use crate::shell_state::Shell;

    fn interactive_shell() -> Shell {
        let mut shell = Shell::new();
        shell.is_interactive = true;
        shell
    }

    #[test]
    fn fires_when_set() {
        let mut shell = interactive_shell();
        shell.set("PROMPT_COMMAND", "true".to_string());
        assert_eq!(fire_prompt_command(&mut shell), None);
        assert_eq!(shell.last_status(), 0);
    }

    #[test]
    fn last_status_reflects_pc() {
        let mut shell = interactive_shell();
        shell.set("PROMPT_COMMAND", "false".to_string());
        assert_eq!(fire_prompt_command(&mut shell), None);
        assert_eq!(shell.last_status(), 1);
    }

    #[test]
    fn no_op_when_unset() {
        let mut shell = interactive_shell();
        shell.set_last_status(42);
        assert_eq!(fire_prompt_command(&mut shell), None);
        assert_eq!(shell.last_status(), 42);
    }

    #[test]
    fn no_op_when_empty() {
        let mut shell = interactive_shell();
        shell.set("PROMPT_COMMAND", String::new());
        shell.set_last_status(42);
        assert_eq!(fire_prompt_command(&mut shell), None);
        assert_eq!(shell.last_status(), 42);
    }

    #[test]
    fn propagates_exit() {
        let mut shell = interactive_shell();
        shell.set("PROMPT_COMMAND", "exit 7".to_string());
        assert_eq!(fire_prompt_command(&mut shell), Some(7));
    }

    #[test]
    fn silent_when_non_interactive() {
        let mut shell = Shell::new();
        shell.is_interactive = false;
        shell.set("PROMPT_COMMAND", "false".to_string());
        shell.set_last_status(42);
        assert_eq!(fire_prompt_command(&mut shell), None);
        // last_status unchanged since PC didn't run.
        assert_eq!(shell.last_status(), 42);
    }
}
```

If `is_interactive` is not a pub field of `Shell`, switch to
whatever the public toggle is (or shell_state.rs may need to
expose a setter).

- [ ] **Step 1.4**

### Step 1.5: Run tests

```bash
cargo test --bin huck prompt_command_tests
```

Expected: 6 pass.

- [ ] **Step 1.5**

### Step 1.6: Full unit suite

`cargo test --bin huck`. Expected: green.

- [ ] **Step 1.6**

### Step 1.7: Clippy

`cargo clippy --all-targets -- -D warnings`. Expected: clean.

- [ ] **Step 1.7**

### Step 1.8: Commit Task 1

```bash
git add src/shell.rs
git commit -m "$(cat <<'EOF'
shell: fire PROMPT_COMMAND before PS1 (v61)

Bash extension: run \`\$PROMPT_COMMAND\` (if set, non-empty, and
shell is interactive) before each PS1 prompt. Completes the
PROMPT_COMMAND row that M-76 (v60) listed as deferred.

Implementation: one helper \`fire_prompt_command(shell) ->
Option<i32>\` reusing \`process_line\` (the same path trap actions
use). Returns Some(exit_code) when PROMPT_COMMAND ran an
\`exit\`; the outer REPL handles cleanup (fire_exit_trap +
hangup_jobs + history.save) before returning the code.
Otherwise updates \`shell.last_status\` so PS1's \`\\?\` reflects
PROMPT_COMMAND's exit code (matches bash semantics).

Wire-in at the top of the outer REPL loop, between the helper
refresh and \`read_logical_command\`. Skipped when
\`is_interactive\` is false (matches bash — non-interactive
shells don't fire PROMPT_COMMAND). Naturally only fires once per
logical command (before PS1); continuation lines (PS2) inside
\`read_logical_command\` don't trigger it.

6 unit tests in \`mod prompt_command_tests\`: fires-when-set
(true), last_status-reflects-pc (false), no-op-when-unset,
no-op-when-empty, propagates-exit (exit 7 → Some(7)), and
silent-when-non-interactive.

Deferred: array form \`PROMPT_COMMAND=("cmd1" "cmd2")\` (huck has
no arrays).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

Stage exactly: `src/shell.rs`.

- [ ] **Step 1.8**

---

## Task 2: Docs

**Files:**
- Modify `docs/bash-divergences.md` — update M-76 to remove
  `PROMPT_COMMAND` from the deferred list and note v61 shipped
  it; add v61 change-log entry.
- Modify `README.md` — add v61 row.

### Step 2.1: Update M-76 entry

Find M-76 in the Tier 2 section. The current "Deferred" list
includes `PROMPT_COMMAND`. Remove that single item from the
deferred list and add a sentence noting v61 shipped it. The
rest of the entry stays the same.

Change the line:

```markdown
... **Deferred**: date/time escapes (`\d \t \T \@ \A \D{fmt}`), `\v`/`\V`/`\s`, octal `\nnn`, command substitution `$(...)` inside the template, `PROMPT_COMMAND`, `PS3` (for `select`, not yet in huck), `PS4` (for `set -x`, not yet in huck).
```

to:

```markdown
... **Deferred**: date/time escapes (`\d \t \T \@ \A \D{fmt}`), `\v`/`\V`/`\s`, octal `\nnn`, command substitution `$(...)` inside the template, `PS3` (for `select`, not yet in huck), `PS4` (for `set -x`, not yet in huck). **Updates**: v61 ships `PROMPT_COMMAND` (string form; array form deferred since huck has no arrays).
```

- [ ] **Step 2.1**

### Step 2.2: Add v61 change-log entry

In `## Change log` after v60:

```markdown
- **2026-05-31**: v61 finishes the `PROMPT_COMMAND` row M-76 left deferred. New `fire_prompt_command(shell) -> Option<i32>` helper in `src/shell.rs` reuses `process_line` (the same path trap actions use) to execute `$PROMPT_COMMAND` in the current shell context. Wire-in at the top of the outer REPL loop fires it between the helper refresh and `read_logical_command`. `is_interactive`-gated (matches bash — non-interactive shells skip it). Only fires before PS1, not PS2 (continuation lines run inside `read_logical_command` and don't loop back). `exit` inside PROMPT_COMMAND propagates: helper returns `Some(code)` and the outer loop does the standard cleanup (fire_exit_trap + hangup_jobs + history.save) before returning. Otherwise `last_status` is updated from PROMPT_COMMAND's exit so PS1's `\?` and the next user command's `$?` both see it (matches bash). 6 unit tests. Deferred: array form `PROMPT_COMMAND=("cmd1" "cmd2")` since huck has no arrays. No new L-* divergences.
```

- [ ] **Step 2.2**

### Step 2.3: Add v61 row to README

After v60:

```markdown
| v61       | `PROMPT_COMMAND` (M-76 cont.)                                  |
```

Match v60 column padding.

- [ ] **Step 2.3**

### Step 2.4: Full suite

`cargo test --all-targets`. Expected: green (PTY flake tolerated).

- [ ] **Step 2.4**

### Step 2.5: Clippy

`cargo clippy --all-targets -- -D warnings`. Expected: clean.

- [ ] **Step 2.5**

### Step 2.6: Commit Task 2

```bash
git add docs/bash-divergences.md README.md
git commit -m "$(cat <<'EOF'
docs: PROMPT_COMMAND shipped v61; update M-76

M-76 entry no longer lists PROMPT_COMMAND as deferred; an
"Updates" sentence notes v61 ships the string form (array form
still deferred since huck has no arrays).

Change log: 2026-05-31 v61 entry summarizing the
fire_prompt_command helper, the outer-loop wire-in,
is_interactive gating, and exit-propagation semantics.

README: v61 row added.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 2.6**

---

## Final verification (controller)

1. `cargo test --all-targets` once more.
2. `cargo clippy --all-targets -- -D warnings`.
3. Branch is three commits ahead of `main`: docs preamble + 2
   task commits.
4. Skip the full reviewer dispatch (small iteration; v57 set
   the precedent for skipping when scope is small).
5. Merge to `main` with `--no-ff`, push, delete branch, update
   memory.
