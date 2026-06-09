# huck v121 — suppress job control during completion-function invocation (M-116) Design

**Status:** approved design, ready for implementation plan.
**Implements:** **M-116** (Tier-1 bug) — interactive TAB completion hangs the
shell whenever a completion function runs an external command / pipeline.
**Why now:** with v118/v119 making `_init_completion` byte-identical, live
`mise<TAB>` / `ls -<TAB>` reach real completer functions. bash-completion's
`_longopt` (which completes `ls`, `grep`, and most commands) runs
`LC_ALL=C $1 --help 2>&1 | while read -r line; …`. huck invokes the completer
with a **Terminal** sink and no job-control suppression, so the inner pipeline
takes the interactive job-control path and **hands the controlling terminal to a
new process group while huck is mid-line-edit** → the shell wedges (the
`while read` reader spins on the raw tty, 140+ empty reads; Ctrl-C can't
recover). This makes bash-completion unusable interactively.
**Branch (impl):** `v121-completion-jobcontrol`.

## Background — the bug (root-caused this session; headless pty repro exists)

Isolation matrix (interactive TAB, completer body inside `$( … )`):

| completer producer | result |
|---|---|
| builtin (`printf … \| while read`) | ✅ completes |
| builtin + `2>&1` | ✅ completes |
| **external (`ls --help \| while read`)** | ❌ **HANG** (140+ `read -r line`, never EOF) |
| external, no `2>&1` | ❌ HANG |

Headless (script mode / normally-called function) the external-producer pipeline
**always works** (`ls --help | while read …; echo DONE` → DONE; inside `$()`
→ n=139; inside a normal function call → n=139). So the trigger is **exclusively
the interactive-completion invocation path**.

**Root cause (confirmed by code reading):**
- `call_completion_function` (`src/completion_spec.rs:294`) runs the completer
  via `call_function_body`, which uses `sink = StdoutSink::Terminal`.
- A pipeline inside the completer hits `run_multi_stage` (`src/executor.rs:3113`),
  whose job-control gate is `let interactive = matches!(sink, StdoutSink::Terminal)
  && !shell.in_subshell;` (`:3125`). During completion `sink==Terminal` and
  `in_subshell==false` → `interactive==true`.
- The interactive path does `setpgid` + `give_terminal_to(pgid)` /
  `tcsetpgrp` (`:3714`, `:3722`) — a controlling-terminal handoff. Performed
  while huck owns the terminal in raw mode for line editing, this deadlocks /
  wedges (the v108 class: a job-controlled pipeline run where it must not be).
- A single external command in a completer hits `run_subprocess`
  (`src/executor.rs:2889`), `let interactive = matches!(sink, StdoutSink::Terminal);`
  (`:2897`, no `in_subshell` guard), which also does `setpgid` + terminal
  handoff when interactive — same wedge for `$(usage …)` etc.
- Builtin producers don't fork a separate process group, so they dodge the
  handoff — which is why `printf | while read` completers work.

bash runs completion functions **synchronously, without job control** (they are
not jobs). huck must do the same.

## Architecture — an `in_completion` flag gating the two job-control decisions

Add a transient `Shell.in_completion` flag, set for the dynamic extent of a
completion-function call, and gate the job-control decisions on it so a
completer's external commands/pipelines run **without** `setpgid`/
`give_terminal_to` (foreground in huck's own process group, EOF'ing pipes
normally). This mirrors v108's `in_subshell` fix, applied to the completion
path. (`in_subshell` is read **only** by the `run_multi_stage` gate at `:3125`
— it does not cover `run_subprocess` — so a dedicated, self-documenting flag is
required.)

### Component 1 — `Shell.in_completion` (`src/shell_state.rs`)
Add `pub in_completion: bool` to `Shell` (init `false`, beside `in_subshell`).

### Component 2 — set it around the completer call (`src/completion_spec.rs`)
In `call_completion_function`, wrap step 5 (the `call_function_body` invocation):
save the prior `in_completion`, set it `true`, call `call_function_body`, then
restore. The flag stays `true` through nested function calls / subshells /
pipelines spawned by the completer (it lives in the shared `Shell`). Restore is
unconditional (the call returns an `ExecOutcome`, no panic path; restore right
after).

### Component 3 — gate the two job-control sites (`src/executor.rs`)
- `run_multi_stage` (`:3125`): `matches!(sink, StdoutSink::Terminal) &&
  !shell.in_subshell && !shell.in_completion`.
- `run_subprocess` (`:2897`): `matches!(sink, StdoutSink::Terminal) &&
  !shell.in_completion`.
- The `2>&1` dup-target sites (`:2744`, `:2801`, both `matches!(sink, Terminal)`
  to pick fd 1 as the dup source) are **NOT** job-control and are left
  unchanged.

## Scope & correctness
- Only the job-control decision changes, and only while `in_completion` is set.
  Outside completion the flag is `false` → behavior identical to today.
- Completion-invoked external commands/pipelines run foreground in huck's
  process group (no terminal handoff), synchronous and short-lived — matching
  bash, which never job-controls completion functions.
- The pipe between a forked external producer and a `while read` reader EOFs
  normally on the non-job-control path (proven: this exact pipeline works in
  script mode, which is the non-job-control path).
- v108's subshell job-control suppression is untouched (`in_subshell` term
  retained at `:3125`).

## Must-not-regress
- Interactive job control OUTSIDE completion: Ctrl-Z stops a foreground command,
  `fg`/`bg`, `&` background, terminal handoff for `vim`/`less` — all unchanged
  (the existing `pty_interactive` job-control suite must stay green).
- v108: subshell-internal pipelines under a controlling terminal (no deadlock).
- Completion correctness: a completer with a builtin producer, a `-W` wordlist,
  a `-F` function setting `COMPREPLY`, and default file completion all still
  produce the right candidates.
- `$?` / `COMPREPLY` save-restore in `call_completion_function` unchanged.

## Files & responsibilities

| File | Change |
|------|--------|
| `src/shell_state.rs` | add `in_completion: bool` field (+ init) |
| `src/completion_spec.rs` | set/restore `in_completion` around `call_function_body` |
| `src/executor.rs` | gate `run_multi_stage:3125` + `run_subprocess:2897` on `!in_completion` |
| `tests/pty_interactive*.rs` | NEW pty regression test(s): external-pipeline completer + (optional) real bash-completion `ls -<TAB>` |
| `docs/bash-divergences.md`, `README.md` | M-116 `[fixed v121]`; changelog; README row; Tier counts |

## Testing
1. **pty regression test (the gate)** in the existing `pty_interactive` harness:
   register a completer that runs an EXTERNAL-producer pipeline, e.g.
   ```
   f(){ COMPREPLY=($(ls --help 2>&1 | while read -r l; do printf '%s\n' "$l"; done | head -3)); }
   complete -F f c
   ```
   then drive `c <TAB>` under a wall-clock timeout and assert it RETURNS (a
   completion happens / the prompt redraws) rather than hanging. This reproduces
   the bug class without depending on the system bash-completion. Before the
   fix it must hang (verify the test fails on the parent commit); after, pass.
2. **Real-case pty test** (best-effort, skip if the file is absent): source
   `/usr/share/bash-completion/bash_completion`, drive `ls -<TAB>`, assert it
   returns within a timeout with `-`-options offered.
3. **Job-control non-regression**: the existing `pty_interactive` Ctrl-Z /
   fg / bg / terminal-handoff tests stay green (the flag is `false` there).
4. **Full suite** + clippy `--all-targets`.
5. **Manual payoff**: the headless pty repro (`source …/bash_completion; ls
   -<TAB>`) terminates with completions and the shell stays responsive.

## Edge cases & notes
- **Nested completion** (a completer that itself triggers completion): the
  save/restore makes `in_completion` re-entrant-safe.
- **Completer forks a subshell**: the forked child copies the `Shell` (so
  `in_completion` stays true there) and also sets `in_subshell`; either flag
  suppresses job control. Fine.
- **`run_subprocess` lacks the `in_subshell` guard today**: out of scope —
  v121 only adds the `in_completion` term there; the subshell behavior of a
  single external command is unchanged (not part of this bug).
- **Backgrounded jobs started from inside a completer**: not a real scenario
  (completers don't background jobs); if one did, it simply wouldn't get its own
  pgroup during completion — acceptable and bash-like.
- The `mise<TAB>` no-candidates issue is SEPARATE (the 2.11-vs-2.12
  bash-completion API mismatch + the `>|` parse gap) and out of scope for v121.
