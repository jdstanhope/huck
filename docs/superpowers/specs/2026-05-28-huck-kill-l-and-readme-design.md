# huck v41 — `kill -l` (M-39) + README cleanup

## Goal

Two small, related changes:

- **M-39**: `kill -l` — list signal names; bash's three lookup forms
  (`-l NAME` → number, `-l NUM` → name, `-l <status≥128>` → name via
  N-128); multiple args produce one decode per line.
- **README cleanup**: surgical trim of the "Not yet implemented"
  paragraph at `README.md:233-238` to remove items shipped in v33, v37,
  v40, and v41.

Bundle is natural: both touch `src/builtins.rs` and README. Both are
small. No new modules.

## Scope decisions (locked)

1. **`kill -l` argument forms**: include all bash forms — bare list,
   name→num, num→name, and status-decode (`-l 137` → `KILL`).
2. **README cleanup**: surgical — remove `${var:off:len}`,
   `${var^^}`/`${var,,}`, `wait -n`, `kill -l`; keep `kill -s`, brace
   expansion, extended job specs, `disown -a/-r/-h`, backgrounded
   multi-pipelines, aliases.
3. **Signal coverage**: all 16 (the 14 trappable from
   `src/traps.rs::TRAPPABLE` + KILL + STOP). New helper
   `killable_signals()` in `src/traps.rs` is the single source of
   truth.

## Out of scope (deferred)

- M-40 (`kill -s SIGNAME`). Separate iteration.
- M-41 (full platform signal set: SEGV, ABRT, FPE, BUS, ILL, TRAP, …).
- Localized signal listing formats.
- Sysv `kill -<sig> -l` (the `-l` AFTER `-<sig>`). v41 only recognizes
  `-l` as the first positional.

## Architecture

All code changes confined to `src/traps.rs` (one new helper) and
`src/builtins.rs` (kill dispatcher extension + a new helper +
deduplication of `signal_by_name`). One new integration test file.
README receives a content edit.

### New helper in `src/traps.rs`

```rust
/// Returns the table of signal names huck knows how to SEND via
/// `kill`. This is the 14-entry `TRAPPABLE` table plus KILL and STOP,
/// which are not trappable but ARE sendable. Order matches the natural
/// signal-number order on the host.
pub fn killable_signals() -> &'static [(&'static str, i32)] {
    KILLABLE
}
```

Backed by a new `const KILLABLE: &[(&str, i32)]` slice defined next to
`TRAPPABLE`. The list duplicates the 14 trappable entries plus adds
`("KILL", libc::SIGKILL)` and `("STOP", libc::SIGSTOP)`.

We do NOT remove `TRAPPABLE` or `name_table()` — those remain the
source of truth for trappable signals (which is a different concept).

### Extended `builtin_kill` dispatcher

The existing dispatcher (`src/builtins.rs:753`+) starts by reading the
first arg. v41 adds a `-l` short-circuit:

```rust
fn builtin_kill(args: &[String], shell: &mut Shell) -> ExecOutcome {
    if matches!(args.first().map(|s| s.as_str()), Some("-l")) {
        return handle_kill_l(&args[1..]);
    }
    // ... existing logic unchanged ...
}
```

`handle_kill_l` prints to stdout (using `println!` for the listing
output and `eprintln!` for errors, matching the existing builtin
convention — `kill` doesn't accept a passed `out` writer today, so
stdout writes go through `println!` to keep the change tight).

### `handle_kill_l` helper

```rust
fn handle_kill_l(args: &[String]) -> ExecOutcome
```

Behavior table:

| Input | Behavior | Stdout | Status |
|---|---|---|---|
| `[]` (bare `-l`) | Print 4-column `NUM) NAME` table | 16 entries | 0 |
| `["TERM"]` | Name → number | `15\n` (or libc::SIGTERM) | 0 |
| `["SIGTERM"]` | Name → number (SIG prefix stripped) | same | 0 |
| `["term"]` | Name → number (case-insensitive lookup) | same | 0 |
| `["15"]` | Number → name | `TERM\n` | 0 |
| `["137"]` | Status decode (137-128=9) | `KILL\n` | 0 |
| `["1", "9", "15"]` | One decode per arg | `HUP\nKILL\nTERM\n` | 0 |
| `["XYZ"]` | Unknown name | (nothing) + stderr msg | 1 |
| `["99"]` | Number not in table | (nothing) + stderr msg | 1 |
| `["1", "XYZ"]` | First arg prints, second errors | `HUP\n` + stderr msg | 1 |

Error message: `huck: kill: <arg>: invalid signal specification` (one
line per bad arg; subsequent good args after a bad one are NOT
processed — stop at first error).

### Number → name lookup rule

- If `n >= 128`: subtract 128 (status-decode convention).
- Look up the resulting `n` in `killable_signals()`. Return the name
  on success.
- If not found OR negative: error.

This means `kill -l 137` → `KILL` (137-128=9). `kill -l 9` → `KILL`
directly. `kill -l 256` → error (256-128=128, not in table).

### Name → number lookup rule

- Strip optional `SIG` prefix (case-insensitive).
- Uppercase the remainder.
- Look up in `killable_signals()`. Return the number on success.
- If not found: error.

### Bare-list output format

Reuses the existing `print_signal_table` (`src/builtins.rs:1153`)
4-column format. Since `print_signal_table` is hard-wired to consume
`name_table()` (14 entries), we DON'T reuse it directly — instead a
new `print_killable_table` function uses the same format string but
iterates `killable_signals()` (16 entries). Two functions, ~10 lines
each. Minor duplication accepted to avoid coupling `trap -l` to
`kill -l`'s signal set.

### `signal_by_name` deduplication

The existing `signal_by_name` (`src/builtins.rs:712-734`) hardcodes
15 signal names. Replace its body with a lookup over
`killable_signals()`:

```rust
fn signal_by_name(s: &str) -> Option<i32> {
    let upper = s.to_ascii_uppercase();
    let name = upper.strip_prefix("SIG").unwrap_or(&upper);
    crate::traps::killable_signals()
        .iter()
        .find_map(|(table_name, num)| {
            if *table_name == name {
                Some(*num)
            } else {
                None
            }
        })
}
```

Result: `kill -WINCH pid` now works (it was rejected before because
WINCH wasn't in the hardcoded 15-name table).

### README trim

The current paragraph (`README.md:233-238`):

```
**Not yet implemented:**
substring parameter expansion (`${var:off:len}`),
case modification (`${var^^}`/`${var,,}`),
brace expansion (`{a,b,c}`), extended job specs
(`%cmd`/`%?cmd`), `wait -n`, `kill -l`/`-s`, `disown -a`/`-r`/`-h`,
backgrounded multi-pipeline sequences (`cmd1 && cmd2 &`), aliases.
```

Replaced with:

```
**Not yet implemented:**
brace expansion (`{a,b,c}`), extended job specs
(`%cmd`/`%?cmd`), `kill -s`, `disown -a`/`-r`/`-h`,
backgrounded multi-pipeline sequences (`cmd1 && cmd2 &`), aliases.
```

(Same surrounding markdown — the paragraph above and the "## Project
layout" heading below are unchanged.)

## Test plan

### Unit tests in `src/builtins.rs#[cfg(test)] mod tests`

~8 tests. `handle_kill_l` writes to `println!`, so the unit tests
need to either invoke through `run_builtin` (which captures via the
existing `&mut buf` writer) or call `handle_kill_l` directly and
inspect the return code only. The cleanest path: make
`handle_kill_l(args, out: &mut dyn Write) -> ExecOutcome` and have
the dispatcher pass an `out` writer through. We already pass `out`
into `builtin_jobs` etc., so the existing
`run_builtin(name, args, &mut buf, &mut shell)` infrastructure works.

This means `builtin_kill`'s signature needs to change to accept the
`out: &mut dyn Write` parameter (currently only takes `args` and
`shell`). The wiring update is one line in the dispatcher table at
`src/builtins.rs:55`.

Test list:

1. `kill_l_no_args_lists_all_16_signals` — bare `-l` writes 16 entries
   to the buffer (verify by counting occurrences of `)` in the output
   = 16).
2. `kill_l_with_name_returns_number` — `kill -l TERM` → buffer
   contains `15\n`.
3. `kill_l_with_sig_prefix_returns_number` — `kill -l SIGTERM` → same.
4. `kill_l_lowercase_name_returns_number` — `kill -l term` → same.
5. `kill_l_with_number_returns_name` — `kill -l 15` → buffer contains
   `TERM\n`.
6. `kill_l_status_decode` — `kill -l 137` → buffer contains `KILL\n`.
7. `kill_l_unknown_name_errors_status_1` — `kill -l xyz` → status 1.
8. `kill_l_invalid_number_errors_status_1` — `kill -l 99` → status 1.
9. `kill_l_multiple_args_decodes_each` — `kill -l 1 9 15` → buffer
   contains `HUP\n`, `KILL\n`, `TERM\n`.

Plus one test for the deduplication fix:

10. `kill_winch_now_resolves` — drive `signal_by_name` directly (or
    through a small test of the existing parser path) and assert
    `signal_by_name("WINCH")` returns `Some(libc::SIGWINCH)`.

### Integration tests at `tests/kill_l_integration.rs`

Two binary-driven tests:

1. `kill_l_bare_lists_signals` — `kill -l` then exit; stdout contains
   `TERM` AND `KILL` (verifies the listing reaches the binary path).
2. `kill_l_name_to_number` — `kill -l TERM` then exit; stdout has a
   line `15`.

### Smoke

`cargo test --all-targets` must pass after the change. PTY flake
tolerated per prior iterations.

## Implementation tasks

1. **`killable_signals` + builtin core**: add `KILLABLE` + the new
   `killable_signals()` helper in `src/traps.rs`; extend
   `builtin_kill` dispatcher (signature change to accept `out`); add
   `handle_kill_l` + `print_killable_table`; replace `signal_by_name`
   body with `killable_signals()` lookup; add 10 unit tests; verify
   no other callers of `builtin_kill` break (the dispatcher table
   row at `src/builtins.rs:55` is the only call site).
2. **Integration tests**: create `tests/kill_l_integration.rs` with
   the 2 scenarios.
3. **Docs + README**: flip M-39 to `[fixed v41]` in
   `docs/bash-divergences.md`; add v41 change-log entry; add v41 row
   to README version table; trim the "Not yet implemented"
   paragraph. Full-suite verify.

Three tasks. TDD within each, one commit per task.

## Acceptance criteria

- All 10 new unit tests pass.
- Both integration tests pass.
- `cargo test --all-targets` passes (modulo known PTY flake).
- `cargo clippy --all-targets -- -D warnings` passes.
- `docs/bash-divergences.md` shows M-39 as `[fixed v41]`.
- `README.md` "Not yet implemented" paragraph no longer mentions any
  v33/v37/v40/v41 shipped items.
- `kill -l TERM` writes `15` to stdout and exits 0.
- `kill -l 137` writes `KILL` to stdout and exits 0.
- `kill -l` (no args) writes a 4-column 16-entry table to stdout.
- `kill -WINCH 0` resolves the signal name (regression of the
  `signal_by_name` bug).
