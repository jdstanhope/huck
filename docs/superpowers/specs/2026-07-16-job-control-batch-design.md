# Job-control batch: table pruning, full command line, `set -m` — Design

Batch of three job-control divergences from bash 5.2.21, all in the same
subsystem (`crates/huck-engine/src/jobs.rs`, the `builtin_jobs`/`fg`/`bg`/`wait`
builtins, and the executor's background-spawn path):

- **[#175](https://github.com/jdstanhope/huck/issues/175)** — non-interactive job
  table never prunes completed jobs (unbounded memory growth).
- **[#80](https://github.com/jdstanhope/huck/issues/80)** — `jobs` shows only the
  command *name* / a `background job` placeholder, not the full command line.
- **[#167](https://github.com/jdstanhope/huck/issues/167)** — non-interactive
  `set -m` does not activate job control, so scripted `fg`/`bg` on a live job
  fails.

Each is independent enough to be its own task, but they share the job-control
test surface, so they ship as one branch (`v306`) with one bash-diff harness set.

---

## #175 — Non-interactive job-table pruning

### Current behavior
Outside the interactive REPL, huck reaps completed background children and marks
their job `Done`/`Signaled`, but never removes the entry. The table grows without
bound, and every `jobs` re-reports the same `Done` line.

### bash 5.2.21 behavior (verified)
Non-interactively (no `set -m`, so no async notices), bash **silently prunes**
completed jobs at job-table maintenance points:
- After each command (before running the next), reaped Done/Signaled jobs are
  removed. `sleep 0 & sleep 0.2; jobs` → empty; 300 backgrounded-without-wait
  jobs → `jobs` shows 0.
- `wait`/`wait $!` delivers a job's status and removes that job.
- Running and Stopped jobs are **kept**.

### Root cause
Two gaps:
1. The interactive REPL calls `jobs::reap_and_notify` before each prompt
   (`crates/huck-cli/src/repl.rs:198`), which reaps + `drain_notifications` +
   `remove_notified` (prune). The **non-interactive** driver loop
   (`run_sourced_contents_in_sinks`' `'outer` loop in `builtins.rs`) never runs
   this prune — only `reap_completed` (state update, no prune) is wired into the
   `jobs`/`fg`/`bg` builtins (v299).
2. `wait_for_job(id, …)` (the `wait $!` / `wait %n` path in `builtins.rs`)
   returns the terminal job's exit code but never removes the entry (the no-arg
   `wait` path already calls `reap_and_notify`).

### Design
`reap_and_notify` already does exactly the right thing — reap, then
`drain_notifications` (marks non-Running jobs notified) + `remove_notified`
(drops non-Running notified jobs, i.e. Done/Signaled; keeps Running/Stopped) —
and its **printing is already gated on `shell.is_interactive`**, so calling it
non-interactively prunes silently, matching bash.

1. **Between-command prune.** In `run_sourced_contents_in_sinks`' top-level
   `'outer` loop, call `jobs::reap_and_notify(shell)` once per iteration (before
   executing the next unit), mirroring the interactive REPL's per-prompt cadence.
   This prunes Done/Signaled jobs that were never explicitly waited.
2. **`wait` prune.** After `wait_for_job` resolves a job to its terminal status,
   remove that job from the table before returning the code. (bash removes a
   waited job immediately, so a following `jobs` does not show it.)

Notes / invariants:
- Stopped jobs are retained (`remove_notified` keeps `Stopped`).
- No behavior change interactively (the REPL already prunes; a second prune per
  loop is a no-op there since that path isn't the REPL).
- Under `set -m` bash DOES emit async `[N]+ Done` notices non-interactively;
  huck omits them — that remains the deferred **#158 scope (b)** and is out of
  scope here. This task only changes *pruning*, not *notification*.
- The `$(…)`/`execute_capturing` path already runs backgrounding synchronously
  (no real jobs), so it is unaffected.

---

## #80 — Full command line in `jobs`

### Current behavior
- `sleep 0.3 aa bb & jobs` → huck `[1]+ Running   sleep &` (leading name only).
- `sleep 0.3 | cat & jobs` → huck `[1]+ Running   background job &` (placeholder).

The trailing-`&`-only forms (`cmd &` with nothing after) already render the full
source via `display_command(source)`. The divergence is specifically the
**mid-list `&`** partition (`cmd & jobs`, a `Connector::Amp` separator), which
routes through `execute_sequence_body` → `group_display_label(first)` — that
helper returns the first command's static program name, or the literal
`background job` for anything non-simple, because the original source substring
is not threaded to that site.

### bash 5.2.21 behavior (verified)
bash **re-renders** the job's command from its parsed form (it does not store the
literal source): whitespace is collapsed to single spaces, quotes are preserved
(`sleep 0.3 "a b"`), variables are shown UNexpanded (`sleep $x`), pipelines join
with ` | `, and-or with ` && `/` || `, redirects render as ` > file`. So
byte-identical output requires an AST→source deparser, NOT source slicing (which
would keep the extra whitespace bash collapses).

### Design
Add an AST→source deparser in the executor and use it as the job-display label:

```rust
/// Render a Command back to a normalized bash-style source line for `jobs`
/// display (whitespace collapsed, quotes preserved via shell-quoting, words
/// shown pre-expansion). Mirrors bash's job-command re-rendering.
fn render_job_command(cmd: &Command) -> String
```

- **Words**: render each `Word` from its `WordPart`s. Unquoted literal parts
  emit verbatim; a `Literal { quoted: true, .. }` part (or any part needing
  protection) is shell-quoted via the existing `param_expansion::xtrace_quote`
  (`@Q`-style) so the result re-reads. Expansions (`$x`, `${…}`, `$(…)`,
  arithmetic) render from their retained raw text, unexpanded. Words join with
  single spaces.
- **SimpleCommand**: assignments + program + args, space-joined; trailing
  redirections rendered ` <op> <target>` (e.g. `> /dev/null`, `2>&1`).
- **Pipeline**: stages joined with ` | `.
- **And-or / Sequence group**: joined with ` && ` / ` || ` (and `;` for the rare
  backgrounded `;`-group, though that path backgrounds only the last group).
- **Subshell**: `( <body> )`; brace group: `{ <body>; }`.

Wire-up: replace `group_display_label(first)` (line ~562) with
`render_job_command(first)`. Also route the trailing-`&`-only path
(`display_command(source)` at the two `run_background_*` sites) through the
deparser so ALL backgrounded forms are normalized identically to bash (this also
drops the now-unused `display_command`/`group_display_label`, or keeps
`display_command` only if another caller remains). The synthetic `( subshell )`
label at `executor.rs:817` and the coproc label are unaffected.

Scope / residual: the deparser is byte-identical to bash for the common,
unquoted and simply-quoted forms the harness covers. Exotic quoting (mixing
`'`/`"`/`\` on one word — huck retains `quoted: bool`, not the original quote
character) re-quotes canonically and MAY differ from bash's echo of the original
quote char; documented as an accepted best-effort residual for a `sev:low`
cosmetic field, not a new tracked divergence.

---

## #167 — `set -m` activates job control non-interactively

### Current behavior
```
$ huck -c 'set -m; sleep 0.2 & fg %1; echo rc=$?'   # sleep\nrc=1
$ bash -c 'set -m; sleep 0.2 & fg %1; echo rc=$?'   # sleep 0.2\nrc=0
```
`Shell::job_control_active()` (`shell_state.rs:1138`) is
`self.is_interactive && !self.in_subshell && !self.in_completion` — it ignores
the `monitor` option. So under `set -m` in a script, no `&` job is `setpgid`'d
into its own group; `fg`'s `waitpid(-pgid, …)` then `ECHILD`s (the job shares the
shell's group) and returns rc 1 instead of the job's real status.

### bash behavior
bash honors `set -m` non-interactively: a backgrounded job gets its own process
group and `fg`/`bg` work in a script. Terminal control (`tcsetpgrp`) is applied
only when there is a controlling terminal; with redirected/no tty it is skipped
and `fg` still `waitpid`s the job's group to completion.

### Design
1. **Activate job control under `monitor`.** Change `job_control_active()` to:
   ```rust
   (self.is_interactive || self.shell_options.monitor)
       && !self.in_subshell && !self.in_completion
   ```
   Every spawn-site `pgid_target` decision keys off this (`executor.rs:2835`,
   `2886`, `5903`, the pipeline spawner), so under `set -m` background jobs now
   become their own process-group leaders non-interactively, and `fg`'s
   `waitpid(-pgid)` succeeds.
2. **Terminal control degrades without a tty.** `give_terminal_to`/`tcsetpgrp`
   are gated behind `interactive = job_control_active() && matches!(sink,
   Terminal)`, so a script with redirected stdout already skips them. Add an
   explicit `isatty(STDIN)` guard so that even a `Terminal` sink with no
   controlling tty (e.g. `set -m` under a pipe) does not `tcsetpgrp` — `fg`/`bg`
   then reduce to `setpgid` + `waitpid` on the job's group, which is the observed
   bash behavior. Confirm the existing interactive tty path is unchanged.

### Risk
This broadens process-group creation across every exec path under `set -m`. It
is the riskiest of the three. Mitigation: run the full existing job-control test
suite (`jobs`/`jobcontrol-pty` integration bins, `fg_bg_tests`) plus new `set -m`
cases, and the whole bash-diff sweep, on the branch. Interactive behavior (the
common case, `is_interactive` already true) is unchanged because `|| monitor`
only *adds* activation.

Related deferred: async `[N]+ Done` notices under non-interactive `set -m`
(#158 b) and the broader job-control-pgroup area (#45) remain out of scope.

---

## Testing

New bash-diff harnesses under `tests/scripts/` (byte-identical vs bash 5.2.21):
- `job_prune_diff_check.sh` (#175): `bg+wait` → empty `jobs`; N-iteration
  bg+wait loop → `jobs | wc -l` == 0; bg-without-wait → pruned; a Stopped/Running
  job is retained; `wait $!` prunes its target.
- `job_command_line_diff_check.sh` (#80): full command line for simple/pipeline/
  and-or/redirect/quoted background forms (`jobs` output, prologue-normalized).
- `set_m_jobcontrol_diff_check.sh` (#167): `set -m; cmd & fg %1` rc + output;
  `bg` on a stopped job; guarded so it runs where a pty isn't required (rc +
  stdout, tty-independent cases).

Plus unit tests: `render_job_command` cases in the executor tests; a
`wait_for_job`-prunes assertion and a between-command-prune assertion in
`jobs`/`fg_bg_tests`. The existing job-control integration bins
(`jobs`, `jobcontrol-pty`, `disown_h_integration`) and the full diff sweep must
stay green on both binaries.

## Scope / non-goals
- **In scope:** the three divergences above.
- **Out of scope (deferred):** async Done notices under non-interactive `set -m`
  (#158 b), the wider job-control-pgroup work (#45), exact original-quote-char
  reproduction in `jobs` display.

The merged PR closes #175, #80, and #167.
