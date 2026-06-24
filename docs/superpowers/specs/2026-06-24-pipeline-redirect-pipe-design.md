# v212: Pipeline non-final stage with explicit stdout redirect leaks parent stdin (M-125)

## Goal

Fix M-125 (Tier 2 deferred): when a non-final pipeline stage has an
explicit stdout redirect (`>file` / `>>file`), the downstream stage
inherits the parent's stdin instead of an EOF inter-stage pipe. This
causes huck to HANG where bash returns, whenever the parent stdin is
blocking (terminal / FIFO).

Resolves M-125. Tier 2 count: 11 → 10.

## Background

Found in the v156 review; pre-existing, not introduced by v156.
Empirically verified 2026-06-24:

```
$ printf 'hello\n' | huck -c 'echo upstream > /tmp/x | cat'
hello                                    # ← cat read PARENT stdin

$ printf 'hello\n' | bash -c 'echo upstream > /tmp/x | cat'
                                         # ← cat read pipe (EOF)
```

The bug lives in the pipeline executor's stdout-fd selection:

```rust
let stdout_fd: RawFd = if let Some(fd) = explicit_stdout_fd {
    fd                              // upstream → file
} else if !is_last {
    let (r, w) = make_pipe()?;      // inter-stage pipe
    prev_pipe_read = Some(r);
    parent_held.extend([r, w]);
    w
} else { ... };
```

When `explicit_stdout_fd.is_some() && !is_last`, the `else if !is_last`
branch is skipped, so `prev_pipe_read` stays `None`, and the downstream
stage's stdin defaults to the parent's stdin.

With an already-EOF parent stdin (e.g. `huck -c 'frag' </dev/null`),
the observable result matches bash. With a blocking parent stdin
(terminal / FIFO), huck HANGS where bash returns.

Two pipeline builders both have the bug:
- `run_pipeline` (executor.rs:2658) — interactive / job-control path.
- `run_multi_stage` (executor.rs:5390) — multi-stage helper.

The stderr-redirect analog (`cmd 2>file | next`) is NOT affected
because stdout still flows through the inter-stage pipe. `cmd >&2 |
next` is also fine because Dup redirects aren't captured into
`explicit_stdout_fd` (the redirect happens inside the child via the
redirects list at exec time, not via the parent's fd-routing logic).

## Scope

**In scope:**

- Add `make_orphan_pipe_for_eof_reader() -> std::io::Result<RawFd>`
  helper near `make_pipe` in `executor.rs`. Creates a pipe, closes the
  write-end immediately, returns the read-end.
- Apply the fix at both pipeline-builder sites:
  - `run_pipeline` around `executor.rs:3085`.
  - `run_multi_stage` around `executor.rs:5800`.
- One unit test on `make_orphan_pipe_for_eof_reader`.
- One bash-diff harness `tests/scripts/pipeline_redirect_pipe_diff_check.sh`
  with 7 fragments covering the fix + adjacent no-bug guards.
- Delete the M-125 entry from `docs/bash-divergences.md`. Update Tier 2
  count from 11 to 10.

**Out of scope:**

- Deduplicating the two ~200-LOC pipeline builders. That's a future
  refactor (option 3 in brainstorming) with broader risk; v212 just
  fixes the specific bug.
- The stderr-redirect analog (already works correctly).
- Other pipeline-shape divergences not in M-125's scope.
- Coproc fd routing.

## Behavioral / observable changes

- **Before:** `cmd >file | next` gives `next` the parent's stdin.
  Hangs on blocking stdin; behaves correctly on already-EOF stdin.
- **After:** `cmd >file | next` gives `next` an EOF pipe-read, matching
  bash. The blocking-stdin hang is closed. The already-EOF case
  remains correct.
- No change to:
  - `cmd | next` (no redirect — unchanged path).
  - `cmd 2>file | next` (stderr redirect — unaffected).
  - `cmd >&2 | next` (dup redirect — unaffected).
  - `cmd | next >file` (last-stage redirect — unaffected).

## Fix details

### New helper

```rust
/// Create an inter-stage pipe for a downstream pipeline reader, where
/// the upstream stage's stdout is going elsewhere (an explicit file
/// redirect). Closes the write-end immediately so the downstream reader
/// sees EOF instead of inheriting parent stdin or blocking on an
/// orphaned write-end. Returns the read-end fd to thread into
/// `prev_pipe_read`. On `make_pipe` failure, the caller propagates the
/// error.
fn make_orphan_pipe_for_eof_reader() -> std::io::Result<RawFd> {
    let (r, w) = make_pipe()?;
    unsafe { libc::close(w); }
    Ok(r)
}
```

### Call-site shape

Both pipeline builders' `let stdout_fd: RawFd = ...` chain becomes:

```rust
let stdout_fd: RawFd = if let Some(fd) = explicit_stdout_fd {
    if !is_last {
        let r = match make_orphan_pipe_for_eof_reader() {
            Ok(r) => r,
            Err(e) => {
                // existing cleanup pattern (close held fds, drain procsubs,
                // restore inline assignments, etc.) — exact code mirrors the
                // existing `make_pipe()` error arm in the same function.
                { let mut err = err_writer(err_sink, sink);
                  e!(&mut *err, "huck: pipe: {e}"); }
                /* … function-specific cleanup … */
                return ExecOutcome::Continue(1);
            }
        };
        prev_pipe_read = Some(r);
        parent_held.push(r);
    }
    fd
} else if !is_last {
    // unchanged: normal inter-stage pipe creation.
    match make_pipe() { /* … */ }
} else {
    // unchanged: capture-sink pipe or stdout terminal.
};
```

### Why "orphan" naming

The helper produces a pipe whose write-end is closed before any child
fork. The downstream stage holds only the read-end; the parent holds
nothing. From the downstream stage's perspective the pipe is
"orphaned" of writers — `read()` immediately returns 0 (EOF). This
shape is distinct from a normal inter-stage pipe (where the upstream
child holds the write-end), so the helper name reflects the
end-of-pipe-life semantic.

## Edge cases

| Case | Pre-fix | Post-fix |
|---|---|---|
| `cmd >f \| cat` (blocking stdin) | hangs | EOF, returns |
| `cmd >f \| cat` (already-EOF stdin) | EOF (coincidental) | EOF |
| `cmd >>f \| cat` (append) | hangs | EOF |
| `cmd >&2 \| cat` (dup redirect) | EOF (correct already) | EOF |
| `cmd 2>e \| cat` (stderr redirect) | works (correct) | works |
| `cmd \| next >f` (final stage redirect) | works | works |
| `cmd >f \| echo \| cat` (3-stage, middle has redirect) | hangs | EOF |
| `cmd >/no/such/dir/f \| cat` (redirect fails) | error path, no fd leak | same error path; no new fd leak |

## Testing strategy

### Unit test (1 new)

In `crates/huck-engine/src/executor.rs::mod tests`:

```rust
#[test]
fn make_orphan_pipe_for_eof_reader_yields_immediate_eof() {
    use std::io::Read;
    use std::os::unix::io::FromRawFd;
    let r = make_orphan_pipe_for_eof_reader().expect("pipe");
    let mut f = unsafe { std::fs::File::from_raw_fd(r) };
    let mut buf = [0u8; 8];
    let n = f.read(&mut buf).expect("read");
    assert_eq!(n, 0, "expected EOF, got {n} bytes");
}
```

Verifies the write-end is genuinely closed before the function
returns.

### Bash-diff harness (7 fragments)

New `tests/scripts/pipeline_redirect_pipe_diff_check.sh`:

| Fragment label | Pattern | What it pins |
|---|---|---|
| `stdout-trunc-non-final-eof` | `printf 'X' \| huck -c 'echo up >/tmp/m125-out \| cat'` | the bug fix |
| `stdout-append-non-final-eof` | `printf 'X' \| huck -c 'echo up >>/tmp/m125-out \| cat'` | append redirect same fix |
| `stdout-redir-3-stage` | `printf 'X' \| huck -c 'echo a >/tmp/m125-out \| echo b \| cat'` | mid-pipeline redirect, downstream chain unaffected |
| `stderr-only-redir-no-bug` | `printf 'X' \| huck -c 'echo up 2>/tmp/m125-err \| cat'` | regression guard: stderr-redirect path stays correct |
| `dup-redirect-no-bug` | `printf 'X' \| huck -c 'echo up >&2 \| cat' 2>/dev/null` | regression guard: `>&2` path stays correct |
| `final-stage-redir-no-bug` | `printf 'X' \| huck -c 'cat \| tee /tmp/m125-out >/dev/null'` | final stage redirect: no inter-stage pipe needed |
| `redir-failure-still-skips` | `printf 'X' \| huck -c 'echo up >/no/such/dir/f \| cat' 2>&1 \| sed 's|huck:|<err>:|; s|bash:|<err>:|'` | redirect-open failure: pipe NOT created; no fd leak; error path matches |

Notes:
- All fragments use `printf 'X' |` to pipe a known byte into the
  harness's huck/bash invocation. Without the fix, the downstream
  stage in the buggy cases reads `'X'` and echoes it.
- The `redir-failure-still-skips` fragment normalizes the
  `huck:`/`bash:` error-message prefix via `sed` so the byte-compare
  works across shells. The actual error text after the prefix should
  match closely enough; if it doesn't, the fragment may need further
  normalization (truncate to the error keyword `No such file`).
- The `dup-redirect-no-bug` fragment redirects stderr to `/dev/null`
  in BOTH bash and huck (the `>&2` writes "up" to stderr; we don't
  want stderr in the byte-compare).

The harness follows the shape established by
`tests/scripts/array_transforms_diff_check.sh` and friends:
`cd "$(dirname "$0")/../.."`, `cargo build --quiet --workspace --bin
huck`, `check label frag` helper, `printf 'FAIL' / 'PASS'`, exit 1 on
any failure.

### Existing harness regression

Spot-check the relevant pipeline harnesses post-fix:
- `pipe_compound_redirect_diff_check.sh` — should still pass.
- `captured_pipeline_drain_diff_check.sh` — should still pass.
- `pipefail_diff_check.sh` — should still pass.
- `subshell_pipeline_position_diff_check.sh` — should still pass.
- `sigpipe_diff_check.sh` — sigpipe paths are upstream-write driven;
  closing the orphan pipe write-end SHOULDN'T cause a stray SIGPIPE
  in the upstream stage (which writes to the file, not the pipe).
  Spot-check to be sure.
- `builtin_pipe_flush_diff_check.sh` — should still pass.
- `heredoc_pipeline_diff_check.sh` — should still pass.

Run the full `for h in tests/scripts/*_diff_check.sh; do bash "$h"; done`
sweep at end of the implementation.

## Documentation updates

`docs/bash-divergences.md`:
- DELETE the M-125 entry entirely (Tier 2 11 → 10). Per the
  current-divergences-only policy.

`docs/architecture.md`: no change. This is a localized fix inside
`executor.rs`.

## Risks

1. **Fd-leak on the error path.** If the new `make_orphan_pipe_for_eof_reader`
   fails AFTER the upstream stage's `explicit_stdout_fd` was opened,
   we must close `explicit_stdout_fd` on the error path. Mitigation:
   mirror the existing `make_pipe()` error arm in the same function —
   it already handles `explicit_stdout_fd` cleanup. Both bug sites
   have this pattern in place; re-use it verbatim.
2. **Surprise SIGPIPE.** The upstream stage writes to the FILE, not
   the pipe, so it can't get SIGPIPE on the orphaned pipe. But if any
   downstream Dup redirect later wires its stdout BACK to the pipe
   write-end (which is closed) we'd get SIGPIPE. The redirects list
   inside each child is independent of the orphan pipe so this
   shouldn't happen, but `sigpipe_diff_check.sh` regression is the
   guard.
3. **Resource exhaustion under deep pipelines.** Each affected
   pipeline now consumes one extra fd (the read-end held by the
   parent until the downstream stage forks). No expected impact at
   normal pipeline depths.
4. **`run_pipeline` vs `run_multi_stage` divergence.** Applying the
   same shape to two ~200-LOC functions risks subtle drift. We use
   the same helper; the cleanup arm mirrors each function's existing
   `make_pipe()` error handler. Review both call sites side-by-side.

## Acceptance

- `cargo test --workspace --quiet` green.
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `cargo build --release --workspace --quiet` clean.
- New `pipeline_redirect_pipe_diff_check.sh` 7/7 PASS.
- All 130+ existing `*_diff_check.sh` harnesses still pass.
- M-125 entry deleted from `bash-divergences.md`; Tier 2 = 10.
- Headless smoke: `printf 'X' | ./target/release/huck -c 'echo up >/tmp/x | cat'`
  prints nothing (not `X`).
