# v311 — a `!`-negated pipeline suppresses errexit/ERR for its whole body

**Issue:** [#1](https://github.com/jdstanhope/huck/issues/1) — `! eval CMD` /
`! { …; }` don't suppress `set -e` for a failure *inside* the body; huck exits
where bash negates and continues. First member of the error-fatality funnel
umbrella [#198](https://github.com/jdstanhope/huck/issues/198).

**Goal:** `set -e; ! eval false; echo after` prints `after` (rc 0), matching
bash — for every inner-execution construct under `!`, without suppressing a real
`exit`.

---

## Root cause (measured)

`run_pipeline` (`executor.rs:2778`) runs the pipeline body, then negates **only a
`Continue` outcome** (`2794`); an `Exit` propagates unchanged. When a
`!`-negated command runs an *inner execution* — `eval` via
`process_line_in_sinks`, a brace group `{ …; }` via `execute_sequence_body` —
the inner failing command runs its own errexit check (`executor.rs:173`, `447`,
`490`, each gated on `err_suppressed_depth == 0`), sees the counter at 0, and
returns `ExecOutcome::Exit`. That `Exit` bypasses the outer `!` negation and
exits the shell.

A subshell `( … )` does **not** exhibit this: it is an execution boundary that
contains the errexit-triggered exit as its own status, which the outer `!` then
negates. So the issue's "subshell is buggy" framing is **stale** — `! ( false )`
already matches bash. The buggy constructs are the non-boundary inner executions:
`! eval …` and `! { …; }` (a brace-group case the issue did not list).

The existing gate at `executor.rs:447`/`490` already exempts the *outer* negated
pipeline from errexit (`!is_negated_pipeline(...)`), which is why `! false` and
`! builtin false` work. What is missing is suppressing errexit/ERR *within* the
negated body.

## Measured bash 5.2.21 behavior (the target — every row verified)

`set -e` is set in all rows.

| input | bash |
|---|---|
| `! eval false; echo after` | `after`, rc 0 |
| `! eval '(exit 5)'; echo after` | `after`, rc 0 |
| `! { false; }; echo after` | `after`, rc 0 |
| `! { false; true; }; echo after` | `after`, rc 0 |
| `! { true; false; }; echo after` | `after`, rc 0 |
| `! eval 'false; true'; echo after` | `after`, rc 0 |
| `! ( false ); echo after` | `after`, rc 0 (already correct in huck) |
| `! false` / `! builtin false` | `after`, rc 0 (already correct) |
| **`! eval 'exit 5'; echo after`** | **rc 5 — a real `exit` is NOT suppressed** |
| `eval false; echo after` (no `!`) | rc 1 (errexit fires normally) |
| `trap 'echo ERR' ERR; ! eval false; echo after` | `after` only — **ERR NOT fired** |
| `trap 'echo ERR' ERR; eval false; echo after` (no `!`) | `ERR`, rc 1 |

Two invariants the fix must honor, both confirmed above:
1. **A real `exit` inside the negated body still exits** (`! eval 'exit 5'` → 5).
2. **The ERR trap is also suppressed** inside the negated body (row 11 vs 12).

## Design

In `run_pipeline`, wrap the body execution in the existing errexit-suppression
counter when the pipeline is negated — the same mechanism `run_while_inner`
(`executor.rs:1685`) and the `if`/`case` condition paths already use to exempt a
tested condition:

```rust
fn run_pipeline(pipeline, shell, sink, err_sink) -> ExecOutcome {
    // A `!`-negated pipeline's failure is EXPECTED (it is being tested), so
    // `set -e`/ERR must not fire for anything the body runs — including inner
    // executions like `eval` and brace groups that are not their own boundary
    // (#1). Bump the shared suppression counter for the body only; the outer
    // and-or gate already exempts the negated pipeline itself.
    let outcome = if pipeline.negate {
        shell.err_suppressed_depth += 1;
        let o = run_pipeline_body(pipeline, shell, sink, err_sink); // the existing len==1 / multi-stage dispatch
        shell.err_suppressed_depth -= 1;
        o
    } else {
        run_pipeline_body(pipeline, shell, sink, err_sink)
    };
    if pipeline.negate {
        if let ExecOutcome::Continue(s) = outcome {
            return ExecOutcome::Continue(if s == 0 { 1 } else { 0 });
        }
    }
    outcome
}
```

("`run_pipeline_body`" is just the existing `if commands.len()==1 { run_command }
else { run_multi_stage }` dispatch — extract it or inline it; the point is the
inc/dec brackets exactly that call with no early return between them, so the
counter stays balanced.)

**Why this is correct for every invariant:**
- Inner errexit checks (`:173`/`:447`/`:490`) now see `err_suppressed_depth > 0`
  and return `Continue`, so the inner `false` yields `Continue(1)`, the body
  returns `Continue`, and `!` negates it → `Continue(0)`. Fixes `eval` **and**
  brace groups uniformly (any inner-execution construct consults the same
  counter).
- A real `exit` builtin returns `ExecOutcome::Exit` **directly**, not through the
  errexit gate — the counter never touches it, and `run_pipeline`'s negation only
  rewrites `Continue`. So `! eval 'exit 5'` still exits 5.
- The ERR trap fires from `fire_err_trap` at the same gated sites, so it is
  suppressed by the same bump — matching bash row 11.
- The counter is a depth (u32), so nested negation / a negated pipeline inside a
  while-condition compose correctly.

**Scope:** only `run_pipeline`. No change to `eval`, subshell, or brace-group
code — the fix is that they already consult `err_suppressed_depth`, and we now
raise it around the negated body. The already-correct cases (`! false`,
`! ( false )`, `! builtin false`) are unaffected (the bump is harmless where no
inner errexit check would have fired).

## Testing

New `tests/scripts/negated_errexit_diff_check.sh`, byte-diffing huck vs bash
(stdout+stderr+rc) over every row of the table above:
- **Fix (red→green):** `! eval false`, `! eval '(exit 5)'`, `! { false; }`,
  `! { false; true; }`, `! { true; false; }`, `! eval 'false; true'`.
- **Controls (stay green):** `! false`, `! ( false )`, `! builtin false`,
  `eval false` (no `!`, must still exit rc 1).
- **Invariant guards:** `! eval 'exit 5'` (must exit rc 5 — real exit not
  suppressed) and the two ERR-trap rows (suppressed under `!`, fired without).

Plus: the existing `set -e` behavior must not regress — run the engine lib tests
and the existing errexit/`set -e` diff-check harness(es); confirm green.

## Rejected alternatives

- **Thread a suppression flag through `eval`'s `process_line_in_sinks` and each
  subshell/brace-group body** (the issue's suggestion). More invasive and
  *incomplete* — it patches two constructs when many inner-execution paths exist;
  Approach A catches them all at one point via the shared counter they already
  consult.
- **Convert an inner `Exit` back to `Continue` when negated.** Unworkable: at the
  pipeline level an errexit-`Exit` and a real-`exit`-`Exit` are the same
  `ExecOutcome::Exit`, so this would wrongly swallow `! eval 'exit 5'`.
