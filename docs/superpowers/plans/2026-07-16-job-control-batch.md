# Job-control batch (v306) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix three bash-5.2.21 job-control divergences — full `jobs` command line (#80), non-interactive job-table pruning (#175), and `set -m` job control (#167).

**Architecture:** All in `crates/huck-engine`. #80 adds an AST→source deparser used at the background-job-display sites; #175 wires the existing `reap_and_notify` prune into the non-interactive driver loop and makes `wait_for_job` remove its target; #167 makes `job_control_active()` honor the `monitor` option with an `isatty` guard on terminal control.

**Tech Stack:** Rust, `libc`, the huck-syntax AST (`Command`/`Pipeline`/`SimpleCommand`/`Word`).

**Design doc:** `docs/superpowers/specs/2026-07-16-job-control-batch-design.md`.

## Global Constraints

- Every commit ends with the trailer `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- `cargo fmt --all` before every commit (CI enforces `--check`).
- Tests on this box are OOM-sensitive: build the binary with `cargo build -p huck`; run engine lib tests as `( ulimit -v 2500000; timeout 900 cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 )`; run each integration bin single-threaded with a `ulimit -v` guard; guard the sweep with `( ulimit -v 1500000; timeout 1100 tests/scripts/run_diff_checks.sh )`. NEVER `cargo test --workspace`.
- bash-diff harnesses go in `tests/scripts/*_diff_check.sh`, auto-discovered by `run_diff_checks.sh`; each targets `target/debug/huck` by default and normalizes the `huck:`/`bash:` prologue to compare byte-identically.
- Order matters: implement in the task order below (#80 first — `fg`'s echoed command in #167 depends on it).
- Do NOT change async-notification behavior (the `[N]+ Done` notice under non-interactive `set -m` is deferred #158 b). This batch changes pruning and pgroup activation only.

---

## Task 1: #80 — full command line in `jobs` (AST→source deparser)

**Files:**
- Modify: `crates/huck-engine/src/executor.rs` (add `render_job_command`; rewire `group_display_label` call site ~562 and the two `run_background_*` display sites ~2834/2885)
- Test: `crates/huck-engine/src/executor/tests.rs` (deparser unit tests)
- Create: `tests/scripts/job_command_line_diff_check.sh`

**Interfaces:**
- Consumes: `crate::lexer::{Word, WordPart}`, `huck_syntax::command::{Command, Pipeline, SimpleCommand, ExecCommand}`, `param_expansion::xtrace_quote`.
- Produces: `fn render_job_command(cmd: &Command) -> String` — the normalized bash-style command line stored as a job's `command` (used by `jobs`, `fg`, `bg` display).

**Background (bash-verified, must match):** whitespace collapses to single spaces; quotes preserved (`sleep 0.3 "a b"`); variables UNexpanded (`sleep $x`); pipeline ` | `; and-or ` && `/` || `; redirect ` > /dev/null`, `2>&1`; subshell `( … )`; brace group `{ …; }`. The trailing `&` is NOT part of the stored command (the `jobs` formatter appends ` &`).

- [ ] **Step 1: Write the failing bash-diff harness** `tests/scripts/job_command_line_diff_check.sh`.

```bash
#!/usr/bin/env bash
# #80: `jobs` shows the full, normalized command line (not the leading name /
# a `background job` placeholder), matching bash 5.2.21's re-rendered job text.
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: build huck first (cargo build -p huck)" >&2; exit 1; }
FAIL=0
# Compare only the command column: strip `[N]<flag>`, the state word, and the
# padding, leaving `<command> &`. Both shells pad differently; we compare the
# trimmed `jobs` command text.
cmdcol() { sed -E 's/^\[[0-9]+\][-+ ] +(Running|Done|Stopped)[^ ]* +//'; }
check() {
  local label=$1 frag=$2 b h
  b=$(bash    -c "$frag" 2>/dev/null | cmdcol)
  h=$("$HUCK" -c "$frag" 2>/dev/null | cmdcol)
  if [ "$b" != "$h" ]; then echo "FAIL [$label] bash=[$b] huck=[$h]"; FAIL=1; else echo "PASS [$label]"; fi
}
# Use `sleep 0.3` so the job is still Running when `jobs` reads it, then let it finish.
check simple    'sleep 0.3 aa bb & jobs; wait'
check spaced    'sleep   0.3    aa & jobs; wait'
check pipeline  'sleep 0.3 | cat & jobs; wait'
check andor     'sleep 0.3 && echo hi & jobs; wait'
check redirect  'sleep 0.3 >/dev/null & jobs; wait'
check quoted    'sleep 0.3 "a b" & jobs; wait'
check unexpand  'x=0.3; sleep $x & jobs; wait'
if [ $FAIL -ne 0 ]; then echo "job_command_line_diff_check FAILED" >&2; exit 1; fi
echo "job_command_line_diff_check OK"
```

- [ ] **Step 2: Run it to confirm RED.** `chmod +x tests/scripts/job_command_line_diff_check.sh && cargo build -p huck && ( ulimit -v 2000000; timeout 120 tests/scripts/job_command_line_diff_check.sh )` → FAIL (huck shows `sleep`/`background job`).

- [ ] **Step 3: Implement `render_job_command`** in `executor.rs` near `display_command` (~3046). Render from the AST:

```rust
/// Render a `Command` back to a normalized bash-style source line for the
/// `jobs`/`fg`/`bg` display: whitespace collapsed to single spaces, quotes
/// preserved via shell-quoting, words shown pre-expansion (like bash re-renders
/// a job's command). The trailing `&` is added by the jobs formatter, not here.
fn render_job_command(cmd: &Command) -> String {
    match cmd {
        Command::Simple(s) => render_simple(s),
        Command::Pipeline(p) => p
            .commands
            .iter()
            .map(render_job_command)
            .collect::<Vec<_>>()
            .join(" | "),
        Command::Subshell { body } => format!("( {} )", render_sequence(body)),
        Command::Group { body } => format!("{{ {}; }}", render_sequence(body)),
        // Compound commands (if/for/while/case/…): fall back to a best-effort
        // keyword label; bash renders the full compound, but these are rare as
        // direct background jobs. Use the existing compound header text if one is
        // readily available, else the leading keyword.
        other => render_compound_fallback(other),
    }
}
```

Implement the helpers:
- `render_simple(&SimpleCommand) -> String`: for `SimpleCommand::Exec(e)`, join `e.assignments` (rendered `name=value`), the program word, and arg words with single spaces, then append redirections (` <op> <target>`, using the redirection op text and the rendered target word). For `SimpleCommand::Assignment`-only forms render the assignments.
- `render_word(&Word) -> String`: concatenate `WordPart`s. `WordPart::Literal { text, quoted }` → if `quoted` OR `text` needs quoting (contains whitespace/metachars), emit `param_expansion::xtrace_quote(text)`; else emit `text` verbatim. Expansion parts (`$x`, `${…}`, `$(…)`, arithmetic, command sub) render from their retained raw/name text UNEXPANDED (e.g. `$x`, `${a[1]}`); reuse any existing raw field, else reconstruct the sigil form. Array-literal RHS renders `(a b c)`.
- `render_sequence(&Sequence) -> String`: render `first` then each `(connector, cmd)` joined by ` && `/` || `/` ; ` per `Connector`.
- `render_compound_fallback` may reuse `group_display_label`'s current logic as the last resort.

Consult `crate::param_expansion::xtrace_quote` for shell-quoting (already used by xtrace) and mirror how `xtrace_command_line` renders — but from the AST/pre-expansion words, not resolved args.

- [ ] **Step 4: Rewire the display sites.**
  - Line ~562: replace `let source = group_display_label(group.first);` + `&source` with `let source = render_job_command(group.first);` (keep the `&source` call shape).
  - Lines ~2834 and ~2885 (`run_background_subshell`/`run_background_sequence`): these receive `source: &str` and call `display_command(source)`. Change them to accept the `&Command`/`&Pipeline` they already have in scope (or the caller passes the rendered string) and use `render_job_command`, so the trailing-`&`-only forms are normalized identically. If threading the AST node is awkward, render at the call sites (lines 262/265/286) and pass the rendered `String`. Remove `group_display_label` (now unused) and `display_command` if it becomes unused (watch for dead-code warnings).

- [ ] **Step 5: Add deparser unit tests** in `executor/tests.rs` (parse via the engine, or construct via the parser) asserting `render_job_command` output for: simple+args, collapsed whitespace, pipeline, and-or, redirect, double-quoted arg, `$x` unexpanded. Prefer parsing a snippet and rendering its first command to avoid hand-building AST.

- [ ] **Step 6: GREEN + verify.** `cargo build -p huck`; run the harness (Step 2 command) → OK; run engine lib suite (Global Constraints command) → all pass; `cargo fmt --all`.

- [ ] **Step 7: Commit.**
```bash
git add crates/huck-engine/src/executor.rs crates/huck-engine/src/executor/tests.rs tests/scripts/job_command_line_diff_check.sh
git commit -m "feat(#80): render full command line for jobs display via AST deparser

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: #175 — non-interactive job-table pruning

**Files:**
- Modify: `crates/huck-engine/src/builtins.rs` (`run_sourced_contents_in_sinks` `'outer` loop; `wait_for_job`)
- Test: `crates/huck-engine/src/builtins/fg_bg_tests.rs` (unit assertions)
- Create: `tests/scripts/job_prune_diff_check.sh`

**Interfaces:**
- Consumes: `crate::jobs::reap_and_notify`, `JobTable` (`jobs_mut`/`retain`), `JobState`.
- Produces: no new public API; behavior change only.

**Background (bash-verified):** non-interactively, completed (Done/Signaled) jobs are silently pruned between commands and on `wait`; Running/Stopped are kept; no async notice without `set -m`.

- [ ] **Step 1: Write the failing harness** `tests/scripts/job_prune_diff_check.sh`.

```bash
#!/usr/bin/env bash
# #175: completed background jobs are pruned non-interactively (matching bash):
# after wait, after each command, and via `wait $!`; running/stopped kept.
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: build huck first" >&2; exit 1; }
FAIL=0
check() { # label frag
  local b h
  b=$(bash    -c "$2" 2>/dev/null)
  h=$("$HUCK" -c "$2" 2>/dev/null)
  if [ "$b" != "$h" ]; then echo "FAIL [$1] bash=[$b] huck=[$h]"; FAIL=1; else echo "PASS [$1]"; fi
}
check bgwait_empty   'sleep 0 & wait $!; echo "n=$(jobs | wc -l)"'
check loop_bounded   'i=0; while [ $i -lt 60 ]; do sleep 0 & wait $!; i=$((i+1)); done; echo "n=$(jobs | wc -l)"'
check nowait_pruned  'sleep 0 & sleep 0.2; echo "n=$(jobs | wc -l)"'
check many_nowait    'i=0; while [ $i -lt 60 ]; do sleep 0 & i=$((i+1)); done; sleep 0.3; echo "n=$(jobs | wc -l)"'
check running_kept   'sleep 0.4 & echo "n=$(jobs | wc -l)"; wait'
if [ $FAIL -ne 0 ]; then echo "job_prune_diff_check FAILED" >&2; exit 1; fi
echo "job_prune_diff_check OK"
```

- [ ] **Step 2: RED.** Build + run → FAIL (`bgwait_empty` huck n=1; loop/many huck n=60).

- [ ] **Step 3: Prune on `wait`.** In `wait_for_job` (builtins.rs ~4438), after computing the terminal `code` and before `return ExecOutcome::Continue(code)`, remove the job:
```rust
if let Some(code) = terminal {
    shell.jobs.jobs_mut().retain(|j| j.id != id);
    return ExecOutcome::Continue(code);
}
```
(Confirm `id` is in scope; if `wait_for_job` takes `id: u32`, it is.)

- [ ] **Step 4: Between-command prune.** In `run_sourced_contents_in_sinks` (builtins.rs ~7495), inside the top-level `'outer` loop, call `crate::jobs::reap_and_notify(&mut *shell)` once per iteration (place it at the loop top, before parsing/executing the next unit, mirroring `repl.rs:198`). `reap_and_notify`'s printing is already gated on `shell.is_interactive`, so this prunes silently non-interactively. Verify the borrow of `shell` there is compatible (it is a `&mut Shell` in that function).

- [ ] **Step 5: Unit assertions** in `fg_bg_tests.rs`: (a) synthesize a Done job, run a no-op through the between-command path (or call `reap_and_notify`) and assert the table is empty; (b) a Stopped job is retained; (c) `wait_for_job` on a terminal job leaves the table without that id.

- [ ] **Step 6: GREEN + verify.** Build; run the harness → OK; run engine lib suite → pass; run the `jobs` integration bin `( ulimit -v 2500000; timeout 300 cargo test -p huck --test jobs --jobs 1 -- --test-threads 1 )` → pass; `cargo fmt --all`.

- [ ] **Step 7: Commit.**
```bash
git add crates/huck-engine/src/builtins.rs crates/huck-engine/src/builtins/fg_bg_tests.rs tests/scripts/job_prune_diff_check.sh
git commit -m "fix(#175): prune completed jobs non-interactively (between commands + on wait)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: #167 — `set -m` activates job control non-interactively

**Files:**
- Modify: `crates/huck-engine/src/shell_state.rs` (`job_control_active`)
- Modify: `crates/huck-engine/src/executor.rs` (`isatty` guard on terminal control — the `give_terminal_to` sites / the `interactive` computation)
- Test: `crates/huck-engine/src/builtins/fg_bg_tests.rs`
- Create: `tests/scripts/set_m_jobcontrol_diff_check.sh`

**Interfaces:**
- Consumes: `shell.shell_options.monitor`, `libc::isatty`.
- Produces: `job_control_active()` returns true under `set -m` too; no new API.

- [ ] **Step 1: Write the failing harness** `tests/scripts/set_m_jobcontrol_diff_check.sh` (rc + stdout, tty-independent):

```bash
#!/usr/bin/env bash
# #167: `set -m` activates job control non-interactively so scripted fg/bg on a
# live job return the job's real status (rc 0), matching bash 5.2.21.
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: build huck first" >&2; exit 1; }
FAIL=0
norm() { sed -e "s#$HUCK: line [0-9]*: #SH: #g" -e 's#bash: line [0-9]*: #SH: #g'; }
check() { local b h; b=$( { bash -c "$2"; echo "rc=$?"; } 2>&1 | norm); h=$( { "$HUCK" -c "$2"; echo "rc=$?"; } 2>&1 | norm)
  if [ "$b" != "$h" ]; then echo "FAIL [$1] bash=[$(printf %s "$b"|tr '\n' '|')] huck=[$(printf %s "$h"|tr '\n' '|')]"; FAIL=1; else echo "PASS [$1]"; fi }
check fg_live   'set -m; sleep 0.2 & fg %1; echo done'
check fg_rc     'set -m; sleep 0.2 & fg %1; echo rc=$?'
check bg_then   'set -m; sleep 0.2 & bg %1 2>/dev/null; wait; echo ok'
if [ $FAIL -ne 0 ]; then echo "set_m_jobcontrol_diff_check FAILED" >&2; exit 1; fi
echo "set_m_jobcontrol_diff_check OK"
```
(Note: `fg` echoes the job command — depends on Task 1 rendering `sleep 0.2` correctly. Keep the commands simple so the deparse is exact.)

- [ ] **Step 2: RED.** Build + run → FAIL (huck `fg_rc` rc=1, and command echoes `sleep` not `sleep 0.2` if run before Task 1 — Task 1 is already merged in this branch).

- [ ] **Step 3: Activate under monitor.** In `shell_state.rs` `job_control_active`:
```rust
pub fn job_control_active(&self) -> bool {
    (self.is_interactive || self.shell_options.monitor) && !self.in_subshell && !self.in_completion
}
```

- [ ] **Step 4: Guard terminal control for no-tty.** Add a helper `fn stdin_is_tty() -> bool { unsafe { libc::isatty(0) == 1 } }` (or reuse an existing tty check if present — grep `isatty`). At each `give_terminal_to(pid)` call in a foreground-job path (the `interactive` branches around executor.rs 811/5490 and in `builtin_fg`), only call `give_terminal_to`/`tcsetpgrp` when `stdin_is_tty()`. When there is no tty, skip the tcsetpgrp but still `setpgid` + `waitpid(-pgid)` the job (this is what makes `set -m` under a pipe behave like bash). Verify the interactive tty path is unchanged (isatty true there).

- [ ] **Step 5: Unit assertions** in `fg_bg_tests.rs`: with `monitor=true` and `is_interactive=false`, `shell.job_control_active()` is true; with both false, it is false.

- [ ] **Step 6: GREEN + full verify.** Build; run the new harness → OK; run engine lib suite → pass; run the job-control integration bins single-threaded: `jobs`, `jobcontrol-pty`, `disown_h_integration` (each `cargo test -p huck --test <name> --jobs 1 -- --test-threads 1` under `ulimit -v`); `cargo fmt --all`.

- [ ] **Step 7: Commit.**
```bash
git add crates/huck-engine/src/shell_state.rs crates/huck-engine/src/executor.rs crates/huck-engine/src/builtins/fg_bg_tests.rs tests/scripts/set_m_jobcontrol_diff_check.sh
git commit -m "fix(#167): set -m activates job control non-interactively (isatty-guarded tty control)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Whole-branch verification (after all three tasks)

- [ ] `cargo fmt --all --check` clean.
- [ ] `cargo build -p huck` and `cargo build --release --locked -p huck` both clean (no warnings).
- [ ] Engine lib suite green (Global Constraints command).
- [ ] huck-syntax lib green (`( ulimit -v 2000000; timeout 300 cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1 )`).
- [ ] Each touched integration bin green single-threaded: `jobs`, `jobcontrol-pty`, `disown_h_integration`, plus any redirect/exec bins if executor.rs terminal-control edits touched shared code.
- [ ] Full diff sweep green on both binaries: `( ulimit -v 1500000; timeout 1100 tests/scripts/run_diff_checks.sh )` — includes the 3 new harnesses. Watch for nondeterminism in the prune/`sleep 0` cases; if a case flakes, use a deterministic upstream (see the #151 SIGPIPE lesson) — here prefer `sleep 0.2`-style live jobs + `wait` over racing `sleep 0`.
- [ ] Whole-branch review (Opus): async-signal-safety unaffected; the `job_control_active` broadening reviewed against every `pgid_target`/`give_terminal_to` site; deparser has no panic on unusual AST; pruning keeps Running/Stopped.

## Self-review notes
- Task ordering: #80 first (fg echo depends on it), then #175, then #167.
- Type consistency: `render_job_command(&Command) -> String`; `wait_for_job(id: u32, …)` uses `id` for the retain; `job_control_active` reads `self.shell_options.monitor`.
- No placeholders: each code step shows the change or a concrete skeleton; the deparser helpers are enumerated with their exact join strings.
