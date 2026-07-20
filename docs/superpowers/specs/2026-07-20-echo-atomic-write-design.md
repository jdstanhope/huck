# v317 — builtin `echo` writes its line in a single `write()`

**Issue:** [#208](https://github.com/jdstanhope/huck/issues/208) — backgrounded
builtin `echo` is not atomic-per-line, so concurrent `echo`s interleave mid-line.
Root cause of the recurring CI flakes `amp_in_for_body`, `nvm_shaped_function`,
and the coproc harness.

**Goal:** `builtin_echo` emits its whole line (content + trailing newline) in ONE
`write_all` — i.e. one `write(2)` for a line ≤ `PIPE_BUF` — matching bash, so
concurrent backgrounded `echo`s no longer interleave between content and newline.

---

## Root cause

`FdWriter` (`crates/huck-engine/src/fd_writer.rs`) is **unbuffered**: each
`write_all` becomes a direct `libc::write` syscall (looping only on a short
count). `builtin_echo` (`builtins.rs:668`) does **two** `write_all` calls:

```rust
    if out.write_all(&bytes).is_err() { return ExecOutcome::Continue(1); }   // 1: content
    if !suppress_newline && out.write_all(b"\n").is_err() { ... }            // 2: newline
```

So `echo a` issues `write("a")` then `write("\n")` — two syscalls. Under
concurrency (`echo a & echo b &`, or `echo $i >> f &` in a loop) another
process's `write` lands between them, gluing content and misplacing newlines
(observed: `echo b & echo a & | sort` → `\nab\n` instead of `a\nb\n`; the
`>>` variant → `\n21\n3\n`). bash writes the whole line in one `write()`, so its
concurrent writes never split.

`printf` already builds its whole output into one buffer and does a single
`write_all` (`builtins.rs:~4150`) — it is already atomic. **echo is the only
culprit** (per the echo-only scope decision); other output builtins are not
backgrounded in tight concurrent loops.

## Design

Append the trailing newline to the content buffer and do a **single**
`write_all`:

```rust
    let mut bytes = if process_escapes {
        let (b, hit_c) = process_echo_escapes(&joined);
        if hit_c { suppress_newline = true; }
        b
    } else {
        joined.into_bytes()
    };
    if !suppress_newline {
        bytes.push(b'\n');
    }
    if out.write_all(&bytes).is_err() {
        // v308: reported once by the epilogue.
        return ExecOutcome::Continue(1);
    }
    ExecOutcome::Continue(0)
```

For a line ≤ `PIPE_BUF` (4096 on Linux) `FdWriter`'s single `write` is one
atomic syscall; larger lines loop in `write_all` and are not atomic — the same
limit bash has (bash's `echo` is also non-atomic above `PIPE_BUF`), so no extra
guarantee is claimed or needed.

**Invariants preserved:**
- `echo -n` (suppress newline): `bytes` has no `\n`, one `write_all` of the
  content. Unchanged output.
- `echo -e` with a `\c` escape: `process_echo_escapes` sets `suppress_newline`;
  no `\n` appended. Unchanged.
- **The zero-byte rule (v308):** `echo -n ''` → `bytes` empty →
  `write_all(&[])` → `FdWriter::write` returns `Ok(0)` and issues NO syscall
  (so a bad fd stays silent, rc 0). Still holds, since the newline is only
  pushed when not suppressed and there is content or a newline to write. Note
  `echo ''` → `bytes = "\n"` → one `write_all` (one syscall) — same as before.
- **The v308 write-error surface:** coalescing to one `write_all` means one error
  site instead of two; the epilogue still reports the error once (rc 1). The
  `FdWriter.first_errno` capture is unaffected.

This is the entire change to `builtin_echo` — a few lines, no new types, no
signature change.

## Testing

- **New unit test with a recording writer** (deterministic — the flakes are
  timing-dependent and cannot be reproduced reliably on the 1-core box). Define a
  small `Write` impl that pushes each `write` call's buffer into a `Vec<Vec<u8>>`:
  ```rust
  struct RecordingWriter { calls: Vec<Vec<u8>> }
  impl std::io::Write for RecordingWriter {
      fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
          self.calls.push(buf.to_vec());
          Ok(buf.len())
      }
      fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
  }
  ```
  Run `builtin_echo(&["echo".into(), "hi".into()], &mut rec, &mut sink, &shell)`
  and assert `rec.calls == vec![b"hi\n".to_vec()]` — **exactly one** `write`
  call containing the whole line. (Before the fix this is two calls: `b"hi"`,
  `b"\n"`.) Add companion cases: `echo -n hi` → one call `b"hi"`; `echo` (no
  args) → one call `b"\n"`; `echo -n ''` → ZERO calls (zero-byte rule).
  NOTE: `write_all` on the recording writer calls `write` once per call since it
  returns the full length — so "one `write_all`" ⇒ "one recorded call", which is
  what the test asserts.
- **Reliability check (not a gate):** the previously-flaky integration tests
  `async_list_integration::amp_in_for_body` and
  `subshell_pipeline_position_integration::nvm_shaped_function` should now pass;
  run each at `--test-threads 2` (they pass on the 1-core box regardless, but
  confirm no regression). The real proof is the atomic-write unit test; the CI
  flakes stop because the root is fixed.
- **Regression:** the existing `echo` diff harnesses / lib tests
  (`/usr/bin/grep -rln 'builtin_echo\|echo ' crates/huck-engine/src tests/`) and
  the full `run_diff_checks.sh` stay green — output is byte-identical (only the
  syscall boundary moves).

## Rejected alternatives

- **Make the flaky TESTS robust** (per-iteration files, different completion
  check) instead of fixing echo. Rejected: papers over a real bash divergence;
  the root fix makes the tests reliable AND matches bash.
- **Buffer all builtin output** behind a `BufWriter` that flushes once per
  builtin. Larger change, reintroduces the v308 buffering hazards
  (`LineWriter`/flush-error ordering), and unnecessary — echo just needs to
  build its one line before writing.
- **Audit + coalesce every output builtin.** Out of scope (echo-only decision);
  `printf` is already atomic and nothing else races.
