# huck v15: PTY-Based Interactive Test Harness

**Date:** 2026-05-21
**Status:** Design

## Goal

Add automated, golden-path regression coverage for huck's
interactive features — tab completion, history recall, and Ctrl-C
handling — by driving the shell through a real pseudo-terminal
(PTY). These features cannot be reached by the existing piped-stdin
integration tests, so they currently have no end-to-end coverage and
are at risk of silent regression as huck grows.

## Background

rustyline checks `isatty()` and disables completion and line editing
when stdin is not a terminal; signal-driven behavior (Ctrl-C) needs a
controlling terminal; arrow keys are escape sequences only processed
in raw TTY mode. The piped-stdin integration suites
(`arith_integration`, `glob_integration`, `history_integration`,
`param_expansion_integration`) therefore structurally cannot exercise
these features. A PTY-based harness gives the child a real
controlling terminal, so rustyline runs in full interactive mode.

## Scope

**In scope:**
- A PTY test harness using the `expectrl` crate (a Rust
  `expect`/`pexpect`) — spawn huck attached to a fresh PTY, send
  keystrokes, assert on output with timeouts
- Golden-path tests (~13) across three feature areas:
  - **Tab completion** — builtin, double-Tab list, filename,
    directory, variable
  - **History recall** — up-arrow, up-arrow twice, down-arrow
  - **Ctrl-C handling** — empty prompt, partial line, breaking out
    of `wait`, plus Ctrl-D exit
- Per-test environment isolation (temp `HISTFILE`, controlled
  `HOME`/`PATH`/custom vars where needed)
- Graceful skip when PTY allocation fails (restricted sandbox)
- Tests run by default under `cargo test`

**Out of scope (deferred):**
- Ctrl-Z / `fg` / `bg` job-control signal tests — the fiddliest tier
  (process groups, terminal handoff); a candidate for a later
  iteration
- Exhaustive interactive coverage (every key binding, every
  completion edge case) — the per-area logic is already unit-tested
- Any change to `src/` — v15 is purely test infrastructure
- CI configuration

## Architecture

A single new test file, `tests/pty_interactive.rs`, contains the
harness helpers and all golden-path tests. v15 changes **no `src/`
code**: the only production-tree change is adding `expectrl` to
`[dev-dependencies]` in `Cargo.toml`.

### `expectrl` dependency

`expectrl` is added as a dev-dependency. It allocates its own PTY for
the spawned child, so the `cargo test` process itself needs no TTY.
The version is whatever `cargo add expectrl --dev` resolves to at
implementation time; pin it in `Cargo.toml`.

### Spawn helper

A helper at the top of `tests/pty_interactive.rs`:

```rust
fn spawn_huck(cwd: &Path, env: &[(&str, &str)]) -> Result<Session, ExpectrlError>;
```

It builds a `std::process::Command` for `env!("CARGO_BIN_EXE_huck")`,
sets `current_dir(cwd)` and the supplied environment overrides, and
spawns it through `expectrl` inside a fresh PTY. The returned
`Session` is the read/write handle to the PTY master.

The exact `expectrl` spawn API (e.g. `Session::spawn(Command)` vs
`expectrl::spawn(str)`) is resolved at implementation time against
the pinned version; the helper's signature above is the stable
contract the tests depend on.

### Environment isolation

Every spawn sets, at minimum:
- `HISTFILE` → a per-test temp file (via `tempfile::TempDir`), so
  history tests start empty and never read or write
  `~/.huck_history`.

Tests that need them also set:
- `HOME` → a temp directory (tilde-related checks).
- `PATH` → a controlled value (command-completion determinism).
- Custom variables (e.g. `HUCKPTYVAR`) for variable-completion tests.

Each test spawns its own huck process and terminates it by sending
`exit\r` (or, for the Ctrl-D test, `\x04`).

### Graceful skip

`spawn_huck` returns a `Result`. PTY allocation can fail in a
restricted sandbox with no `/dev/ptmx`. Each test handles this by
skipping — an early `return`, which the Rust test framework records
as a pass (it has no runtime "skipped" state):

```rust
let mut session = match spawn_huck(dir.path(), &env) {
    Ok(s) => s,
    Err(e) => {
        eprintln!("pty_interactive: skipping — no PTY available: {e}");
        return;
    }
};
```

This is safe. A genuinely broken huck binary is still caught: the
four piped-stdin integration suites all execute the binary and would
fail. A skip here happens only on PTY-*allocation* failure. If the
PTY allocates but huck then misbehaves, the test's `expect` calls
time out and the test fails for real.

## Keystroke encoding

Bytes sent over the PTY master:

| Key | Bytes |
|---|---|
| Tab | `\t` |
| Enter | `\r` |
| Up arrow | `\x1b[A` |
| Down arrow | `\x1b[B` |
| Ctrl-C | `\x03` |
| Ctrl-D (EOF) | `\x04` |

Use `expectrl`'s raw byte-send (`send`) for control characters and
escape sequences; `send_line` may be used for ordinary command text
where appending the line terminator is wanted.

## Assertion approach

Every `send` is followed by an `expect` of the output it should
produce. `expectrl`'s `expect` reads the stream until the pattern
appears or a timeout fires — so there are **no fixed sleeps** and no
races. The harness sets a single generous expect timeout (on the
order of 10 seconds) at spawn time.

Assertions use **unique marker strings** (`PTYMARKER`,
`histmarker`, `aftersigint`, …) so a match cannot collide with the
prompt text or earlier output.

**"Completion landed" technique.** To prove a completion fired,
complete the token, append a unique argument, and run the line: send
`ec\t`, then ` PTYMARKER\r`. If `ec` completed to `echo`, the
executed command is `echo PTYMARKER` and the output contains
`PTYMARKER`. A failed completion produces different output and the
`expect` times out.

## The test set (~13 tests)

### Harness smoke (1)

- **`pty_huck_starts_and_exits`** — spawn; `expect("huck> ")`; send
  `exit\r`; expect the session to end (EOF). Validates the harness
  itself.

### Tab completion (5)

- **`tab_completes_builtin`** — send `ec\t` then ` PTYMARKER\r`;
  expect output `PTYMARKER`.
- **`tab_double_tab_lists`** — at an empty prompt send `\t\t`; expect
  the listed output to contain `echo` and `history`. Then send
  `\x03` to clear and `exit\r`.
- **`tab_completes_filename`** — cwd is a temp dir containing
  `ptyfile_unique.txt`; send `echo ptyfile_un\t` then `\r`; expect
  output to contain `ptyfile_unique.txt`.
- **`tab_completes_directory_slash`** — cwd is a temp dir containing
  a subdirectory `ptydir_unique`; send `echo ptydir_un\t`; expect the
  redrawn line to contain `ptydir_unique/`.
- **`tab_completes_variable`** — spawn with env `HUCKPTYVAR=ptyvarvalue`;
  send `echo $HUCKPTY\t` then `\r`; expect output `ptyvarvalue`.

### History recall (3)

- **`up_arrow_recalls_previous`** — send `echo histmarker\r`, expect
  `histmarker`; send `\x1b[A` (up arrow); send `\r`; expect
  `histmarker` printed again.
- **`up_arrow_twice_recalls_older`** — send `echo older\r` then
  `echo newer\r`; send `\x1b[A\x1b[A`; send `\r`; expect `older`.
- **`down_arrow_navigates_forward`** — run two commands; send up,
  up, down; send `\r`; expect the newer command's output.

### Ctrl-C handling (4)

- **`ctrl_c_empty_prompt_survives`** — send `\x03` at an empty
  prompt; then send `echo aftersigint\r`; expect `aftersigint` (the
  shell survived the signal).
- **`ctrl_c_clears_partial_line`** — type `echo partial` with no
  Enter; send `\x03`; then send `echo afterclear\r`; expect an
  output line of exactly `afterclear` (the abandoned partial line was
  not prepended).
- **`ctrl_c_breaks_out_of_wait`** — send `sleep 30 &\r`, expect
  `[1]`; send `wait\r`; send `\x03`; then send `echo afterwait\r`;
  expect `afterwait`. Confirms Ctrl-C breaks a blocking `wait` and
  the shell stays alive (the v6 regression). The backgrounded
  `sleep 30` is orphaned to init when huck exits and finishes on its
  own — harmless.
- **`ctrl_d_empty_prompt_exits`** — send `\x04` at an empty prompt;
  expect the session to end (EOF).

## Flakiness discipline

PTY tests are timing-sensitive only if written carelessly. The rules
this harness follows:

- **Never use a fixed `sleep`.** Every wait is an `expect` that
  blocks precisely until the awaited output arrives.
- **Always `expect` before the next `send`.** A `send` followed by
  another `send` without an intervening `expect` can race the
  child's processing; pairing each `send` with an `expect` of its
  effect serializes the interaction.
- **Generous timeout.** ~10 seconds — long enough that a slow CI
  host never spuriously fails, short enough that a genuine hang is
  reported in reasonable time.
- **Unique markers.** Every assertion target is a string that
  appears nowhere else in the session.

## Error handling summary

| Condition | Behavior |
|---|---|
| PTY allocation fails | test logs a skip notice and returns (passes) |
| huck binary misbehaves after a successful spawn | `expect` times out → test fails |
| `expect` pattern never appears within the timeout | test fails with the timeout error |
| Orphaned `sleep` from the `wait` test | reparented to init, exits on its own |

## File layout impact

- **New:** `tests/pty_interactive.rs` — spawn helper, keystroke
  helpers, and the ~13 golden-path tests
- **Modify:** `Cargo.toml` — add `expectrl` to `[dev-dependencies]`
- **Modify:** `README.md` — v15 row in the iteration table, a note
  on the PTY suite, updated test count
- **No `src/` changes.**

## Testing the harness

The harness validates itself: `pty_huck_starts_and_exits` is a smoke
test that fails if spawning, the PTY, or the prompt is broken. The
other twelve tests each exercise one interactive behavior. Running
`cargo test` runs them alongside everything else. A manual sanity
check after implementation: run `cargo test --test pty_interactive`
and confirm all tests pass (or skip cleanly where no PTY exists).

## Open questions

None at design time.

## References

- `expectrl` crate documentation (Rust `expect`/`pexpect`)
- rustyline 18 — interactive-mode behavior gated on `isatty()`
- ANSI escape sequences for arrow keys; ASCII control codes for
  Ctrl-C (`0x03`) and Ctrl-D (`0x04`)
