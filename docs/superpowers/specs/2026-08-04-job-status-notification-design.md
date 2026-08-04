# v352 — asynchronous job-status notification

**Date:** 2026-08-04
**Issues:** [#418](https://github.com/jdstanhope/huck/issues/418) (no asynchronous
`[N]+  Stopped` notice) and [#420](https://github.com/jdstanhope/huck/issues/420)
(Done/Terminated jobs never announced; stop/signal state strings diverge)

## Problem

huck never announces a background job's state change in a non-interactive
shell, even with job control on:

```
$ bash -c 'set -m; sleep 0.1 & sleep 0.4; echo MARK'
[1]+  Done                    sleep 0.1
MARK
$ huck -c 'set -m; sleep 0.1 & sleep 0.4; echo MARK'
MARK
```

The machinery is all there. `jobs::reap_and_notify` reaps, drains the
newly-changed jobs, prints, and prunes — but its print is gated on
`shell.is_interactive`, where bash's gate is *job control active*. That one
condition accounts for the missing Stopped notice (#418) and the missing
Done/Terminated notice (#420 item 2) alike.

**#420 item 1 is withdrawn.** It claimed `jobs` never lists a finished job.
It does not: the `[1]+  Done` line in that report was the asynchronous notice
firing at the command boundary, not `jobs` output. bash prunes a reported
terminal job exactly as huck does:

```
$ bash -c 'set -m; sleep 0.1 & sleep 0.4; jobs; echo MID; jobs; echo END'
[1]+  Done                    sleep 0.1     <- the notice, before the first jobs
MID
END                                          <- both listings print nothing
```

What remains beyond the gate is content: the state strings, a second notice
form for signal deaths, and the trailing `&`.

## Measured behavior

Everything below was probed against bash 5.2.21 on the compat target, not
recalled. Both forms are written to **stderr**.

### Which form

A job that died from a signal is announced one of two ways:

```
[1]+  Terminated              sleep 5                          <- job-line form
bash: line 3: 4179740 Killed                  sleep 5          <- pid form
```

The pid form is used when **all** of: the shell is non-interactive, the job
died from a signal, that signal is not SIGINT / SIGTERM / SIGPIPE, and the
signal is not trapped. Otherwise the job-line form is used. Measured:

| signal | form |
|---|---|
| INT, TERM, PIPE | `[N]+  Interrupt` / `Terminated` / `Broken pipe` |
| HUP, KILL, USR1, USR2, ALRM, STKFLT, VTALRM, PROF, IO, PWR | pid form |
| any of the above, trapped | job-line form |

### State strings

| state | text |
|---|---|
| Running | `Running` |
| Stopped, **any** stop signal | `Stopped` |
| Done(0) | `Done` |
| Done(n) | `Exit n` |
| Signaled(s) | `strsignal(s)` |
| Signaled(s), core dumped | `strsignal(s) + " (core dumped)"` |

`strsignal(3)` from libc produces bash's exact wording for every terminated
job measured — `Hangup`, `Interrupt`, `Killed`, `Terminated`, `Broken pipe`,
`Alarm clock`, `User defined signal 1`, `Stack fault`, `Virtual timer
expired`, `Profiling timer expired`, `I/O possible`, `Power failure`, `Quit` —
because bash builds its table from the same system list. huck therefore calls
`strsignal` rather than transcribing a table, which also keeps the text
correct on non-Linux targets.

The **stopped** path does not use `strsignal`: glibc returns `Stopped
(signal)` for SIGSTOP and `Stopped (tty input)` for SIGTTIN, but bash prints a
plain `Stopped` for SIGSTOP, SIGTSTP and SIGTTIN alike. Verified both
non-interactively and under a PTY. huck's current `Stopped (tty input)` /
`Stopped (tty output)` / `Stopped (signal N)` strings are wrong and become a
literal `Stopped`.

### Trailing `&` and the leading newline

bash appends ` &` to a **Running** line only; its `Done`, `Exit n`, `Stopped`
and signal lines carry no `&`. huck appends it to everything that is not
Stopped.

A **Stopped** notice is preceded by a bare newline; Done and Terminated
notices are not — confirmed with `cat -A`:

```
$\n[1]+  Stopped                 sleep 5$      <- leading blank line
[1]+  Done                    sleep 0.1$       <- none
```

huck's executor already emits `\n` before the notice for a stopped foreground
subshell, so this rule is consistent with code that exists; it moves into the
new decision function so there is one owner.

## Design

### `job_notice` — one pure decision

`jobs.rs` gains a decision function and the two shapes it can return:

```rust
enum Notice {
    /// `[1]+  Terminated              sleep 5`
    JobLine(String),
    /// `huck: line 3: 4179740 Killed                  sleep 5`
    SignalLine { pid: i32, body: String },
}

struct NoticeCtx { interactive: bool, job_control: bool, trapped: bool }

fn job_notice(job: &Job, flag: char, ctx: NoticeCtx) -> Option<Notice>
```

`SignalLine.pid` is the job's leader pid (the first entry of `job.pids`,
falling back to `pgid`), and its body is that pid, a space, the state text in
the same 24-column field the listing uses, then the command — i.e. exactly
`notification_line_long`'s first line without the `[N]<flag>` prefix and
without the trailing `&`.

`None` means say nothing: job control off, or a state bash does not announce.
The function is Shell-free, so the whole decision table is unit-testable
without spawning a process — which is the point, since this feature's bugs
live in the per-signal matrix rather than in the plumbing.

Rendering stays where it is. `render_state` gains the `strsignal` lookup and
loses the wrong stop strings; `job_state_and_suffix` restricts ` &` to
Running; `notification_line` / `notification_line_long` are untouched (they
are byte-identical to bash after #410 and #426).

### `reap_and_notify` — a thin printer

The flow is unchanged in shape:

1. `reap_completed`
2. `drain_notifications` — already returns Stopped as well as terminal jobs
3. per job: compute the `+`/`-`/blank flag, build `NoticeCtx`, call
   `job_notice`, print to stderr
4. `remove_notified` — terminal jobs drop once reported, Stopped jobs stay

`JobLine` prints directly. `SignalLine` goes through `sh_error_to!` so its
`prog: line N:` prologue and line number come from the existing error
machinery rather than a second implementation. `ctx.trapped` is read from the
shell's trap table at print time, keeping `job_notice` Shell-free.

### Gate

`ctx.job_control` is `is_interactive || shell_options.monitor`. huck's
existing suppression inside a subshell and inside completion functions is
preserved unchanged.

No new call sites: `reap_and_notify` already runs at the executor's
per-command-group boundary, in the sourced/eval per-unit loop, after `wait`,
and at each REPL prompt. Letting those existing passes speak puts the notice
before the next command's output, which is where bash puts it.

## Risks and open questions

**The line number in the pid form.** bash reports the line executing when the
death was noticed — in a four-line script whose `sleep 0.3` is on line 3, bash
prints `line 3` although the notice appears just before line 4 runs. huck's
`current_lineno` at that hook may be the next command's line. The harness
compares this number (only the program name and pid are normalized), so a
mismatch surfaces immediately. **The plan opens with a spike that measures
huck's line number at that boundary** and decides then between fixing the
number and documenting a normalization.

**`set -m` harness churn.** Turning notices on non-interactively changes the
output of every existing harness fragment that uses `set -m`, including three
added on 2026-08-03. They are compared against bash, so the expectation is
that they move toward it; fragments whose timing differs may need a filter or
a reordering. The full sweep is the gate, and this churn is expected work, not
a surprise.

**Core-dumping signals are nondeterministic on this box.** `core_pattern`
pipes to apport, which leaves a SIGQUIT/SIGABRT/SIGSEGV'd child visibly alive
for seconds — long enough that `jobs` still reports it Running. They stay out
of the harness (a comment records why); the `(core dumped)` suffix is covered
by a unit test instead.

## Non-goals

- **huck never notices SIGTSTP/SIGTTIN/SIGTTOU stops at all.** Its background
  children carry `SigIgn` `0x380000` — signals 20/21/22 ignored — where bash
  ignores them only when job control is off. That is a fork-time signal
  disposition bug, a different layer from notification, and is filed as
  [#428](https://github.com/jdstanhope/huck/issues/428).
- No new PTY assertions. The interactive path shares the formatter, so the
  harness exercises the same text; the three PTY suites must stay green.
- No change to the `jobs` builtin listing path.
- No notices at shell exit; bash does not announce then either.

## Testing

**New harness** `tests/scripts/job_notify_diff_check.sh` — byte-identical
against bash 5.2.21, normalizing the program name and the pid, **asserting**
the line number:

- the gate: a notice with `set -m`, silence without it
- `Done` and `Exit n`
- job-line form via TERM / INT / PIPE
- pid form via HUP / KILL / USR1 / ALRM
- a trapped signal flipping the form back to the job line
- the `Stopped` notice including its leading blank line
- two jobs finishing in one window, for ordering
- a notice after `wait`, and `jobs` printing nothing afterwards
- core-dumping signals excluded by construction, with the reason in a comment

**Unit truth table** over `job_notice` and `render_state`: the form decision
across interactive × signal-class × trapped, `strsignal` text for the standard
signals, the `(core dumped)` suffix, the stop-string collapse, and the `&`
rule.

**Tests that assert today's divergent behavior** and will be re-pointed with
the probe evidence in their comments: three `jobs::tests` stop-string cases
(`Stopped (tty input)`, `Stopped (tty output)`, `Stopped (signal N)`) and any
expectation carrying `Killed (signal N)`.

**Regression bar:** full `run_diff_checks.sh` green; engine lib green at
`--test-threads 4`; the job/wait/disown integration binaries green; and
`subshell_job_notice_pty`, `jobcontrol_pgroup_pty`, `completion_jobcontrol_pty`
green, since they share the formatter.

## Files

- `crates/huck-engine/src/jobs.rs` — `Notice`, `NoticeCtx`, `job_notice`,
  `render_state`, `job_state_and_suffix`, `reap_and_notify`
- `crates/huck-engine/src/jobs.rs` tests — truth table, re-pointed cases
- `tests/scripts/job_notify_diff_check.sh` — new
- existing `set -m` harnesses — adjusted only where the new notices change
  their output
