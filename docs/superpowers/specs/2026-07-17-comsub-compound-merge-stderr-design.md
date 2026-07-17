# v310 — in-process compound `2>&1` inside `$()` merges stderr into the capture

**Issue:** [#176](https://github.com/jdstanhope/huck/issues/176) — a compound
command's stderr under `2>&1` inside a command substitution is lost from the
capture (leaked to a real fd) instead of merged into the captured stdout.

**Goal:** `$( { cmd; cmd >&2; } 2>&1 )` captures both streams in program order,
matching bash — for builtin, external, and mixed group bodies, and for subshells.

---

## Root cause (measured)

Command substitution captures stdout into an in-memory `Vec<u8>`
(`StdoutSink::Capture`, `executor.rs:362`); builtins write to it directly via
`LineDispatchWriter`, externals via a per-command real pipe drained into it
(`executor.rs:647`, `818`). Crucially there is **no single real fd** that *is*
"the capture."

`execute_capturing` hard-codes `err_sink = StderrSink::Terminal` (`executor.rs:365`,
with a comment saying stderr capture "isn't plumbed here yet"). A compound
command's trailing `2>&1` (`Command::Redirected`) is applied by
`with_redirect_scope` (`executor.rs:1233`) **only as a real `dup2(2→1)`** — and
because fd 1 in a capture context is the process's real fd 1 (the terminal), that
`dup2` points fd 2 at the terminal, not the capture. So the group's inner
`cmd >&2` writes to the real fd and escapes the capture entirely. `with_redirect_scope`
never touches `err_sink`, so the software capture model never learns about the merge.

The simple-command path already handles this: `run_builtin_with_redirects`
computes `route_err_to_out` (`executor.rs:1373`) and, in a capture, routes the
command's stderr into the stdout sink; the external-spawn path handles a `Merged`
err_sink by pointing the child's fd 2 at the capture pipe (`executor.rs:672`).
The **compound** path simply never got the equivalent.

### Measured behavior (huck vs bash 5.2.21)

`printf "<%s>"` shows the captured `$x`; fd 2 is sent to `/dev/null`.

| case | bash `$x` | huck `$x` |
|---|---|---|
| `$( echo out; echo er >&2 )` (no merge) | `out` | `out` ✓ (er→terminal, correct) |
| `$( echo hi 2>&1 )` (simple `2>&1`) | `hi` | `hi` ✓ |
| **`$( { echo out; echo er >&2; } 2>&1 )`** | `out\ner` | `er` then `<out>` ✗ (er leaked to real fd 1) |
| **`$( { /bin/echo out; /bin/echo er >&2; } 2>&1 )`** (external) | `out\ner` | `er`,`<out>` ✗ |
| **`$( ( echo out; echo er >&2 ) 2>&1 )`** (subshell) | `out\ner` | `er`,`<out>` ✗ |
| **`$( { echo out; /bin/echo er >&2; } 2>&1 )`** (mixed) | `out\ner` | `er`,`<out>` ✗ |
| `{ echo out; echo er >&2; } 2>&1` (terminal, no comsub) | both→term | both→term ✓ |

The four ✗ rows are the bug. The function-context symptom in the issue
(`f(){ got=$(…); }`) is the **same** bug: the engine path is byte-identical inside
a function (`call_function` passes sinks through unchanged); the issue's "stderr
dropped in a function" was a testing artifact of what real fd 2 was connected to.

## Design (Approach A — software `Merged` routing in the compound path)

Give `with_redirect_scope` the same `2>&1`-detection the simple-command path
already uses, via a shared predicate.

### Component 1 — shared predicate

Extract the redirect-side condition of `route_err_to_out` into a named function:

```rust
/// True when `redirs` makes fd 2 follow fd 1 (`2>&1`) with fd 1's FINAL
/// destination still the software sink — i.e. stderr should be merged into the
/// (captured) stdout in memory, not sent to a real fd. Shared by the
/// simple-command path (`run_builtin_with_redirects`) and the compound-redirect
/// path (`with_redirect_scope`).
fn redirs_merge_err_into_out(redirs: &[Redirection], shell: &mut Shell) -> bool {
    let (final_1, final_2) = final_dests_for_1_2(redirs, shell);
    matches!(final_1, RedirectDest::Sink) && matches!(final_2, RedirectDest::Follows(1))
}
```

At `executor.rs:1373` the simple path becomes:

```rust
let route_err_to_out =
    matches!(sink, StdoutSink::Capture(_)) && redirs_merge_err_into_out(redirs, shell);
```

This is a pure refactor of the simple path — same predicate, same behavior.
(`final_dests_for_1_2` takes `&mut Shell` because `Dup` source words resolve via
`resolve_fd_target`; the helper does too.)

### Component 2 — the compound path

In `with_redirect_scope`, after `apply_redirects` runs (the real `dup2` stays
exactly as today — see "Why keep the real dup2"), when stdout is being captured
and the redirects merge fd 2 into fd 1, hand the inner body a `Merged` err_sink:

```rust
let mut merged = StderrSink::Merged;
let merge_err = matches!(*sink, StdoutSink::Capture(_)) && redirs_merge_err_into_out(redirs, shell);
let inner_err_sink: &mut StderrSink = if merge_err { &mut merged } else { err_sink };
let outcome = run_inner(shell, inner_sink, inner_err_sink);
```

Then each inner command routes stderr into the capture by existing, tested
machinery: builtins via `err_writer(Merged, Capture)` → `LineDispatchWriter` →
buf; externals via `stderr_fd = stdout_fd` (the capture pipe, `executor.rs:672`).
Everything lands in the one capture in program order.

**Why keep the real `dup2`:** the simple-command path also runs
`scope.apply_redirects` unconditionally alongside `route_err_to_out`, and the real
`dup2(2→1)` is inert in both consumers — builtins use the software err sink and
ignore real fd 2; external children explicitly set their fd 2 to the capture pipe,
overriding the inherited (dup'd) fd 2. Keeping it mirrors the proven path and
avoids fiddly "skip one redirect mid-list" logic. The plan's TDD gate verifies
empirically that no double-routing occurs.

**Terminal case is untouched:** `merge_err` requires `StdoutSink::Capture`, so a
plain `{ } 2>&1` at the terminal keeps its current behavior (err_sink unchanged,
real `dup2` sends both to the terminal). No regression.

## Scope boundary — `2>&1 >file` ordering is NOT fixed here

`$( { echo out; echo er >&2; } 2>&1 >file )` — bash routes **er → capture**
(fd 1's value at the `2>&1`) and **out → file**. huck currently leaks er and
captures nothing. This design does **not** fix it, and correctly so: the shared
predicate uses `final_dests_for_1_2`, a state-overwrite walk that reports fd 1's
*final* dest (the file) — so `redirs_merge_err_into_out` returns false and the
merge does not fire (no over-fire, no regression — it stays exactly as broken as
today). Handling it needs order-aware "was fd 1 the sink at the moment of the
`2>&1`" tracking that the software model does not have, and the simple-command
path has the identical limitation (it relies on ordered real-fd dup replay, which
works only when a real capture pipe exists — i.e. externals, not builtin groups).
Fixing it is a distinct, larger change. **File a follow-on `divergence` issue** for
the `2>&1 >file` (and `>file 2>&1`) compound-in-capture ordering cases; reference
it in the spec commit. This iteration fixes the plain `2>&1` merge, which is the
reported bug and the common case.

## Testing

New `tests/scripts/comsub_merge_stderr_diff_check.sh`, byte-diffing huck vs bash,
capturing `$x` via `printf "<%s>"` with fd 2 routed to `/dev/null` so a leak is
visible as a missing/misordered capture. It asserts **content and order** across:

- **The fix (must flip red→green):** builtin group, external group, subshell
  `( )`, and mixed builtin+external group — each `{ …; } 2>&1` / `( … ) 2>&1`
  inside `$()`, asserting `$x == "out\ner"`.
- **Controls (must stay green):** the no-merge case (`$( echo out; echo er >&2 )`
  → `out`, er to terminal); simple-command `2>&1` in a comsub (`$( echo hi 2>&1 )`);
  `$( { …; } >file 2>&1 )` (both to file, capture empty); the terminal `{ } 2>&1`
  with no comsub.
- **Documented-gap guard:** a `2>&1 >file` case asserting huck's *current* behavior
  is unchanged (not silently "fixed" or further broken), with a comment pointing at
  the follow-on issue.

Plus a `run_builtin_with_redirects`-level check that the refactor of
`route_err_to_out` to the shared helper leaves the existing simple-command
`2>&1`-in-capture behavior byte-identical (the existing engine/builtin diff-check
harnesses already cover this — confirm they stay green).

## Rejected alternatives

- **Real OS pipe for the whole comsub capture (bash's model).** Would make the
  group's real `dup2` work like bash and fix every compound-redirect-in-capture
  case, including `2>&1 >file`. Rejected as a disproportionate, high-risk rewrite
  of a hot path (all builtin capture routing, ordering, buffering, procsub
  draining, the v308 FdWriter work) for a sev:medium bug.
- **Fork the group as a subshell in the capture case** (like the #144
  pipeline-stage fix). Rejected: a brace group `{ …; }` must NOT fork — its
  assignments and `cd` must persist into the enclosing shell. Forking would break
  brace-group semantics.
