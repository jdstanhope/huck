# v194: `trap … 0` (numeric signal 0 ≡ EXIT) — Design

**Status:** approved 2026-06-19
**Iteration:** v194
**Origin:** The v193 **runtime sweep** (`RUN_HUCK_ERROR` bucket). Real scripts use
`trap '…' 0` for cleanup (e.g.
`linux-headers/.../rcutorture/bin/kvm-test-1-run-batch.sh`). bash treats the
numeric signal **0** as the `EXIT` pseudo-signal; huck rejects it:
`trap: 0: invalid signal specification`.

## bash contract (verified)

`0` is an alias for `EXIT` everywhere `trap` accepts a signal spec:

- `trap 'cmd' 0` registers an EXIT trap (`cmd` runs when the shell exits) —
  identical to `trap 'cmd' EXIT`.
- `trap -p 0` prints `trap -- 'cmd' EXIT` (normalized to the `EXIT` name).
- `trap '' 0` ignores EXIT; `trap - 0` resets EXIT.
- `trap 'cmd' 0 2` registers EXIT *and* signal 2.

## Root cause

`parse_trap_signal` (`src/traps.rs`) handles the `EXIT` *name* (→
`TrapSignal::Exit`) but its numeric branch never special-cases `0`: it checks
KILL/STOP, then the trappable `name_table()` (which has no entry for number 0),
then errors `{name}: invalid signal specification`.

## Design

One guard at the top of the numeric branch, before the KILL/STOP/table checks:

```rust
    if let Ok(n) = name.parse::<i32>() {
        if n == 0 {
            return Ok(TrapSignal::Exit);   // 0 ≡ EXIT (bash)
        }
        // … existing KILL/STOP/name_table checks …
    }
```

Mapping numeric `0` to the same `TrapSignal::Exit` the `EXIT` name produces makes
every form fall out for free: registration runs the trap on exit, `trap -p 0`
prints `EXIT` (the existing `-p` formatter renders `TrapSignal::Exit` as `EXIT`),
`trap '' 0` / `trap - 0` ignore/reset EXIT, and `trap 'cmd' 0 2` adds EXIT plus
signal 2. No other code changes — the executor already fires `TrapSignal::Exit`
on shell exit.

## Verification

- **New bash-diff harness** `tests/scripts/trap_zero_diff_check.sh` (byte-identical
  stdout+exit): `trap 'echo EX' 0; echo body` (→ `body`⏎`EX`); `trap 'echo EX' 0 2;
  echo body` (EXIT + a trappable signal both register; only EXIT fires here);
  `trap '' 0; echo body` (ignored — no EX on exit); `trap 'echo EX' 0; trap - 0;
  echo body` (reset — no EX); `trap 'echo A' 0; trap -p 0` (→ `trap -- 'echo A'
  EXIT`, the normalized name). All run via `bash -c`/`huck -c`.
- **Unit test** (`src/traps.rs` `mod tests`, near the `parse_trap_signal` tests):
  `parse_trap_signal("0") == Ok(TrapSignal::Exit)`; and `parse_trap_signal("EXIT")`
  stays `Ok(TrapSignal::Exit)` (regression guard — same result both ways).
- **Up-front grep** `tests/` + `src/` for any test asserting `trap … 0` errors
  (none expected — it was a plain rejection); update if found.
- Full `cargo test` (0 failures); all harnesses + clippy green.

## Docs / close-out

Runtime-sweep-found; no tracked `M-*`/`L-*` covered it (the v189 signal work
added the standard signal *set* but never mapped numeric `0`→EXIT — a separate
miss). No `bash-divergences.md` change. Record v194 in `project_huck_iterations.md`
+ `MEMORY.md` (first bug fixed from the new runtime sweep; note the runtime-sweep
backlog — `${!1}` on unset positional, `$0`-in-function — remains).

## Scope boundary

In scope: the `n == 0 → TrapSignal::Exit` guard in `parse_trap_signal`; the
harness + unit test. **Not** in scope: the other runtime-sweep bugs (`${!1}`,
`$0`-in-function); `kill 0` (a process-group target, NOT a trap spec — unrelated);
real-time signals; any other `trap`/signal behavior.
