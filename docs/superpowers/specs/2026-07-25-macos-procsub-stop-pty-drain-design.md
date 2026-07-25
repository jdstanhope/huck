# v336 — Re-enable `procsub_stop_pty` on macOS (drain the PTY like a real terminal)

Issue: [#97](https://github.com/jdstanhope/huck/issues/97) — "macOS: Ctrl-Z on a
job containing a process substitution hangs the shell" (bug, divergence,
sev:high).

## TL;DR

The two `tests/procsub_stop_pty.rs` regression tests are skipped on macOS behind
`skip_known_macos_hang()`. The issue speculated a kqueue / SIGCHLD / job-control
defect. **Root-cause investigation disproves that.** huck's job control is
correct on macOS. The "hang" is a `rustyline` `tcsetattr(TCSADRAIN)` drain-block
that only deadlocks when the PTY **consumer stops reading** — which the test
harness (`expectrl`) does during its `thread::sleep` windows — and only on macOS,
because of how a BSD/XNU pty implements `TCSADRAIN`.

The fix is test-only: **continuously drain the PTY master** (a background reader
thread, exactly what a real terminal emulator does), then re-enable the two tests
on macOS. No change to huck's runtime behavior.

## Evidence (why this is not a job-control defect)

Reproduced on macOS 26.5 (arm64) against the debug `huck` binary:

1. **huck's stop handling is correct.** With the job wedged, manually
   `kill -TSTP -<pgid>` on the job's process group makes huck's foreground
   `waitpid(WUNTRACED)` return immediately, print `[1]+ Stopped`, and restore
   the prompt — the regression test then passes in ~2.7 s.
2. **The terminal is set up identically to bash.** Dumping the full slave
   `termios` from the foreground child under huck vs bash is **byte-identical**
   (`ISIG=1`, `VSUSP=0x1a`, `ICANON=1`, …) and the child is the terminal's
   foreground process group (`tcgetpgrp == getpgrp`) in both.
3. **The kernel delivers SIGTSTP on the master write regardless of readers.** A
   shell-free control (a bare foreground child that never reads stdin, even with
   a full/blocked output buffer) receives SIGTSTP the instant `0x1a` is written
   to the master. So signal generation does not depend on draining.
4. **The hung huck is stuck in `ioctl`, not `waitpid`.** All-thread `sample`s of
   the wedged process show it blocked in `ioctl` = `tcsetattr(TCSADRAIN)`
   (`TIOCSETAW`) inside `rustyline`'s raw-mode transition
   (`disable_raw_mode`/`enable_raw_mode`, `rustyline .../tty/unix.rs:1609,1646`
   both hardcode `SetArg::TCSADRAIN`). **Draining the PTY master unblocks it
   every time**, after which Ctrl-Z stops the job normally.
5. **bash survives the same stalled-reader harness; so does Linux.** GNU readline
   does not use `TCSADRAIN` for its mode flips, so bash never drain-blocks. On
   Linux the identical `rustyline` `TCSADRAIN` returns without waiting for the
   master reader (Linux pty output is considered drained once queued to the
   master buffer), which is why the tests already pass on Linux.

### Why it looked like "the job never stops"

When the harness stalls its reader, huck blocks in `tcsetattr(TCSADRAIN)` around
a prompt raw-mode transition — sometimes *before* it even spawns the job (the job
never starts), sometimes at the next prompt after the job is already running (so
`ps` shows the job still `S+` and `sample` shows the main thread parked in
`__wait4`). Both manifestations share the one root cause; both clear the instant
the master is drained.

## Real-user impact: none

A real terminal emulator (Terminal.app, iTerm2, tmux, an SSH client) drains the
pty master continuously, so `tcsetattr(TCSADRAIN)` returns promptly and huck
never wedges. The deadlock requires a consumer that deliberately stops reading
mid-session — a property of the test harness, not of interactive use. With a
continuously-draining consumer, both original scenarios pass **5/5** on macOS.

This is therefore an actionable *test-harness* divergence, not a runtime
divergence: closing #97 means making the regression faithfully emulate a real
terminal.

## Fix

Rewrite `tests/procsub_stop_pty.rs` so the PTY master is drained continuously by
a dedicated reader thread for the lifetime of each test (a real-terminal model),
instead of relying on `expectrl`'s intermittent `expect`-time reads with
un-drained `thread::sleep` gaps. Then delete `skip_known_macos_hang()` and its
call sites so both tests run on macOS and Linux.

Constraints / non-negotiables preserved:

- The tests must still **fail** if huck reintroduces the original Linux deadlock
  (a blocking `waitpid` on a live process-substitution child): that hang is
  independent of terminal draining, so a drain-based harness still catches it.
- Keep the existing "skip (pass) when no PTY can be allocated" behavior for
  sandboxed CI.
- Keep `--norc` hermeticity (#239).
- No change to any `crates/**` runtime code.

## Acceptance

- `cargo test --test procsub_stop_pty` passes on macOS (both tests run, not
  skipped) and remains green on Linux/CI.
- `sleep 30 | tee >(cat)` + Ctrl-Z and `sleep 30 > >(cat)` + Ctrl-Z both return
  to the prompt and run the next command, verified through a continuously-drained
  PTY.
- `skip_known_macos_hang()` is removed.
- Full `cargo test --workspace --locked` and the `run_diff_checks.sh` bash-diff
  sweep stay green.

## Out of scope (possible follow-up)

Hardening huck so it cannot drain-block even under a pathologically stalled
terminal consumer would require bypassing/patching `rustyline`'s hardcoded
`TCSADRAIN` (a rustyline-independent raw-mode path). Real terminals never trigger
the condition, so this is deferred; open a separate `enhancement` issue if we
decide the extra robustness is worth the blast radius.
