# huck v128 — suppress job-control notifications inside a subshell environment (L-28 noise) Design

**Status:** approved design, ready for implementation plan.
**Implements:** suppress the automatic `&` job notices (`[N] pid` start,
`[N]- Done … &` completion) when running inside a subshell environment, matching
bash — the remaining L-28 symptom (the `nvm ls` job-notice noise). The L-28
alias-line duplication was already fixed by v127.
**Branch (impl):** `v128-subshell-job-notice-suppression`.

## Background — root cause (confirmed via PTY synthetic bisection)

huck prints automatic background-job notices gated **only** on
`shell.is_interactive`. bash ALSO suppresses them when the job is started in a
**subshell environment** (`subshell_environment != 0`: inside `( )`, a pipeline
stage, `$()`). nvm's alias-listing loops (`nvm.sh:1208-1245`) are three blocks of
the form:
```sh
(
  for … ; do … nvm_print_alias_path … & done
  wait
) | command sort
```
— a subshell that is also a pipeline stage. So the `&` jobs (and their `wait`)
run in the forked subshell child (huck sets `in_subshell = true` there). bash is
silent; huck emits a `[N] pid` per `&` and a `[N]- Done … &` per reap → the
~36-line noise the user sees in interactive `nvm ls`.

Probed (PTY, interactive):

| fragment | huck (current) | bash -i |
|---|---|---|
| `sleep 0.05 & wait` (top level) | 2 notices | 2 notices |
| `( sleep 0.05 & wait )` (subshell) | **2** | **0** |
| `( sleep 0.05 & wait ) \| cat` (nvm shape) | **2** | **0** |
| `nvm ls` (real) | **36** | **0** |

The discriminator is purely the subshell environment (not function/loop/nesting/
timing/monitor-mode — all ruled out). huck already has the `in_subshell` flag
(v108) and `in_completion` (v121); this is the same gating pattern.

## Architecture — gate the automatic notice sites on `!in_subshell`

Three sites emit automatic `&` notices, each currently gated only on
`shell.is_interactive`. Change each gate to
`shell.is_interactive && !shell.in_subshell && !shell.in_completion`:

1. **`src/executor.rs:1501`** — start notice for a single backgrounded command
   (`cmd &`): `if shell.is_interactive { eprintln!("[{id}] {pid}"); }`.
2. **`src/executor.rs:2026`** — start notice for a backgrounded group/pipeline
   (`{ …; } &` / `a | b &`, v98/M-95):
   `if shell.is_interactive { eprintln!("[{id}] {last_pid}"); }`.
3. **`src/jobs.rs:320`** — the async done notice in `reap_and_notify`:
   `if shell.is_interactive { eprintln!("{}", notification_line(&job, flag)); }`.
   Gating at this emission site is correct for all three callers: the REPL prompt
   (`shell.rs:305`) and a top-level `wait` (`builtins.rs:3088`) have
   `in_subshell == false` → still notify; a `wait` inside a `( )`/pipeline
   subshell child (`builtins.rs:3088` or `executor.rs:2019`) has
   `in_subshell == true` → suppress. (Reaping + `remove_notified` still happen;
   only the `eprintln!` is gated — so the subshell silently reaps its jobs, as
   bash does.)

`in_completion` is added alongside `in_subshell` for consistency with the
existing job-control gates (bash emits no job noise during completion either);
harmless for the common path.

Why correct: nvm's `&` jobs are both *started* and *reaped* (`wait`) inside the
forked `( … ) | sort` subshell child, where `in_subshell == true` — so both
their start and done notices are suppressed, exactly matching bash's
no-notices-in-a-subshell-environment behavior.

## Scope & must-not-regress
- **Top-level interactive `&`** (`in_subshell == false`): `[N] pid` + `[N]+ Done`
  STILL print (verified huck == bash == 2 for `sleep 0.05 & wait`).
- **User-invoked `jobs` / `bg` / `fg`** output is NOT touched — different sites
  (`builtins.rs` `jobs`/`bg`), invoked deliberately by the user.
- **Ctrl-Z stopped-job notices** (`executor.rs:392/3239/4058`) are NOT touched —
  they already live inside the interactive job-control branches gated on
  `interactive = Terminal && !in_subshell && !in_completion`.
- Non-interactive scripts: unaffected (`is_interactive == false` already
  suppresses; the added conjuncts only narrow further).
- Job *results* and exit statuses are unchanged — this only gates the cosmetic
  notice `eprintln!`s.

## Files & responsibilities

| File | Change |
|------|--------|
| `src/executor.rs` | Add `&& !shell.in_subshell && !shell.in_completion` to the two `&` start-notice gates (`:1501`, `:2026`). |
| `src/jobs.rs` | Add the same conjuncts to the `reap_and_notify` done-notice gate (`:320`). |
| `tests/subshell_job_notice_pty.rs` (NEW) | PTY regression: subshell/pipeline `&` → no notices; top-level `&` → notices present. |
| `docs/bash-divergences.md` | Update L-28: remove the (now-fixed) job-notice + duplication symptoms; if nothing remains of L-28, DELETE it (Tier-4 25→24); else keep the residual. |

## Testing
1. **PTY regression** `tests/subshell_job_notice_pty.rs` (expectrl `OsSession`,
   mirror `tests/completion_jobcontrol_pty.rs`; notices are interactive-only):
   - `( sleep 0.05 & wait ); echo MK` → output has **no** `[N]`-prefixed job
     lines; `MK` arrives.
   - `( sleep 0.05 & wait ) | cat; echo MK` → no `[N]` lines.
   - `sleep 0.05 & wait; echo MK` (top level) → a `[N]` start line DOES appear
     (must-not-regress). (Assert on a normalized capture; skip gracefully with no
     PTY.)
2. **Regression:** full suite + all harnesses + the existing job-control PTY
   suites (`pty_interactive`, `subshell_pipeline_pty`, `completion_jobcontrol_pty`,
   `subshell_tty_pty`) green — confirm top-level job control / Ctrl-Z / `fg`/`bg`
   notices unaffected.
3. **Payoff (PTY):** `nvm ls` via `~/.nvm/nvm.sh` (NOT `~/.bashrc` — creds) →
   **0** `[N]`/`Done` job-notice lines (was 36) and 0 `∞`-dups → byte-clean vs
   bash. Report before/after.

## Edge cases & notes
- A backgrounded job inside `$()` (Capture sink): command substitution runs via
  `run_substitution` (clone, not a fork) — does it set `in_subshell`? If NOT,
  `$(cmd &)` notices wouldn't be suppressed by this change. nvm's case is the
  `( ) | sort` subshell (which DOES set `in_subshell`), so the payoff holds.
  During implementation, check whether `run_substitution`/`execute_capturing`
  sets `in_subshell`; if a `$()`-backgrounded job still leaks a notice and that
  matters, note it as a residual (out of scope here — nvm doesn't hit it).
- The `reap_and_notify` gate suppresses only the `eprintln!`; the job is still
  reaped and removed from the table, so no stale jobs accumulate.
- No change to `$-` / monitor-mode handling (monitor `m` was confirmed ON
  throughout; not the discriminator).
