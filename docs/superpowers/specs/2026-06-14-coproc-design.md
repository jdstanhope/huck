# huck v157 — the `coproc` reserved word Design

**Status:** approved design, ready for implementation plan.
**Adds:** the `coproc` coprocess — `coproc command` (anonymous, default name `COPROC`) and
`coproc NAME compound` (named) — spawning an asynchronous command with two pipes wired to a
`NAME[0]`/`NAME[1]` fd array + `NAME_PID`, plus `$!`, a job entry, and auto-unset on exit.
Built on v156's arbitrary-fd redirection plumbing (the coproc fds are high-numbered and accessed
via `<&${NAME[0]}` / `>&${NAME[1]}`).
**Branch (impl):** `v157-coproc`.

## Background

huck has no `coproc`: the word is parsed as an ordinary command, and `coproc NAME { … }` is a
syntax error. coproc needs (a) a reserved word + AST + parser, (b) a two-pipe async spawn with the
fd-array/pid bookkeeping, and (c) lifecycle cleanup. Its prerequisite — reading/writing the
coprocess via high-numbered fds (`<&${NAME[0]}`, `>&${NAME[1]}`) — shipped in v156 (arbitrary-fd
redirections), so coproc is now implementable.

## Scope (decided)

- **Both forms**: `coproc command [args] [redirects]` (anonymous → `COPROC`) and
  `coproc NAME compound [redirects]` (named; body is a compound command).
- **Single active coproc** reliably, with bash's warning on a second — BUT the state is a
  NAME-keyed collection so full multi-coproc is a later policy relaxation, not a rewrite (user
  decision: "start with one but design to support multiple").
- `NAME[0]`/`NAME[1]` fd array, `NAME_PID`, `$!`, a `jobs` entry, **auto-unset on exit**.
- coproc fds are **close-on-exec** (modern-bash behavior).

## Section 1 — Grammar & AST

### Grammar (bash-faithful)
`coproc` is a reserved word. Two forms:
- `coproc command [args] [redirects]` — anonymous, default name `COPROC` (e.g. `coproc awk '{print}'`).
- `coproc NAME compound [redirects]` — named; the body is a COMPOUND command (`{ …; }`, `( … )`,
  `if`/`while`/`until`/`for`/`case`/`[[`/`((`).

A NAME is recognized ONLY when the body is a compound command. `coproc foo bar` (simple) is the
anonymous form running `foo bar` (NOT name=`foo`). Disambiguation: after `coproc`, if the next
token is a plain word AND the token after it begins a compound command (`{`, `(`, `if`, `while`,
`until`, `for`, `case`, `[[`, `((`), treat the word as NAME; otherwise parse an anonymous simple
command.

### Lexer (`src/lexer.rs`)
Add `Keyword::Coproc`, recognized at command-word position (like `select`/`function`).

### AST (`src/command.rs`)
```rust
// new Command variant
Coproc { name: String, body: Box<Command> },   // name = "COPROC" when anonymous
```
`parse_command` dispatches on `Keyword::Coproc`: optionally consume a NAME per the disambiguation
rule, parse the body via the existing compound/simple-command parsers, and let trailing
redirections wrap the body as today. The body is an ordinary `Command`; coproc only adds the
two-pipe wiring + name bookkeeping around its async execution.

## Section 2 — Spawn, fd wiring & state model

### State (on `Shell`)
```rust
struct Coproc { name: String, pid: libc::pid_t, read_fd: RawFd, write_fd: RawFd }
// Shell field:
coprocs: Vec<Coproc>,   // v157: at most one live (policy); shaped for many later
```

### Two pipes
- `pipe_in` (shell→coproc): child's stdin = `in_r`; shell holds `in_w` → `NAME[1]`.
- `pipe_out` (coproc→shell): child's stdout = `out_w`; shell holds `out_r` → `NAME[0]`.

### Spawn — reuse `fork_and_run_in_subshell`
Spawn the `body` asynchronously via the existing bg-job spawn, passing `stdin_fd=in_r`,
`stdout_fd=out_w`, and `fds_to_close=[in_w, out_r]` (the child must not see the shell's ends),
in its own process group like any bg job. Parent then:
- closes the child ends (`in_r`, `out_w`);
- relocates `in_w`/`out_r` to high fds (≥10) via v156's `alloc_high_fd`, and sets them
  **close-on-exec** (so they don't leak into unrelated children; a child reaches them only via an
  explicit `>&${NAME[1]}` / `<&${NAME[0]}`, where v156's dup-onto-target clears cloexec for that
  one child);
- `NAME` ← indexed array `{0: read_fd, 1: write_fd}` (via `replace_array`); `NAME_PID` ← pid
  (scalar `set`);
- `$!` ← pid (`last_bg_pid`); register a job (`jobs.add`, displayed like `coproc NAME`);
- push a `Coproc` record onto `shell.coprocs`.

### Single-active policy
Before spawning, if `coprocs` already holds a live entry, emit bash's warning
(`huck: warning: execute_coproc: coproc [PID:NAME] still exists`) and proceed. The `Vec` still
tracks every record (so reaping/auto-unset works per-coproc); relaxing to full multi later is just
dropping the warning.

## Section 3 — Lifecycle, errors

### Auto-unset on coproc exit (bash-faithful)
Hook the existing reap path (`reap_completed` → `jobs.reap`): when a reaped/`Done` pid matches a
`coprocs` entry, the shell CLOSES its `read_fd`/`write_fd` (if still open), UNSETS the `NAME`
array and `NAME_PID`, and drops the record. Like bash, this happens at the next reap point (REPL
prompt / after a command), so `${NAME[0]}` reads as live until the coproc dies and is reaped, then
becomes empty. `wait $NAME_PID` works (registered job). On shell exit, the fds close with the
process.

### Error handling
`pipe()`/`fork()` failure → diagnostic, no record created, `NAME`/`NAME_PID` left unset, rc 1. The
single-active case is a WARNING (proceeds), not an error.

## Divergences (documented in `docs/bash-divergences.md`)
- **Multiple simultaneous coprocs** — `[deferred]`: v157 reliably supports one active coproc and
  warns on a second; full independent multi-coproc tracking is deferred (state is already a `Vec`,
  so it is a policy relaxation). bash itself only half-supports multiple.
- coproc fd numbers differ from bash's exact choice (both high; behavior-tested, not byte-compared).
- warning/error wording uses huck's `huck:` prefix.

## Behaviour matrix
- `coproc { read l; echo "got:$l"; }; echo hi >&${COPROC[1]}; read r <&${COPROC[0]}; echo "$r"`
  → `got:hi` (byte-identical to bash — deterministic round-trip).
- Named variant: `coproc MYP { read l; echo "echo:$l"; }; echo yo >&${MYP[1]}; read r <&${MYP[0]}; echo "$r"`
  → `echo:yo`.
- `coproc cat; echo "$COPROC_PID" | grep -qE '^[0-9]+$' && echo pid-ok` → `pid-ok`; `$!` == `$COPROC_PID`.
- after the coproc exits + reap, `echo "[${COPROC[0]-unset}]"` → `[unset]` (auto-unset), matching bash.
- a second `coproc` while one is live → bash-style warning on stderr, proceeds.

## Edge cases / documented divergences
- A coproc whose body is a builtin/function still forks (it runs in the child with the pipe ends);
  matches bash (coproc always forks a subshell-like child).
- fd numbers / warning wording are not byte-compared (behavior-asserted).
- Reading `${COPROC[0]}` after the coproc closed its stdout but before reap yields EOF (read
  returns nothing, rc 1) — matches bash.

## Testing
1. **Unit (parser):** anonymous (`coproc cmd args`) vs named-compound (`coproc N { …; }`)
   disambiguation → correct `Command::Coproc { name, body }`; the word-then-compound rule;
   `coproc foo bar` is anonymous with body `foo bar`.
2. **Integration (`huck -c`, vs bash):** the deterministic read/echo round-trip (anonymous +
   named); `$!` == `$COPROC_PID`; `NAME_PID` is a pid; auto-unset after exit+reap;
   second-coproc warning to stderr.
3. **`coproc_diff_check.sh`:** byte-identical bash↔huck for the deterministic read/echo exchanges;
   pid/fd-number cases asserted behaviorally (is-a-pid, fd ≥ 10), never literal numbers.
4. **Regression:** full suite + ALL existing harnesses (esp. the v156 fd-redirect ones) + clippy green.

## Notes
- macOS-portable: `pipe`, `dup2`, `close`, `fcntl(F_DUPFD/FD_CLOEXEC)`, `fork`/`waitpid` are all POSIX.
- Reuses v156 (`alloc_high_fd`, the redirect appliers for `>&${NAME[1]}`/`<&${NAME[0]}`) and the
  existing bg-job machinery (`fork_and_run_in_subshell`, `jobs`, `last_bg_pid`, `reap_completed`).
- Commit trailer: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- Implementer subagents must NOT `git checkout <sha>`; controller verifies the branch tip before
  merge (see [[feedback-verify-branch-tip-before-merge]]).
