# v189: complete the standard signal set — Design

**Status:** approved 2026-06-19
**Iteration:** v189
**Origin:** Found by the v188 llvm-cov coverage sweep. `signal_number_to_name`
was untested; exercising it showed huck's signal tables cover only the
job-control signals, so **15 of 31 standard signals are missing**: `kill -l 6`
errors (`invalid signal specification`) instead of printing `ABRT`. The gap has
three faces, all from the same incomplete table:

- **`kill -l <number>`** (number→name): missing ILL(4) TRAP(5) ABRT(6) BUS(7)
  FPE(8) SEGV(11) STKFLT(16) URG(23) XCPU(24) XFSZ(25) VTALRM(26) PROF(27) IO(29)
  PWR(30) SYS(31).
- **Sending** (`kill -ABRT`, `kill -s SEGV`, `kill -11`): `invalid signal`.
- **Trapping** (`trap 'h' SEGV`): rejected.

## bash contract (verified on Linux/glibc)

- `kill -l 6` → `ABRT`; `kill -l 11` → `SEGV` (bare name, no `SIG` prefix).
- `kill -l ABRT` / `kill -l SIGABRT` → `6` (name→number; `SIG` prefix optional).
- `kill -l` (no args) full listing: `N) SIGNAME` form, **`SIG` prefix**,
  **tab-separated**, **5 columns**, number right-aligned to width 2:
  `` 1) SIGHUP\t 2) SIGINT\t 3) SIGQUIT\t 4) SIGILL\t 5) SIGTRAP``.
- `kill -ABRT <pid>` / `kill -s ABRT <pid>` / `kill -6 <pid>` send SIGABRT.
- `trap 'echo hit' SEGV; kill -SEGV $$` fires the trap (KILL/STOP remain
  un-trappable).

## Scope (decided in brainstorming)

- **Standard named signals only** (1–31 on Linux). **Real-time signals
  (SIGRTMIN..SIGRTMAX) are deferred** — rarely used in shells, and the trap
  pending bitmask is an `AtomicU32` that holds exactly bits 1–31, so RT signals
  (34–64) would need a wider mask. (This is independent confirmation RT is the
  right deferral.)
- The full `kill -l` listing **adopts bash's `SIG`-prefix/tab/5-column format**.
  It still cannot be byte-identical to bash for the *whole* output (bash appends
  the RT-signal tail 34–64), but the standard-signal rows (1–30) match exactly.

## Root cause

`src/traps.rs` hand-maintains two overlapping tables covering only job-control
signals:
- `TRAPPABLE` (14 entries) — used by `parse_trap_signal`, `signal_number_to_name`
  (`builtins.rs:5036`, via `name_table()`), `ignored_at_startup_set`, and the
  `trap -l` printer `print_signal_table` (`builtins.rs:5017`).
- `KILLABLE` (16 entries = TRAPPABLE + KILL + STOP) — used via
  `killable_signals()` by `handle_kill_l` (number↔name, `builtins.rs`),
  `signal_by_name` (sending), `kill_with_n_flag`, and `print_killable_table`
  (the `kill -l` full listing).

## Design

### 1. One source of truth (`src/traps.rs`)

Replace the two hand-maintained `const` tables with a single builder over
`libc::SIG*` constants — correct per-platform numbers — listing every standard
signal, with `#[cfg]`-guarded platform-specific entries so it builds on macOS
(portability constraint). Sketch:

```rust
/// Every standard (non-real-time) signal this platform names, as
/// (name without SIG prefix, libc number). Built from libc constants so the
/// numbers are correct per platform; platform-specific signals are cfg-gated.
fn standard_signals() -> Vec<(&'static str, i32)> {
    let mut v = vec![
        ("HUP", libc::SIGHUP), ("INT", libc::SIGINT), ("QUIT", libc::SIGQUIT),
        ("ILL", libc::SIGILL), ("TRAP", libc::SIGTRAP), ("ABRT", libc::SIGABRT),
        ("BUS", libc::SIGBUS), ("FPE", libc::SIGFPE), ("KILL", libc::SIGKILL),
        ("USR1", libc::SIGUSR1), ("SEGV", libc::SIGSEGV), ("USR2", libc::SIGUSR2),
        ("PIPE", libc::SIGPIPE), ("ALRM", libc::SIGALRM), ("TERM", libc::SIGTERM),
        ("CHLD", libc::SIGCHLD), ("CONT", libc::SIGCONT), ("STOP", libc::SIGSTOP),
        ("TSTP", libc::SIGTSTP), ("TTIN", libc::SIGTTIN), ("TTOU", libc::SIGTTOU),
        ("URG", libc::SIGURG), ("XCPU", libc::SIGXCPU), ("XFSZ", libc::SIGXFSZ),
        ("VTALRM", libc::SIGVTALRM), ("PROF", libc::SIGPROF), ("WINCH", libc::SIGWINCH),
        ("IO", libc::SIGIO), ("SYS", libc::SIGSYS),
    ];
    #[cfg(target_os = "linux")]
    {
        v.push(("STKFLT", libc::SIGSTKFLT));
        v.push(("PWR", libc::SIGPWR));
    }
    #[cfg(any(target_os = "macos", target_os = "freebsd"))]
    {
        v.push(("EMT", libc::SIGEMT));
        v.push(("INFO", libc::SIGINFO));
    }
    v
}
```

Derive the existing API surfaces from this builder so callers are unchanged in
spirit (they may now read a `Vec`/`OnceLock<Vec<…>>` instead of a `&'static`
slice — a `OnceLock<Vec<(&'static str, i32)>>` keeps the `&'static` lifetime):

- **`killable_signals()`** → the full table (all standard signals). Used by
  `kill` send + `kill -l` number↔name.
- **`name_table()`** (trappable view) → the full table **minus KILL and STOP**.
  Used by `trap` + `signal_number_to_name` + `ignored_at_startup`.

The bitmask, `parse_trap_signal`, `ignored_at_startup_set`, and `install`/`reset`
already work for any signal number ≤ 31 — no change beyond the table they read.
`parse_trap_signal`'s explicit KILL/STOP rejection stays.

### 2. `kill -l` listing format (`print_killable_table`, `src/builtins.rs`)

Reformat to bash's listing: signals sorted by number, `SIG`-prefixed names,
**5 per row**, tab-separated, number right-aligned width 2, e.g.:

```rust
fn print_killable_table(out: &mut dyn Write) {
    let mut sigs: Vec<_> = crate::traps::killable_signals().to_vec();
    sigs.sort_by_key(|(_, n)| *n);
    for (i, (name, num)) in sigs.iter().enumerate() {
        let sep = if i % 5 == 4 || i == sigs.len() - 1 { "\n" } else { "\t" };
        let _ = write!(out, "{num:>2}) SIG{name}{sep}");
    }
}
```

(The single-name lookups `kill -l 6`→`ABRT` and `kill -l ABRT`→`6` keep their
current bare output — only the no-arg full listing is reformatted.) `trap -l`'s
`print_signal_table` adopts the same bash format for consistency, reading the
trappable view.

### 3. Sending & trapping

No code change beyond the expanded tables: `signal_by_name`, `kill_with_n_flag`,
and `parse_trap_signal` all read the now-complete tables, so `kill -ABRT`,
`kill -s SEGV`, `kill -11`, and `trap '…' SEGV` all work. Catching a *synchronous
fault* (a real SIGSEGV) runs the deferred trap at the next safe point exactly as
bash's model allows registration; we add registration parity, not fault-recovery
guarantees.

## Verification

- **New bash-diff harness** `tests/scripts/kill_signals_diff_check.sh`
  (byte-identical stdout+exit):
  - `for n in $(seq 1 31); do kill -l $n; done` (number→name, the core fix).
  - `kill -l ABRT SIGSEGV bus 11 KILL` (mixed name/number/SIG-prefix/case).
  - `kill -l | head -6` (full listing: the first 30 signals = 6 complete rows,
    byte-identical to bash; the RT tail beyond 31 is excluded by `head`).
  - `kill -l 137` (128+9 → `KILL`, the exit-status form already handled).
- **Integration test** (`tests/` — new or extend `kill_l_integration`):
  `trap 'echo HIT' SEGV; kill -SEGV $$; ...` prints `HIT` (trapping a new
  signal); `kill -s ABRT` to a backgrounded `sleep` terminates it; round-trip
  `kill -l ABRT` → `6`, `kill -l 6` → `ABRT`.
- **Existing tests**: UP-FRONT grep `tests/` + `src/` for `TRAPPABLE`/`KILLABLE`/
  `killable_signals`/`name_table`/`signal_by_name` assertions (e.g.
  `signal_names_are_sig_prefixed_and_exclude_pseudo`, `print_signal_table` line
  format, the `kill -l` HUP/KILL/TERM column tests at `builtins.rs:~9050`). Any
  test asserting the OLD 14/16-entry set or the OLD 4-column bare format must be
  UPDATED to the new complete set / bash format. Verify each against bash.
- **macOS build sanity**: the cfg-gated table must compile on macOS
  (`libc::SIGSTKFLT`/`SIGPWR` are Linux-only, `SIGINFO`/`SIGEMT` macOS/BSD-only).
  Confirm via inspection + `cargo check` (and `cargo check --target
  x86_64-apple-darwin` if the target is installed).
- Full `cargo test` (0 failures); all `tests/scripts/*_diff_check.sh` green;
  clippy clean.

## Docs / close-out

Coverage-found; no tracked `M-*`/`L-*` divergence. No `bash-divergences.md`
change beyond optionally adding a `[deferred]` note for the two follow-ons.
Record v189 in `project_huck_iterations.md` + `MEMORY.md`. **Deferred follow-ons
to log:** (a) real-time signals (RTMIN..RTMAX) — needs a wider-than-u32 pending
mask; (b) the full `kill -l` RT-signal tail (34–64) so the whole listing matches
bash, contingent on (a).

## Scope boundary

In scope: the unified standard-signal table; `kill -l` number↔name + listing
format; sending and trapping the new standard signals; the new harness +
integration tests; updating tests that encoded the old set/format. **Not** in
scope: real-time signals; `kill -l` RT tail; any change to the trap dispatch /
pending-bitmask mechanism (the `AtomicU32` already covers 1–31); fault-recovery
semantics for synchronous-fault traps.
