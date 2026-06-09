# huck v125 — redirections on a function-call command (M-117) Design

**Status:** approved design, ready for implementation plan.
**Implements:** M-117 — apply a redirect attached to a *function-call* command
(`func >file`, `func 2>&1`, `func >&2`, `func <file`, …) to the function body.
**Why now:** this is the real cause of `nvm ls`'s `→ ∞` (deferred from v124).
nvm's `nvm_err () { >&2 nvm_echo "$@"; }` is a `>&2` on a function call
(`nvm_echo`); huck ignores it, so the error text lands on stdout, leaks into
nvm's `$( … 2>/dev/null )` capture, and trips its alias-cycle sentinel.
**Branch (impl):** `v125-function-call-redirects`.

## Background — the bug (root-caused v124)

`run_exec_single`'s function-call branch (`src/executor.rs:~2844`) is:
```rust
} else if !bypass_functions && let Some(body) = shell.functions.get(&resolved.program).cloned() {
    call_function(&resolved.program.clone(), body, resolved.args, shell, sink)
}
```
It passes `sink` to `call_function` but **never applies `cmd.stdin` / `cmd.stdout`
/ `cmd.stderr`** (the redirects on the function-call command). So the body runs
with the shell's own fds. Probed vs bash:

| fragment | bash | huck (pre-fix) |
|---|---|---|
| `f(){ printf '%s\n' B; }; f >/tmp/x; cat /tmp/x` | `B` | (empty — `B` hits terminal) |
| `a=$(echo Z >&2)` *(builtin — fixed in v124)* | `[]` | `[]` (v124) |
| `f(){ printf '%s\n' B; }; a=$(f >&2 2>/dev/null); echo "[$a]"` | `[]` | `[B]` |
| `g(){ printf E >&2; }; b=$(g 2>&1); echo "[$b]"` | `[E]` | `[]` |

So **all** redirect forms on a function call are dropped, not just `>&N`. (v124
fixed `>&N` on *builtins* directly, a narrower path; the function-call path is
this iteration.)

## Architecture — reuse the v97 compound-redirect mechanism

A function body is a compound command, and huck already has the right machinery:
`run_redirected` (`src/executor.rs:498-597`) applies trailing redirects to a
compound at the real-fd level via `CompoundRedirectScope` (dup2 onto fd 0/1/2,
originals saved and restored on `Drop`), AND handles the capture-context
subtlety: when a stdout redirect is present it forces a `Terminal` inner sink
(lines 587-592) so builtins write via `io::stdout()` (= the redirected fd 1 =
the target) and externals inherit the redirected fd; the `$()` capture then
correctly receives nothing for the diverted stream. `apply_out_redirect`
(`:602`) already handles `>`/`>>`/`>&N`/`2>&N`/Clobber; the stdin arm handles
`<file`/heredoc/here-string.

`run_redirected` has exactly **one** caller (`:441`, the compound-command path),
so it is safe to refactor.

### Component 1 — extract a closure-based scope helper

Generalize the scope mechanism so it can run *any* inner action, not just a
`Command`:
```rust
/// Applies stdin/stdout/stderr redirects at the real-fd level (saved/restored
/// via CompoundRedirectScope), forcing a Terminal inner sink when a stdout
/// redirect is present so the redirect wins over an outer capture, then runs
/// `run_inner`. A redirect-open failure prints `huck: <target>: <err>` and
/// returns Continue(1) WITHOUT running `run_inner`.
fn with_redirect_scope<F>(
    stdin: &Option<Redirect>,
    stdout: &Option<Redirect>,
    stderr: &Option<Redirect>,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    run_inner: F,
) -> ExecOutcome
where
    F: FnOnce(&mut Shell, &mut StdoutSink) -> ExecOutcome,
```
Its body is the current `run_redirected` body verbatim (the flush, the
`CompoundRedirectScope` setup, the stdin/stdout/stderr application, the
`stdout.is_some()` Terminal sink-switch, the final flush + `drop(scope)`),
except the single line `run_command(inner, shell, inner_sink)` becomes
`run_inner(shell, inner_sink)`.

`run_redirected` becomes a thin wrapper (behavior-preserving — the compound
path is unchanged):
```rust
fn run_redirected(inner, stdin, stdout, stderr, shell, sink) -> ExecOutcome {
    with_redirect_scope(stdin, stdout, stderr, shell, sink,
        |shell, inner_sink| run_command(inner, shell, inner_sink))
}
```

### Component 2 — wire into the function-call branch

In `run_exec_single`'s function branch, gate on whether the call carries any
redirect:
```rust
} else if !bypass_functions && let Some(body) = shell.functions.get(&resolved.program).cloned() {
    let name = resolved.program.clone();
    let args = resolved.args;            // (whatever the current call passes)
    if cmd.stdin.is_some() || cmd.stdout.is_some() || cmd.stderr.is_some() {
        with_redirect_scope(&cmd.stdin, &cmd.stdout, &cmd.stderr, shell, sink,
            move |shell, inner_sink| call_function(&name, body, args, shell, inner_sink))
    } else {
        call_function(&name, body, args, shell, sink)   // fast path, unchanged
    }
}
```
(Match the exact `call_function(name, body, args, shell, sink)` argument shape
the current branch uses — `resolved.program`/`resolved.args`/`body`. The closure
must own `body`/`args`/`name`.)

This runs *after* the inline-assignment snapshot is taken (the function branch
already sits inside the inline-assign snapshot/restore region), so
`VAR=x func >f` keeps working: inline assignments are applied/restored as today,
orthogonal to the redirect scope.

## Why this fixes nvm

`>&2 nvm_echo "..."`: `nvm_echo` is a function call with
`cmd.stdout = Dup{fd:1, source:2}`. `with_redirect_scope` → `apply_out_redirect`
dup2's fd 2 onto fd 1, then forces a Terminal inner sink; `nvm_echo`'s body
(`command printf`) writes via `io::stdout()` = fd 1 = wherever fd 2 points. In
nvm's pipeline stage `nvm_alias 2>/dev/null | …`, fd 2 is `/dev/null`, so the
"Alias does not exist." text goes to `/dev/null` — it no longer leaks into the
`$()` capture. `ALIAS_TEMP` is empty → the resolve loop breaks correctly → real
versions resolve → no `→ ∞`.

## Scope & correctness
- Covers the standalone function-call-with-redirect path in `run_exec_single`
  (where nvm's `>&2 funccall` runs, including inside forked pipeline stages —
  the body's commands dispatch through `run_exec_single`).
- A function used as a pipeline *stage* with its own redirect (`func >f | x`)
  already gets fd setup from `run_multi_stage`'s per-stage fork; add a test to
  confirm no interaction, but no code change expected there.
- `>&-`, `>&1`, `2>&1`, `>>`, `<file`, heredoc, here-string on a function-call
  all flow through the existing `apply_out_redirect` / stdin arms.
- **Capture of a function's stdout with NO redirect** (`x=$(f)`) is unchanged
  (fast path keeps the outer `sink`).

## Must-not-regress
- Redirections on compound commands (`while/if/{}/()`/subshell `> file`,
  heredoc-on-`done`) — the `with_redirect_scope` extraction is behavior-
  preserving; the existing compound-redirect tests must stay green.
- No-redirect function calls — fast path, byte-identical.
- Inline assignments on a function call (`VAR=x func`).
- v124's builtin `>&N` and the subshell job-control fix — untouched.
- L-25 (a builtin's `2>&1` under a capture with no stdout redirect) is unrelated
  and unchanged.

## Files & responsibilities

| File | Change |
|------|--------|
| `src/executor.rs` | Extract `with_redirect_scope` from `run_redirected`; make `run_redirected` a wrapper; wire the function-call branch to use it when redirects are present. Unit tests. |
| `tests/function_redirect_integration.rs` (NEW) | Probed cases vs bash (file-arg per L-27). |
| `tests/scripts/function_redirect_diff_check.sh` (NEW) | 48th bash-diff harness. |
| `docs/bash-divergences.md` | DELETE M-117 (Tier-1 2→1). |
| `README.md` | harness count 47 → 48. |

## Testing
1. **Unit** (`executor.rs`): `with_redirect_scope` applies a `>file` (body output
   lands in the file) and a `>&2` (capture empty), restores fds after; the
   no-redirect fast path is taken when all three are `None`. (If constructing
   the closure/inputs is verbose, cover via integration and keep a minimal unit
   check that `run_redirected` still works for a compound.)
2. **Integration** (`tests/function_redirect_integration.rs`, binary vs bash,
   file-arg): `f >/tmp/x` writes to file; `a=$(f >&2 2>/dev/null)` → `[]`;
   `g 2>&1` captures stderr; `func 2>/dev/null` suppresses; `func >&-` discards;
   inline-assign `V=1 f >/tmp/x`; a function whose body has a builtin AND an
   external, both redirected. Byte-identical stdout + file contents + exit.
3. **48th bash-diff harness** `function_redirect_diff_check.sh` — ~7 fragments.
4. **Regression**: full suite, all 48 harnesses, `cargo clippy --all-targets`;
   specifically the compound-redirect tests + v124's subshell/builtin tests.
5. **Payoff (PTY + non-interactive)**: `nvm alias` (non-interactive) and `nvm ls`
   (PTY, `~/.nvm/nvm.sh`) — the alias section resolves real versions, **no
   `→ ∞`**. Report before/after. Honest note: the job-notification noise (L-28)
   and the ~30s runtime are separate and remain.

## Edge cases & notes
- **`func >file` under a direct capture** (`x=$(f >file)`): handled by the
  existing sink-switch — body stdout goes to the file, `x` is empty (matches
  bash). This is the same logic that already makes `x=$( { …; } >file )` work.
- **`>&-` on a function**: routes through `apply_out_redirect`'s Dup handling;
  if `apply_out_redirect` doesn't already special-case the `-` close form,
  match v124's builtin `>&-` choice (route to `/dev/null`) — verify during
  implementation and keep behavior consistent with v124.
- **Source-order** (`func 2>&1 >file`): inherits the pre-existing intentional
  **L-08** (field-based AST can't preserve redirect source order); unchanged.
