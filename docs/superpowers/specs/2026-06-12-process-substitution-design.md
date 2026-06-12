# huck v150 — Process substitution `<(…)` / `>(…)` Design

**Status:** approved design, ready for implementation plan.
**Adds:** process substitution — `<(cmd)` (read from `cmd`'s stdout) and `>(cmd)`
(write to `cmd`'s stdin), usable anywhere a word appears (command arguments AND
redirect targets). Currently `<(` lexes as a `<` redirect followed by `(`, so
`diff <(a) <(b)` is a parse/exec error.
**Branch (impl):** `v150-process-substitution`.

**Portability constraint (standing):** huck must build and run on **macOS** as well as
Linux. This design uses only POSIX primitives (`pipe`, `fork`, `mkfifo`, `waitpid`) and
never references `/proc`; `/dev/fd` is detected at runtime (a real `fdescfs` directory
on macOS, a `/proc/self/fd` symlink on Linux — we only ever name `/dev/fd/N`). Any
platform-specific constant gets a `#[cfg(target_os = …)]` guard.

## Background / how bash behaves

`<(cmd)` runs `cmd` asynchronously with its stdout connected to a pipe and substitutes a
filename that, when opened for **reading**, yields `cmd`'s output (e.g.
`diff <(sort a) <(sort b)`). `>(cmd)` connects `cmd`'s stdin to a pipe and substitutes a
filename that, when opened for **writing**, feeds `cmd`'s stdin (e.g. `tee >(gzip >a.gz)`).
On Linux/macOS bash realizes the filename as `/dev/fd/N` when `/dev/fd` is available, else
a named FIFO. The inner process runs concurrently with the outer command; `$!` is NOT set
to its pid, and the outer command's exit status is unaffected by it. Process substitution
is recognized only when **unquoted** (inside `"…"` the `<(`/`>(` are literal — unlike
`$(…)`, which is active in double quotes). The substituted `/dev/fd/N` word is a single
field (no further word-splitting or globbing).

## Architecture

### 1. AST — new word part

Add to `WordPart` (`src/lexer.rs:199`), alongside `CommandSub`:

```rust
ProcessSub { sequence: crate::command::Sequence, dir: ProcDir },
```

with `pub enum ProcDir { In, Out }` (`In` = `<(`, `Out` = `>(`). No `quoted` field — a
`ProcessSub` part is only ever produced when unquoted (see lexing). The drift-guard
exhaustive `match WordPart` sites must each gain a `ProcessSub` arm.

### 2. Lexing

In the word scanner, when an **unquoted** `<` or `>` is immediately followed by `(`,
treat it as the start of a process substitution rather than a redirect operator:
- consume `<(` / `>(`,
- lex the inner text as a `Sequence` using the same recursive body-lexer that `$(…)`
  uses (`tokenize_partial` / the cmdsub body path), balancing nested parens, quotes, and
  `$( )`/backticks, up to the matching `)`,
- push `WordPart::ProcessSub { sequence, dir }`.

Gating rules (match bash):
- Only when **unquoted**: inside double quotes `<(`/`>(` stay literal (the existing
  redirect/literal handling applies). The redirect operators `<`/`>`/`<<`/`<<<`/`>>`/`&>`
  etc. are unaffected — only a `<`/`>` *immediately* followed by `(` and not part of a
  multi-char redirect operator becomes a process substitution.
- A `<`/`>` that begins a real redirect (`< file`, `<< EOF`, `<<<`, `<&`, etc.) is
  unchanged — the trigger is specifically the two-character `<(` / `>(`.

Lexer unit tests assert: `<(a)` / `>(a)` produce a single-word `ProcessSub`; `"<(a)"`
stays literal; `< (a)` (space) is still a redirect; `cat <(echo hi)` tokenizes to
`cat` + a `ProcessSub` word.

### 3. Expansion-time fork (`src/expand.rs`)

`expand(word, shell)` (`src/expand.rs:766`) already holds `&mut Shell`. When it
encounters a `ProcessSub` part it performs setup and yields the path as a single literal
field segment:

1. **FD realization** (helper, e.g. `crate::procsub::realize`):
   - If `/dev/fd` is usable (cached runtime check): create a `pipe()`. For `dir = In`,
     the parent keeps the **read** end (`pr`) and the inner gets the **write** end as
     fd 1; the field becomes `/dev/fd/{pr}`. For `dir = Out`, the parent keeps the
     **write** end (`pw`) and the inner gets the **read** end as fd 0; the field becomes
     `/dev/fd/{pw}`. The parent end must NOT be close-on-exec (it has to survive the
     outer command's `fork`+`exec`).
   - Else (FIFO fallback): `mkfifo` a unique path under `$TMPDIR` (or `/tmp`); the field
     is that path; the inner opens its end, the parent leaves the path for the outer
     command to open. Record the path for `unlink` at cleanup.
2. **Fork the inner** via `fork_and_run_in_subshell` (`src/executor.rs:4838`), wrapping
   `sequence` as a subshell command, wiring the inner end to fd 0 (`Out`) or fd 1 (`In`),
   and closing the parent end in the child (`parent_fds_to_close`).
3. **Record cleanup**: push a `ProcSub { pid, parent_fd, fifo_path: Option<PathBuf> }`
   onto a new `Shell` field `procsub_pending: Vec<ProcSub>`.
4. Return the path as the field text (single field; not split or globbed).

### 4. Lifecycle / cleanup (executor-owned)

The executor owns teardown so the inner processes live exactly as long as the outer
command. At the command boundary — `run_exec_single` (`src/executor.rs:2980`) for simple
commands, and the redirect-target expansion in `with_redirect_scope` / `run_redirected`
(`src/executor.rs:582`/`699`) for redirect operands on simple *and* compound commands:

- snapshot `let base = shell.procsub_pending.len();` **before** expanding the command's
  argument words and redirect targets,
- run the outer command (external `fork`+`exec`, builtin, or function),
- **drain** `shell.procsub_pending[base..]`: for each entry close `parent_fd`, `unlink`
  any `fifo_path`, and `waitpid` the `pid` (it has finished — the pipe is closed, so a
  `<(` inner has hit EOF/`SIGPIPE` and a `>(` inner has seen stdin EOF).

These two sites **nest**: for `cmd arg < <(p)`, `with_redirect_scope` snapshots first
(`base_r`), opens the redirect (forking the redirect-target procsub), then runs
`run_exec_single`, which snapshots again (`base_a`), forks the argument procsubs, runs
the command, and drains `[base_a..]`; finally `with_redirect_scope` drains `[base_r..]`
(now just the redirect procsub) after the command returns. Each layer drains only the
entries it added, so the redirect-target procsub correctly outlives the command while the
argument procsubs are torn down with it.

Snapshot/drain (rather than clear-all) makes nesting and per-pipeline-stage cleanup
correct. `$?` is set only by the outer command; `$!` is never set to a procsub pid.

### 5. Reuse

- Inner execution: `fork_and_run_in_subshell` (job-control reset, SIGPIPE default,
  stdout flush — all already correct and POSIX).
- Redirect-target procsubs ride the existing `with_redirect_scope` path; opening
  `/dev/fd/N` or the FIFO uses the normal redirect open.

## Files & responsibilities

| File | Change |
|------|--------|
| `src/lexer.rs` | `WordPart::ProcessSub` + `ProcDir`; recognize unquoted `<(`/`>(` in the word scanner, lex inner as a `Sequence`; lexer unit tests. |
| `src/procsub.rs` (new) | `ProcSub` record, runtime `/dev/fd` detection (cached), `realize(dir, …) -> (path, ProcSub)` (pipe-or-FIFO + fork inner), and `cleanup(ProcSub)` (close fd / unlink FIFO / reap). Keeps the platform logic in one small unit. |
| `src/expand.rs` | `expand()` `ProcessSub` arm → `procsub::realize` + push onto `shell.procsub_pending`; emit the path as a single field. |
| `src/shell_state.rs` | `Shell.procsub_pending: Vec<ProcSub>` + a `drain_procsubs(base)` helper. |
| `src/executor.rs` | snapshot/drain around `run_exec_single` and redirect-target expansion; wrap the inner `Sequence` as a subshell `Command` for `fork_and_run_in_subshell`. Add `ProcessSub` arms to exhaustive `WordPart` matches. |
| `src/command.rs` | any `WordPart` match (e.g. `try_split_assignment`, reconstruction) gains a `ProcessSub` arm. |
| `tests/scripts/process_sub_diff_check.sh` | bash-diff harness (content-consuming cases). |
| `docs/bash-divergences.md` | No existing entry to delete; add a `[deferred]` entry only for any follow-on gap discovered. |

## Behaviour matrix (target = bash)

| Input | Result |
|---|---|
| `cat <(echo hi)` | `hi` |
| `diff <(printf 'a\nb\n') <(printf 'a\nc\n')` | bash-identical diff |
| `comm -12 <(sort f1) <(sort f2)` | bash-identical |
| `echo foo | tee >(cat) >/dev/null` | `foo` (from the inner `cat`) |
| `while read x; do echo "[$x]"; done < <(seq 3)` | `[1] [2] [3]` |
| `cat <(echo a) <(echo b)` | `a` then `b` (two independent procsubs) |
| `"<(echo hi)"` | literal `<(echo hi)` (quoted → not a procsub) |
| `wc -c < <(echo hello)` | `6` (procsub as a redirect source) |
| nested `cat <(cat <(echo deep))` | `deep` |

## Edge cases & error handling

- **Quoting:** `<(`/`>(` inside `"…"` or `'…'` are literal — only unquoted forms fork.
- **Single field:** the `/dev/fd/N` (or FIFO) path is one field; never IFS-split or globbed.
- **Failure:** if `pipe`/`fork`/`mkfifo` fails, print `huck: <error>` to stderr and fail
  the command gracefully (rc ≠ 0); never panic. Any partially-set-up fds/FIFOs are
  cleaned up.
- **`$!` / `$?`:** procsub pids are not exposed via `$!`; the outer command's status stands.
- **Exotic contexts:** the supported targets are command arguments and redirect operands.
  A `<(cmd)` used as a bare command name, or in an assignment RHS, still forks and is
  drained at the enclosing statement; if its fd is closed sooner than bash in such a
  context, that is a documented minor divergence (log a `[deferred]` entry if observed).

## Testing

1. **Lexer unit tests** (`src/lexer.rs`): `<(`/`>(` → `ProcessSub`; quoted forms inert;
   `< file` / `<< EOF` / `<<<` unaffected; nested-paren body balances.
2. **`process_sub_diff_check.sh`**: byte-identical bash↔huck over the behaviour matrix,
   using **content-consuming** fragments only (never assert the literal `/dev/fd/N`, whose
   number legitimately differs between shells). Run against a modern bash (5.x); on macOS
   that means Homebrew bash, not the 3.2 system bash.
3. **Full regression:** suite + all harnesses green; clippy clean.

## Notes

- **macOS:** `/dev/fd/N` and `mkfifo` both work on macOS; the runtime `/dev/fd` check and
  the FIFO fallback keep the feature portable. No `/proc` references.
- **Git safety:** implementer subagents must NOT `git checkout <sha>`; the controller
  verifies the branch tip before merge. Commit trailer:
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
