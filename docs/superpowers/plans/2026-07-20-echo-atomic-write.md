# v317 — builtin `echo` atomic single-`write()` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `builtin_echo` emits its whole line (content + newline) in one `write_all` (one `write(2)` for a line ≤ `PIPE_BUF`), so concurrent backgrounded `echo`s no longer interleave — fixing the `amp_in_for_body` / `nvm_shaped_function` / coproc CI flakes.

**Architecture:** Append the trailing `\n` to `builtin_echo`'s content buffer and do a single `write_all` (matching `printf`, already atomic, and bash). A deterministic recording-writer unit test asserts one `write` call per line.

**Tech Stack:** Rust (huck-engine crate), bash-diff harnesses.

## Global Constraints

- **Issue:** [#208](https://github.com/jdstanhope/huck/issues/208). Spec: `docs/superpowers/specs/2026-07-20-echo-atomic-write-design.md`.
- `builtin_echo` is in `crates/huck-engine/src/builtins.rs` (~line 668).
- Preserve: `echo -n` (no newline), `echo -e` `\c` (suppress newline), the v308 zero-byte rule (`echo -n ''` issues NO write / no syscall, rc 0), and the v308 single write-error report (rc 1 via the epilogue).
- Output must stay byte-identical (only the syscall boundary moves) — existing echo harnesses + lib tests + full `run_diff_checks.sh` stay green.
- **Box/build:** `cargo build -p huck --bin huck`; `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`. Touched `-p huck` integration binaries at `--test-threads 2`. NEVER `cargo test --workspace`. `cargo fmt --all` before commit. `/usr/bin/grep` only.
- Commit trailer (exact): `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

## File structure

- `crates/huck-engine/src/builtins.rs` — the `builtin_echo` change (~line 668) + a recording-writer unit test in its `#[cfg(test)]` module.

---

### Task 1: One `write_all` in `builtin_echo` + recording-writer unit test

**Files:**
- Modify: `crates/huck-engine/src/builtins.rs` (`builtin_echo` ~668; `#[cfg(test)]` module)

- [ ] **Step 1: Write the failing recording-writer test.** Add to `builtins.rs`'s `#[cfg(test)]` module (find it with `/usr/bin/grep -n '#\[cfg(test)\]' crates/huck-engine/src/builtins.rs`; construct the Shell as the existing echo/builtin tests there do — `Shell::new()`). If no suitable module exists, add one at the end of the file.
```rust
#[cfg(test)]
mod echo_atomic_tests {
    use super::*;

    struct RecordingWriter { calls: Vec<Vec<u8>> }
    impl std::io::Write for RecordingWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.calls.push(buf.to_vec());
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }

    fn echo_calls(args: &[&str]) -> Vec<Vec<u8>> {
        let shell = Shell::new();
        let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let mut rec = RecordingWriter { calls: Vec::new() };
        let mut sink: Vec<u8> = Vec::new();
        let _ = builtin_echo(&owned, &mut rec, &mut sink, &shell);
        rec.calls
    }

    #[test]
    fn echo_writes_line_in_one_call() {
        // The whole line (content + newline) must arrive in ONE write() call, so
        // concurrent backgrounded echoes can't interleave between them (#208).
        assert_eq!(echo_calls(&["echo", "hi"]), vec![b"hi\n".to_vec()]);
    }

    #[test]
    fn echo_n_writes_content_only_one_call() {
        assert_eq!(echo_calls(&["echo", "-n", "hi"]), vec![b"hi".to_vec()]);
    }

    #[test]
    fn echo_no_args_writes_just_newline_one_call() {
        assert_eq!(echo_calls(&["echo"]), vec![b"\n".to_vec()]);
    }

    #[test]
    fn echo_n_empty_issues_no_write() {
        // v308 zero-byte rule: empty output must not issue a write() at all.
        assert_eq!(echo_calls(&["echo", "-n", ""]), Vec::<Vec<u8>>::new());
    }
}
```
Confirm the `Shell::new()` construction compiles against the real API (adjust to match the module's existing test setup if it differs — e.g. a helper that builds a `Shell`).

- [ ] **Step 2: Run — verify the FIRST test FAILS.** `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 echo_writes_line_in_one_call`. Expected: FAIL — the current `builtin_echo` produces TWO calls (`[b"hi", b"\n"]`), so `assert_eq!` fails `left: [[104,105],[10]]`. (The `-n`/empty tests may already pass — that's fine; the first test is the one pinning the fix.)

- [ ] **Step 3: Apply the fix.** Replace the two-`write_all` tail of `builtin_echo` (currently):
```rust
    if out.write_all(&bytes).is_err() {
        // v308: reported once by the epilogue (see pwd above).
        return ExecOutcome::Continue(1);
    }
    if !suppress_newline && out.write_all(b"\n").is_err() {
        return ExecOutcome::Continue(1);
    }
    ExecOutcome::Continue(0)
```
with a single `write_all` after appending the newline. Change `let bytes` to `let mut bytes` (if not already) and:
```rust
    if !suppress_newline {
        bytes.push(b'\n');
    }
    if out.write_all(&bytes).is_err() {
        // v308: reported once by the epilogue (see pwd above).
        return ExecOutcome::Continue(1);
    }
    ExecOutcome::Continue(0)
```

- [ ] **Step 4: Run — verify all four tests PASS.** `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 echo_`. Expected: 4/4 pass (`echo hi` now ONE call `b"hi\n"`; `-n hi` one call `b"hi"`; no-args one call `b"\n"`; `-n ''` zero calls).

- [ ] **Step 5: Full lib + echo harness regression.** `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1` (green). Run the echo-touching bash-diff harnesses: `/usr/bin/grep -rl 'echo' tests/scripts/*_diff_check.sh | head` then run a representative few (e.g. `bash tests/scripts/echo_diff_check.sh` if it exists, plus `ansic_unicode_escape_diff_check.sh`) — output byte-identical. Confirm `./target/debug/huck -c 'echo hi'` and `echo -n hi` / `echo` / `echo -e 'a\tb'` match bash (`cargo build -p huck --bin huck` first).

- [ ] **Step 6: Reliability check — the previously-flaky integration tests.** Run at `--test-threads 2`:
  `ulimit -v 2500000; cargo test -p huck --test async_list_integration --jobs 1 -- --test-threads 2` and
  `ulimit -v 2500000; cargo test -p huck --test subshell_pipeline_position_integration --jobs 1 -- --test-threads 2`
  (both fully green — these hold `amp_in_for_body` / `nvm_shaped_function`; they passed on the 1-core box before too, so this confirms no regression, not the fix itself). Report counts.
- [ ] **Step 7: Full diff-check sweep.** Build both debug+release (`cargo build -p huck --bin huck` + `cargo build --release -p huck --bin huck`); `ulimit -v 1500000; timeout 900 bash tests/scripts/run_diff_checks.sh 2>&1 | tail -6` (green — echo output byte-identical). `coproc_diff_check.sh` was itself a #208-class flake; with the fix it should be reliably green — if any harness fails, investigate (do NOT hand-wave).
- [ ] **Step 8: fmt + commit** (`v317: builtin echo writes its line in one write_all (#208)`). The merged PR closes #208 via `Closes #208`; no divergence-doc entry needed (bug fix, not an intentional divergence).

---

## Notes for the executor

- The whole behavioral change is one syscall boundary moving; if ANY echo output changes bytes (not just syscall count), something is wrong.
- The recording-writer test is the deterministic gate. Do NOT add a concurrency/stress test — it would itself be flaky (the exact problem #208 describes).
- `printf` is already atomic (one `write_all` of its whole buffer) — do NOT touch it.
