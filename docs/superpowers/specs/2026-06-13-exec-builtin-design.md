# huck v155 — the `exec` special builtin Design

**Status:** approved (retroactive — design written after a tested implementation; see note).
**Adds:** the `exec` special builtin in both bash modes — replace the shell process
image (`exec cmd args`) and apply permanent redirections to the shell's own fds
(`exec >log 2>&1`) — plus the `-a NAME` / `-l` / `-c` options. Closes the previously
undocumented gap where `exec` was treated as an unknown command (`command not found: exec`).
**Branch (impl):** `v155-exec`.

> Process note: this iteration was implemented directly on a branch before the
> spec/plan were written (the request was "implement exec", not "start v155: …").
> The work was then retrofitted into the standard flow: this spec and the plan
> document the as-built design, followed by a whole-branch review before merge.

## Background

Before v155, `exec` fell through to PATH resolution and printed
`huck: command not found: exec`, then the rest of the line ran anyway — so
`exec program` did NOT replace the process and `exec >file` did NOT redirect.
bash's `exec` is a POSIX special builtin with two modes; huck implemented neither.
The `is_special_builtin` doc comment already anticipated this ("expand here as huck
adds `exec`").

## Behaviour

### Mode 1 — replace the process image: `exec [opts] command [args]`
Replaces the current process with `command` (no fork) via
`std::os::unix::process::CommandExt::exec`, which only returns on failure. The
replacement inherits the shell's fds (after any redirections — see below) and the
shell's exported environment + exported functions. On success the shell is gone;
the new program's eventual exit status is what the parent sees.

Failure (the command cannot be found/executed):
- print `huck: exec: NAME: <reason>` (`not found` / `cannot execute: …`);
- exit status 127 (ENOENT) or 126 (EACCES/ENOEXEC/EISDIR/…);
- a non-interactive shell EXITS with that status; an interactive shell returns it
  (stays at the prompt). This matches bash's default (no `execfail` shopt needed —
  see Divergences).

### Mode 2 — permanent redirections: `exec [redirections]` (no command)
Applies the command's stdin/stdout/stderr redirections to the shell's OWN fds and
keeps them for the rest of the shell's life (no restore). `exec >file` truncates and
redirects stdout; `exec 2>&1` merges; `exec <file` replaces stdin. Returns 0.
A redirection that fails to open prints a diagnostic and returns 1 — and, matching
bash's default, does NOT exit a non-interactive shell (unlike a failed exec COMMAND).

### Options
- `-a NAME` — use NAME as argv[0] of the replacement.
- `-l` — prepend `-` to argv[0] (login-shell convention); composes with `-a`.
- `-c` — run the replacement with an empty environment.
- `--` — end option parsing.
Clustering (`-cla NAME`) and inline `-aNAME` are supported.

### Ordering & scope
POSIX order: redirections are performed FIRST, then the exec. For `exec` the
redirections are permanent, so a failed exec COMMAND still leaves the redirections
applied (bash parity). `exec` is a special builtin, so inline assignments preceding
it (`V=1 exec cmd`) persist in the shell and are exported into the replacement's
environment. `exec` inside a subshell / `$(...)` / a pipeline stage replaces only
that forked child — the main shell survives — which falls out for free because those
contexts run the command through the same `run_exec_single` path in the child.

## Architecture

`exec` is intercepted in `run_exec_single` (src/executor.rs) right after inline
assignments are applied+exported and after xtrace, but before the
control-builtin/function/builtin/external dispatch — because neither mode fits the
in-process-builtin (`run_builtin` writes to a `Write` sink) nor the fork-an-external
model. Registration: `exec` added to `BUILTIN_NAMES` (so it is recognized, not
"command not found", and `builtin exec` / `command exec` work via the existing strip
loops) and to `is_special_builtin` (inline-assignment persistence). A defensive
`"exec"` arm in `run_builtin` degrades instead of hitting the `unreachable!` if a
future refactor ever routes it there.

New code (executor.rs):
- `parse_exec_flags(args) -> Result<ExecFlags, String>` — `-c`/`-l`/`-a NAME`/`--`,
  clustering + inline `-aNAME`; reports the operand start index.
- `apply_redirects_permanently(cmd, shell) -> Result<(), ()>` — mirrors
  `with_redirect_scope`'s open logic via a `CompoundRedirectScope`, but on success
  closes the saved originals instead of restoring (permanent), and on any open
  failure rolls back already-applied redirects atomically (scope Drop).
- `reset_exec_signals_saving()` / `restore_exec_signals()` — set the job-control
  signals huck `SIG_IGN`s (`SIGTSTP`/`SIGTTIN`/`SIGTTOU`) to `SIG_DFL` for the
  replacement (SIG_IGN is inherited across execve), saving the prior handlers so a
  FAILED exec restores them (exec() does not fork, so the change persists otherwise).
- `run_exec_builtin(resolved, cmd, shell) -> ExecOutcome` — orchestrates: parse flags
  → permanent redirections → (no operand ⇒ return 0) → PATH-resolve via
  `builtins::search_path_for` → build `ProcessCommand` (env, arg0, args) → reset
  signals → `.exec()` → classify failure.

`builtins::search_path_for` is promoted to `pub(crate)` for the PATH resolution
(also handles the `-c` empty-env case, where `ProcessCommand`'s own PATH search
would see an empty `$PATH`).

## Divergences (documented)
- **fd>2 redirections** (`exec 3<file`, `exec {fd}>file`) are unsupported — huck's
  `ExecCommand` AST models only stdin/stdout/stderr. The common `exec 3<file` idiom
  does not work. Tracked separately (a general fd>2 redirect gap, not exec-specific).
- **Error-message wording** differs from bash (`huck: exec: NAME: not found` vs
  `bash: line N: NAME: No such file or directory`) — huck's house style; status codes
  match. So failure cases are status-compared, never byte-compared.
- **No `execfail` shopt.** huck always exits a non-interactive shell on a failed exec
  COMMAND and always returns in an interactive one — which equals bash's default
  behavior; only an explicit `shopt -s execfail` (rare) would differ.
- **`exec -Z 2>/dev/null`**: a bad-option diagnostic is not suppressed by a
  same-command redirect (huck parses exec flags before applying its permanent
  redirect; bash applies the redirect around the builtin). Status (2) matches.
- **Failed exec-redirect signal/zombie minutiae:** a heredoc used as permanent exec
  stdin (`exec <<X`) leaves its transient writer process unreaped until shell exit
  (a bounded zombie; fd is correct). Arcane.

## Testing
1. **Unit** (executor.rs): `parse_exec_flags` — plain command, `-c -l -a NAME`,
   clustered `-cla NAME`, inline `-aNAME`, `--`, bare `-`, errors (`-Z`, dangling
   `-a`), flags-only-no-command.
2. **`exec_diff_check.sh`** — byte-identical bash↔huck for both modes: replace
   process / args / exit codes / env inheritance / inline-assign-exported / `-a` /
   `--` / subshell / pipeline stage / `command exec`; bare-`exec` noop; `>file`,
   `>>file`, `<file` permanent redirections; and status-only failure cases (missing
   command ⇒ 127, failed redirect ⇒ continue, in subshells with stderr suppressed).
3. **Regression:** full suite + clippy green; `type -t exec` ⇒ `builtin`.

## Notes
- Signal save/restore (not a `pre_exec` hook) is deliberate: a `pre_exec` reset would
  fire in-process and, on the rare exec-failure path, leave the interactive shell's
  own job-control signals at `SIG_DFL`. Save/reset/restore avoids that.
- macOS-portable: all libc calls (`signal`, `dup2`, `close`, `getpid`) are POSIX.
- Commit trailer: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
