# v336 — Re-enable `procsub_stop_pty` on macOS Implementation Plan

Issue: [#97](https://github.com/jdstanhope/huck/issues/97).
Spec: `docs/superpowers/specs/2026-07-25-macos-procsub-stop-pty-drain-design.md`.

Test-only change. Root cause (see spec): `rustyline`'s
`tcsetattr(TCSADRAIN)` drain-blocks when the PTY consumer stalls; the harness
stalls its reader in the `thread::sleep` gaps; macOS pty `TCSADRAIN` waits for
the master reader, so huck wedges. Fix: drain the master continuously.

## Design of the new harness

Replace the `expectrl`-based body with a self-contained raw-PTY driver that
models a real terminal:

- Fork+exec `huck --norc` on a PTY using `nix` (`openpty` + `fork` + `login_tty`
  via `setsid`/`TIOCSCTTY`), or reuse `ptyprocess` (already a transitive dep via
  `expectrl`) for the spawn and expose the master fd. Prefer `ptyprocess`
  directly to keep the controlling-tty setup correct and portable — it already
  does `make_controlling_tty` + `set_echo(false)`.
- Spawn ONE reader thread that loops `read(master)` into a `Arc<Mutex<Vec<u8>>>`
  shared buffer until EOF/stop. This is the "real terminal" — the master is
  always being drained, so `tcsetattr(TCSADRAIN)` in the child never blocks.
- Helper `wait_for(buf, needle, timeout) -> bool` polls the shared buffer for a
  substring with a deadline (turns a regression-hang into a failed assertion).
- `send(master, bytes)` writes to the master fd.
- Skip (return, pass) if the PTY cannot be allocated (sandboxed CI) — mirror the
  current behavior.

Sequence per test (unchanged semantics, now drained):
1. `wait_for("READY_42")` after sending `echo READY_$((6*7))\r` to confirm the
   interactive prompt is alive.
2. Send the foreground job (`sleep 30 | tee >(cat >/dev/null)` /
   `sleep 30 > >(cat >/dev/null)`).
3. `wait_for` a proof the job is set up, or a short bounded settle, then send
   `\x1a` (Ctrl-Z).
4. Send `echo AFTER_$((1+1))` / `echo BACK_$((2+2))` and assert
   `wait_for("AFTER_2"/"BACK_4")` within a timeout — i.e. the prompt came back
   and the next command ran.
5. Best-effort `kill -9 %1` cleanup, stop the reader, reap.

## Tasks

### Task 1 — Rewrite `tests/procsub_stop_pty.rs`
- Remove `skip_known_macos_hang()` and both call sites.
- Update the module doc comment: replace the "skipped on macOS / known
  job-control hang" paragraph with a one-paragraph note that the harness drains
  the master continuously (a real terminal) because `rustyline`'s
  `tcsetattr(TCSADRAIN)` deadlocks against a stalled reader on macOS; link #97.
- Implement the raw-PTY + reader-thread harness above. Keep the no-PTY skip.
- Keep both test names and their asserts/messages meaningful.
- If `nix`/`ptyprocess` needs to be an explicit `dev-dependency`, add it to the
  root `Cargo.toml` `[dev-dependencies]` (expectrl already pulls `ptyprocess`;
  make the dep explicit if we import it directly). Keep `Cargo.lock` updated
  (`--locked` builds must pass).

### Task 2 — Verify
- `cargo test --test procsub_stop_pty -- --test-threads=1 --nocapture` on macOS:
  both tests run and pass. Run ≥5× (or a loop) to confirm reliability, not a
  lucky race.
- `cargo build --locked` + `cargo build --release --locked` (both `--bin huck`).
- `cargo test --workspace --locked` green.
- `tests/scripts/run_diff_checks.sh` green (build both binaries first).
- `cargo fmt --all` (CI enforces `--check`).

## Docs / memory / PR
- No `docs/bash-divergences.md` change (this was never an *intentional*
  divergence; the merged PR auto-closes #97 via `Closes #97`).
- Record v336 in `project_huck_iterations.md` + `MEMORY.md`: the durable lesson
  is the rustyline `TCSADRAIN`-vs-stalled-reader / macOS-pty-drain root cause and
  the "drain the master like a real terminal" test pattern — plus the debugging
  method (isolate with a bare-child control; `sample` the wedged process for the
  true blocking syscall; diff termios against bash).
- Optionally open a follow-up `enhancement` issue for a rustyline-independent
  raw-mode path (out of scope here).
- Commit spec+plan on `main`; comment on #97 with blob links. Implement on
  `v336-macos-procsub-stop`. Push branch, open PR `Closes #97`, hand to user to
  merge (do NOT merge).
