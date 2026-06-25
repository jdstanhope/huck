# v224 — FUNCNEST enforcement + recursion backstop

## Status

Design approved 2026-06-25. Implements bash's `FUNCNEST` recursion-depth limit
and a defensive internal backstop so deep recursion yields a clean error instead
of a Rust SIGABRT. This is `func`'s LAST blocker (L-61) — fixing it **flips `func`
to PASS (suite count 8→9)**.

## Background

bash limits function-call nesting via `$FUNCNEST`: when set to a positive integer
N, a call that would make the function-call depth exceed N is refused with
`<prologue>NAME: maximum function nesting level exceeded (N)` and returns rc 1
WITHOUT running the body. Unset / `0` / negative / non-numeric = unlimited.
Measured boundary: `FUNCNEST=3` runs depth 1‑2‑3 and refuses the 4th; the refused
call's rc 1 propagates up.

huck does not implement FUNCNEST at all — it ignores the variable and recurses on
the native Rust stack until it overflows and ABORTS (`fatal runtime error: stack
overflow`, SIGABRT, ~2800 nested calls on the default 8 MB stack). The
`func`/func4.sub residual is exactly the missing FUNCNEST enforcement (cases
`FUNCNEST=100`→f=100, `FUNCNEST=20`→f=38, reset/unset→f=201).

Note (measured): bash ALSO crashes on infinite recursion when FUNCNEST is unset —
SIGSEGV (139). So FUNCNEST is bash's recursion guard; there is no graceful bash
default. huck's SIGABRT is the same failure class. We additionally add a defensive
backstop (below) — an intentional, documented improvement over bash.

## Goals

1. `FUNCNEST=N` (positive int) refuses a function call that would exceed depth N:
   emits `<prologue>NAME: maximum function nesting level exceeded (N)` to stderr
   and returns rc 1, without pushing a frame, running the body, or firing traps.
   Boundary and rc byte/semantically match bash 5.2.21.
2. Unset / `0` / negative / non-numeric `FUNCNEST` = unlimited (subject to the
   backstop).
3. **Backstop:** an internal hard cap `FUNCNEST_HARD_MAX = 2048` (below huck's
   ~2800 crash ceiling) applies whenever the effective FUNCNEST limit is absent or
   higher, so unbounded/very-deep recursion produces the same clean
   `maximum function nesting level exceeded (2048)` error instead of a SIGABRT.
4. `func` bash-test category PASSes; no currently-PASS category regresses.

## Non-goals / Out of scope

- Raising the native stack (no big-stack executor thread) — the depth cap is the
  chosen guard; a stack-size change is broader/riskier and not needed.
- A perfect no-crash guarantee: per-call stack usage varies, so a sufficiently
  stack-heavy function could still overflow below the cap. The cap is a pragmatic
  bound, not a guarantee (documented).
- FUNCNEST as an `integer`-attributed variable / its exact arithmetic coercion
  beyond "positive integer ⇒ limit, else unlimited".

## Design

### Component A — `Shell::funcnest_limit()` (shell_state.rs)

```rust
/// Parse $FUNCNEST. Some(n) for a positive integer limit; None (unlimited)
/// for unset / 0 / negative / non-numeric — matching bash.
pub fn funcnest_limit(&self) -> Option<usize> {
    self.lookup_var("FUNCNEST")
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|&n| n > 0)
        .map(|n| n as usize)
}
```

### Component B — guard in `call_function` (executor.rs:3652)

At the very top of `call_function`, BEFORE `shell.call_stack.push(frame)` (and
before any of the positional/getopts/local-scope setup), insert:

```rust
const FUNCNEST_HARD_MAX: usize = 2048;
// Effective nesting limit: the user's FUNCNEST, never above the safety backstop.
let limit = shell.funcnest_limit().map_or(FUNCNEST_HARD_MAX, |n| n.min(FUNCNEST_HARD_MAX));
let depth = shell
    .call_stack
    .iter()
    .filter(|f| matches!(f.kind, crate::shell_state::FrameKind::Function))
    .count();
if depth >= limit {
    let prefix = shell.error_prefix(Some(name));
    { let mut err = err_writer(err_sink, sink);
      e!(&mut *err, "{prefix}maximum function nesting level exceeded ({limit})"); }
    return ExecOutcome::Continue(1);
}
```

- `depth` = current Function-frame count; this call would be `depth + 1`, so
  `depth >= limit` refuses exactly the call that would exceed the limit (bash
  boundary: `FUNCNEST=3` ⇒ refuse at depth 3 → the 4th call).
- `error_prefix(Some(name))` yields `<source>: line N: NAME: ` (verified — gives
  bash's `./func4.sub: line 23: foo: ` in script mode). The full line is
  `<source>: line N: NAME: maximum function nesting level exceeded (N)`.
- Returning `Continue(1)` before any push means no frame, body, RETURN trap, or
  local scope is created; the caller's statement sees rc 1.
- For the func category (`FUNCNEST=100`/`20`, depths ≤ 201) the user limit governs
  and the message shows the FUNCNEST value; the backstop never fires there.

## Testing / Verification

- **Unit tests** (shell_state.rs): `funcnest_limit` returns `None` for unset/`"0"`/
  `"-3"`/`"abc"`, `Some(5)` for `"5"`, `Some(5)` for `" 5 "`.
- **Behavior tests** (executor.rs, via `exec_script`): `FUNCNEST=3` + a counting
  recursive function stops after depth 3, the refused call's rc is 1, and the
  error text matches `… maximum function nesting level exceeded (3)`; a deeper
  unset run (e.g. bounded to 50) does NOT error.
- **Diff harness** `funcnest_diff_check.sh` vs live bash 5.2.21, run as temp
  SCRIPT FILES (not `-c`, to get a matching `<source>: line N:` prologue):
  `FUNCNEST=3`/`=2`/`=1` bounded recursion (byte-exact error + rc + `$?`),
  `FUNCNEST=0`/unset bounded recursion (no error), and a func4.sub-style sequence.
- **Backstop test** (non-bash, huck-only): `f(){ f; }; f` with no FUNCNEST exits
  with the clean error and rc 1 (NOT 134/SIGABRT); and a moderately stack-heavy
  recursive function recursing past the cap also errors cleanly (tune
  `FUNCNEST_HARD_MAX` down if this still aborts — it must not).
- `cargo test --workspace` green (~3698).
- **func category PASS** (the flip — headline success criterion); `cprint`/`herestr`
  stay PASS.

## Risks

- **Cap too high for stack-heavy functions.** If the backstop test aborts at 2048,
  lower `FUNCNEST_HARD_MAX` (e.g. to 1024) — still far above any realistic
  recursion and above func4's 201. The plan's backstop test is the gate.
- **Prologue mismatch in `-c` mode.** bash labels `-c` source as `environment`;
  the harness uses script files to avoid this. The func category (script mode) is
  the real target and matches.
- **Depth-count cost.** `call_stack` is small in practice; an O(depth) filter per
  call is negligible. (A cached counter is possible but unnecessary — YAGNI.)

## Divergence-doc bookkeeping (on merge)

- `docs/bash-divergences.md`: REMOVE L-61 (func's last blocker resolved — func now
  PASSes). Add a low-severity `[deferred]`/`[intentional]` note: the
  `FUNCNEST_HARD_MAX` backstop emits `maximum function nesting level exceeded
  (2048)` on unbounded recursion where bash segfaults — an intentional robustness
  divergence; and FUNCNEST set above the cap is clamped.
- Update `docs/bash-test-suite-baseline.md` (func → PASS; Summary 8→9) and memory.
