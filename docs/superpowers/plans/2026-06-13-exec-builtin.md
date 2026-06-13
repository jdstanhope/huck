# `exec` Builtin Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> Process note: this plan was written to document an as-built implementation
> (v155 was coded on a branch before the spec/plan were authored, then retrofitted
> into the standard flow). The tasks below describe the change as it landed.

**Goal:** Implement the `exec` special builtin — replace the process image
(`exec cmd args`) and apply permanent redirections (`exec >log 2>&1`) — with the
`-a`/`-l`/`-c` options, matching bash.

**Architecture:** Intercept `exec` in `run_exec_single` (after inline assignments
are applied/exported, before dispatch); replace via `CommandExt::exec`; apply
permanent redirections by reusing `CompoundRedirectScope` without restore.

**Tech Stack:** Rust, `libc`, `std::os::unix::process::CommandExt`.

---

### Task 1: Register `exec` as a special builtin

**Files:**
- Modify: `src/builtins.rs` (`BUILTIN_NAMES`, `is_special_builtin`, `run_builtin` guard, `search_path_for` visibility)

- [ ] **Step 1:** Add `"exec"` to `BUILTIN_NAMES` (so it is recognized, not
  "command not found", and `builtin exec` / `command exec` resolve through the
  existing strip loops).
- [ ] **Step 2:** Add `"exec"` to `is_special_builtin` (inline-assignment persistence).
- [ ] **Step 3:** Add a defensive `"exec" => { eprintln!("huck: exec: not supported in this context"); ExecOutcome::Continue(1) }` arm to `run_builtin` so a future refactor that routes `exec` here degrades instead of panicking on the `unreachable!`.
- [ ] **Step 4:** Promote `fn search_path_for` to `pub(crate)` for reuse by the executor.
- [ ] **Step 5:** `cargo build`; confirm `type -t exec` ⇒ `builtin`. Commit.

### Task 2: Flag parsing

**Files:**
- Modify: `src/executor.rs` (`ExecFlags`, `parse_exec_flags`)
- Test: `src/executor.rs` (`#[cfg(test)] mod tests`)

- [ ] **Step 1:** Write failing unit tests for `parse_exec_flags`: plain command,
  `-c -l -a NAME`, clustered `-cla NAME`, inline `-aNAME`, `--`, bare `-`, error
  cases (`-Z`, dangling `-a`), flags-only.
- [ ] **Step 2:** Implement `ExecFlags { clear_env, login, argv0, operand_start }`
  and `parse_exec_flags(args) -> Result<ExecFlags, String>` (cluster scan; `-a`
  consumes inline remainder or next word; stop at first non-flag or `--`).
- [ ] **Step 3:** Run the tests (green). Commit.

### Task 3: Permanent redirections + signal save/restore

**Files:**
- Modify: `src/executor.rs` (`apply_redirects_permanently`, `EXEC_RESET_SIGNALS`, `reset_exec_signals_saving`, `restore_exec_signals`)

- [ ] **Step 1:** `apply_redirects_permanently(cmd, shell)` — open stdin (file /
  heredoc / here-string; reject Dup/out-redirects on stdin) + stdout/stderr (reuse
  `apply_out_redirect`) into a `CompoundRedirectScope`; on success `drain` the saved
  originals closing them (permanent, no leak); on any failure return `Err` and let
  the scope Drop roll back atomically.
- [ ] **Step 2:** `reset_exec_signals_saving()` / `restore_exec_signals()` — set
  `SIGTSTP`/`SIGTTIN`/`SIGTTOU` to `SIG_DFL`, returning prior handlers (NOT a
  `pre_exec` hook — avoids corrupting the shell's signals on the failure path).
- [ ] **Step 3:** `cargo build`. Commit.

### Task 4: `run_exec_builtin` + interception

**Files:**
- Modify: `src/executor.rs` (`run_exec_builtin`, interception in `run_exec_single`)

- [ ] **Step 1:** `run_exec_builtin(resolved, cmd, shell)`: parse flags → if any
  redirects, `flush_stdout()` + `apply_redirects_permanently` (on failure return
  `Continue(1)`, do NOT exit) → if no operand, return `Continue(0)` → PATH-resolve
  via `search_path_for` (None ⇒ `not found` 127 / `cannot execute` 126; exit
  non-interactive, return interactive) → build `ProcessCommand` (env_clear; add
  exported env+functions unless `-c`; `arg0` from `-a`/`-l`/default; args) → reset
  signals → `.exec()` → on return, restore signals + classify errno (ENOENT 127 else
  126), print diagnostic, exit/return by interactivity.
- [ ] **Step 2:** Intercept in `run_exec_single`: after `let persistent = …`, add
  `if resolved.program == "exec" { let o = run_exec_builtin(&resolved, cmd, shell); drain_procsubs(shell, procsub_base); return o; }`.
- [ ] **Step 3:** `cargo build`. Manually verify replace / `>file` / failure / subshell
  / pipeline against bash. Commit.

### Task 5: Diff harness + final checks

**Files:**
- Create: `tests/scripts/exec_diff_check.sh`
- Modify: `docs/bash-divergences.md` (record the residual fd>2-redirect gap)

- [ ] **Step 1:** Write `exec_diff_check.sh` (both modes; status-only failure cases;
  fd>2 / wording divergences excluded with comments). Run: all PASS.
- [ ] **Step 2:** Record the residual `exec 3<file` (fd>2 redirect) gap in
  `docs/bash-divergences.md`.
- [ ] **Step 3:** Full suite + clippy green. Commit.
