# v132 — sink/context threading for `eval` / `source` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `eval` and `source`/`.` run their commands with the enclosing execution's `StdoutSink` (and job-control context) instead of a fresh `Terminal` sink — fixing the `nvm ls-remote` interactive hang, the `$()` output leak, and the ignored redirect on eval/source.

**Architecture:** Behavior-preserving refactor adding `_in_sink` variants of `execute`/`process_line`/`run_sourced_contents` (the public ones become Terminal-sink wrappers), then a new dispatch arm in `run_exec_single` that routes eval/source through `with_redirect_scope` (v125) when redirects are present, else threads the current sink — exactly mirroring the function-call branch.

**Tech Stack:** Rust. Tests: cargo integration (capture STDOUT/files), an expectrl PTY regression, a bash-diff harness.

**GIT SAFETY:** Do NOT `git checkout <sha>` — stay on `v132-eval-source-sink-threading`; edit, build, commit in place. Commit trailer on every commit: `Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`.

**Reference:** spec `docs/superpowers/specs/2026-06-10-eval-source-sink-threading-design.md`. Key locations: `execute` (executor.rs:47, creates `let mut sink = StdoutSink::Terminal;`); `process_line` (shell.rs:555, ends in `executor::execute(&sequence, shell, line)`); `run_sourced_contents` (builtins.rs:5353, per-unit `crate::executor::execute(&seq, shell, span)`); `builtin_eval` (builtins.rs:4698); `builtin_source` (builtins.rs:5270, uses `resolve_source_path` + `source_depth` + positional save/restore + `run_sourced_contents`); the function-call dispatch branch (executor.rs:3083-3091) using `with_redirect_scope` (executor.rs:509); the generic builtin branch begins executor.rs:3092 (`} else if builtins::is_builtin(...)`). `StdoutSink<'a>` enum (executor.rs:17, `Terminal | Capture(&mut Vec<u8>)`, `pub`).

**Factoring decision (least churn):** put `eval_in_sink`/`source_in_sink` in `src/builtins.rs` (next to the existing eval/source code and its `resolve_source_path` helper). They take `sink: &mut crate::executor::StdoutSink`. The dispatch arm in `run_exec_single` calls `crate::builtins::eval_in_sink` / `crate::builtins::source_in_sink`.

---

### Task 1: Behavior-preserving sink-threaded refactor

End state: identical behavior (eval/source still run via `run_builtin` → `builtin_eval`/`builtin_source` → the new `*_in_sink` helpers with a Terminal sink). The full existing suite MUST stay green — this task changes NO observable behavior.

**Files:**
- Modify: `src/executor.rs` (`execute` → `execute_with_sink` + wrapper)
- Modify: `src/shell.rs` (`process_line` → `process_line_in_sink` + wrapper)
- Modify: `src/builtins.rs` (`run_sourced_contents` → `_in_sink` + wrapper; add `eval_in_sink`/`source_in_sink`; `builtin_eval`/`builtin_source` delegate)

- [ ] **Step 1: `execute_with_sink` in `src/executor.rs`.** Rename the current `pub fn execute(seq, shell, source)` body into `pub fn execute_with_sink(seq, shell, source, sink: &mut StdoutSink)`, replacing its `let mut sink = StdoutSink::Terminal;` line by USING the passed `sink` (every `&mut sink` in the body becomes `sink`). Then add the thin wrapper:
```rust
pub fn execute(seq: &Sequence, shell: &mut Shell, source: &str) -> ExecOutcome {
    let mut sink = StdoutSink::Terminal;
    execute_with_sink(seq, shell, source, &mut sink)
}
```
Read `execute` first (executor.rs:47-87) — it has a background fast-path (several `return run_background_*(... &mut sink ...)` calls) plus a final `execute_sequence_body(seq, shell, &mut sink)`. In `execute_with_sink` those become `... sink)` (the passed `&mut StdoutSink`). Keep the doc comment.

- [ ] **Step 2: Build** — `cargo build 2>&1 | tail -3` (success).

- [ ] **Step 3: `process_line_in_sink` in `src/shell.rs`.** Rename the current `pub fn process_line(line, shell, expand_aliases)` body into `pub fn process_line_in_sink(line, shell, expand_aliases, sink: &mut crate::executor::StdoutSink)`, changing ONLY its final `executor::execute(&sequence, shell, line)` to `executor::execute_with_sink(&sequence, shell, line, sink)`. Add the wrapper:
```rust
pub fn process_line(line: &str, shell: &mut Shell, expand_aliases: bool) -> ExecOutcome {
    let mut sink = crate::executor::StdoutSink::Terminal;
    process_line_in_sink(line, shell, expand_aliases, &mut sink)
}
```

- [ ] **Step 4: `run_sourced_contents_in_sink` in `src/builtins.rs`.** Rename the current `run_sourced_contents(contents, path, shell)` body into `run_sourced_contents_in_sink(contents, path, shell, sink: &mut crate::executor::StdoutSink)`, changing ONLY its per-unit `crate::executor::execute(&seq, shell, span)` to `crate::executor::execute_with_sink(&seq, shell, span, sink)`. Add the wrapper:
```rust
pub(crate) fn run_sourced_contents(contents: &str, path: &std::path::Path,
        shell: &mut crate::shell_state::Shell) -> ExecOutcome {
    let mut sink = crate::executor::StdoutSink::Terminal;
    run_sourced_contents_in_sink(contents, path, shell, &mut sink)
}
```

- [ ] **Step 5: `eval_in_sink` / `source_in_sink` in `src/builtins.rs`.** Add:
```rust
pub(crate) fn eval_in_sink(args: &[String], shell: &mut Shell,
        sink: &mut crate::executor::StdoutSink) -> ExecOutcome {
    if args.is_empty() { return ExecOutcome::Continue(0); }
    let joined = args.join(" ");
    if joined.trim().is_empty() { return ExecOutcome::Continue(0); }
    crate::shell::process_line_in_sink(&joined, shell, true, sink)
}
```
For `source_in_sink`: copy `builtin_source`'s body VERBATIM but take the `sink` param and call `run_sourced_contents_in_sink(&contents, &path, shell, sink)` instead of `run_sourced_contents(...)`:
```rust
pub(crate) fn source_in_sink(args: &[String], shell: &mut Shell,
        sink: &mut crate::executor::StdoutSink) -> ExecOutcome {
    // … VERBATIM copy of builtin_source's body (arg check, source_depth cap,
    //   resolve_source_path, read_to_string, positional save/restore) …
    //   EXCEPT: `let result = run_sourced_contents_in_sink(&contents, &path, shell, sink);`
}
```

- [ ] **Step 6: Delegate `builtin_eval` / `builtin_source`.** Replace their bodies:
```rust
fn builtin_eval(args: &[String], shell: &mut Shell) -> ExecOutcome {
    let mut sink = crate::executor::StdoutSink::Terminal;
    eval_in_sink(args, shell, &mut sink)
}
fn builtin_source(args: &[String], shell: &mut Shell) -> ExecOutcome {
    let mut sink = crate::executor::StdoutSink::Terminal;
    source_in_sink(args, shell, &mut sink)
}
```

- [ ] **Step 7: Build + FULL regression (no behavior change).** `cargo build 2>&1 | tail -3`; `cargo test 2>&1 | grep -E "FAILED|error\[|panicked|test result: FAILED" | head` (NONE — this is a pure refactor, everything must stay green); `cargo clippy --all-targets 2>&1 | tail -3` (clean). If anything fails, the refactor diverged from verbatim — fix to match the original behavior.

- [ ] **Step 8: Confirm no behavior change** — run `./target/debug/huck -c 'x=$(eval "echo hi"); echo "[$x]"'` → still `[]` (leak NOT yet fixed — that's Task 2; this confirms Task 1 is behavior-preserving).

- [ ] **Step 9: Commit**
```bash
git add src/executor.rs src/shell.rs src/builtins.rs
git commit -m "$(cat <<'EOF'
refactor(v132): sink-threaded execute/process_line/source variants

Add execute_with_sink, process_line_in_sink, run_sourced_contents_in_sink, and
eval_in_sink/source_in_sink; the public execute/process_line/run_sourced_contents
become Terminal-sink wrappers and builtin_eval/builtin_source delegate. Pure
refactor — no behavior change yet (eval/source still run via run_builtin with a
Terminal sink). Wires the plumbing for the v132 dispatch fix.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Dispatch arm — eval/source thread the enclosing sink (the fix)

**Files:**
- Create: `tests/eval_source_sink_integration.rs`
- Modify: `src/executor.rs` (the dispatch arm in `run_exec_single`)

- [ ] **Step 1: Write the failing integration tests** — create `tests/eval_source_sink_integration.rs`:
```rust
//! v132: eval/source run with the enclosing StdoutSink (capture/redirect),
//! not a fresh Terminal sink.
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }
fn run(script: &str) -> (String, String, i32) {
    let mut child = Command::new(huck_bin())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().expect("spawn huck");
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(),
     String::from_utf8_lossy(&out.stderr).into_owned(),
     out.status.code().unwrap_or(-1))
}

#[test]
fn eval_captured_in_subst() {
    let (o, _e, _c) = run("x=$(eval 'echo hi'); echo \"[$x]\"\n");
    assert_eq!(o, "[hi]\n", "o: {o:?}");
}
#[test]
fn eval_multi_command_captured() {
    let (o, _e, _c) = run("x=$(eval 'echo a; echo b'); echo \"[$x]\"\n");
    assert_eq!(o, "[a\nb]\n", "o: {o:?}");
}
#[test]
fn eval_pipe_inside_capture() {
    let (o, _e, _c) = run("x=$(eval 'seq 1 100 | wc -l'); echo \"[$x]\"\n");
    assert_eq!(o.trim(), "[100]", "o: {o:?}");
}
#[test]
fn source_captured_in_subst() {
    let (o, _e, _c) = run("printf 'echo S\\n' > /tmp/v132src.sh\nx=$(source /tmp/v132src.sh); echo \"[$x]\"\n");
    assert_eq!(o, "[S]\n", "o: {o:?}");
}
#[test]
fn eval_top_level_prints() {
    let (o, _e, _c) = run("eval 'echo top'\n");
    assert_eq!(o, "top\n", "o: {o:?}");
}
#[test]
fn command_eval_captured() {
    let (o, _e, _c) = run("x=$(command eval 'echo c'); echo \"[$x]\"\n");
    assert_eq!(o, "[c]\n", "o: {o:?}");
}
#[test]
fn function_named_eval_shadows() {
    let (o, _e, _c) = run("eval() { echo fn; }\neval x\n");
    assert_eq!(o, "fn\n", "o: {o:?}");
}
```

- [ ] **Step 2: Run to verify failures** — `cargo test --test eval_source_sink_integration 2>&1 | tail -20`. Expected: the capture tests (`eval_captured_in_subst`, `eval_multi_command_captured`, `eval_pipe_inside_capture`, `source_captured_in_subst`, `command_eval_captured`) FAIL (leak → `[]`); `eval_top_level_prints` and `function_named_eval_shadows` PASS.

- [ ] **Step 3: Add the dispatch arm.** In `run_exec_single` (src/executor.rs), insert BETWEEN the function-call branch (ends ~3091 with `call_function(&name, body, args, shell, sink)`) and the generic `} else if builtins::is_builtin(&resolved.program) {` branch (~3092):
```rust
    } else if resolved.program == "eval" {
        let args = resolved.args;
        if cmd.stdin.is_some() || cmd.stdout.is_some() || cmd.stderr.is_some() {
            with_redirect_scope(&cmd.stdin, &cmd.stdout, &cmd.stderr, shell, sink,
                move |shell, inner_sink| builtins::eval_in_sink(&args, shell, inner_sink))
        } else {
            builtins::eval_in_sink(&args, shell, sink)
        }
    } else if resolved.program == "source" || resolved.program == "." {
        let args = resolved.args;
        if cmd.stdin.is_some() || cmd.stdout.is_some() || cmd.stderr.is_some() {
            with_redirect_scope(&cmd.stdin, &cmd.stdout, &cmd.stderr, shell, sink,
                move |shell, inner_sink| builtins::source_in_sink(&args, shell, inner_sink))
        } else {
            builtins::source_in_sink(&args, shell, sink)
        }
    }
```
Borrow note: `let args = resolved.args;` moves the `Vec<String>` out of `resolved`; both if/else arms consume it (only one runs) — mirrors the function-call branch's `let args = resolved.args;`. If the borrow checker objects to `resolved` being partially-moved later, check that `resolved` is not used after this arm (it isn't in the function branch). The `xtrace` block + inline-assignment apply already ran ABOVE this dispatch (so eval/source still trace + see inline assignments — verify the arm is placed after those, same as the function branch).

- [ ] **Step 4: Run the integration tests** — `cargo test --test eval_source_sink_integration 2>&1 | tail -20`. Expected: all 7 pass.

- [ ] **Step 5: Build + FULL regression + clippy** — `cargo build 2>&1 | tail -3`; `cargo test 2>&1 | grep -E "FAILED|error\[|panicked|test result: FAILED" | head` (none — existing source/eval/trap/command/redirect tests stay green); `cargo clippy --all-targets 2>&1 | tail -3` (clean).

- [ ] **Step 6: Sanity vs bash** (report):
```
for f in "x=\$(eval 'echo hi'); echo [\$x]" "eval 'echo R' > /tmp/v132r; cat /tmp/v132r" "x=\$(source /tmp/v132src.sh); echo [\$x]"; do
  printf 'echo S\n' > /tmp/v132src.sh
  b=$(bash -c "$f" 2>&1); h=$(./target/debug/huck -c "$f" 2>&1)
  [ "$b" = "$h" ] && echo "MATCH: $f" || { echo "DIFF: $f"; diff <(echo "$b") <(echo "$h"); }
done; rm -f /tmp/v132r /tmp/v132src.sh
```

- [ ] **Step 7: Commit**
```bash
git add src/executor.rs tests/eval_source_sink_integration.rs
git commit -m "$(cat <<'EOF'
fix(v132): run eval/source with the enclosing sink (capture + redirect)

Add an eval/source dispatch arm in run_exec_single mirroring the function-call
branch: with_redirect_scope when a redirect is present (honors `eval cmd >file`),
else thread the current sink. Inside $() the eval'd/sourced output is now captured
(was leaking to the terminal), and an eval'd external no longer re-enters
interactive job control — fixing the nvm ls-remote hang.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: PTY hang regression + bash-diff harness + docs/payoff

**Files:**
- Create: `tests/eval_source_hang_pty.rs`
- Create: `tests/scripts/eval_source_sink_diff_check.sh`
- Modify: `docs/bash-divergences.md` (only if a residual is found)

- [ ] **Step 1: PTY hang regression** — create `tests/eval_source_hang_pty.rs`, mirroring `tests/subshell_tty_pty.rs` (expectrl `OsSession`; skip gracefully if no PTY). Tests:
  - spawn huck interactively; send `x=$(eval 'seq 1 500000'); echo "L=${#x} EVDONE"`; assert `EVDONE` arrives within the timeout (no hang) and the captured length token `L=` is followed by a non-zero number.
  - send `x=$(eval 'seq 1 200000' | wc -l); echo "W=$x WDONE"`; assert `WDONE` arrives.
  Read `tests/subshell_tty_pty.rs` for the exact expectrl spawn/timeout/skip idiom and copy it. Use a generous timeout (e.g. 10s) and a sentinel.

- [ ] **Step 2: Run the PTY test** — `cargo test --test eval_source_hang_pty 2>&1 | tail -10`. Expected: pass (or skip without a PTY). Before the fix this would hang; confirm it now completes.

- [ ] **Step 3: Bash-diff harness** — create `tests/scripts/eval_source_sink_diff_check.sh`:
```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v132: eval/source run with the enclosing
# sink (capture + redirect). Fragments run as FILE-ARGS via -c.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
FIX="$(mktemp -d)"; trap 'rm -rf "$FIX"' EXIT
printf 'echo SOURCED\n' > "$FIX/s.sh"
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
check "eval capture"        'x=$(eval "echo hi"); echo "[$x]"'
check "eval multi"          'x=$(eval "echo a; echo b"); echo "[$x]"'
check "eval pipe"           'x=$(eval "seq 1 50 | wc -l"); echo "[$x]"'
check "eval redirect"       'eval "echo R" > '"$FIX"'/r; cat '"$FIX"'/r'
check "eval stderr redir"   'eval "echo E 1>&2" 2> '"$FIX"'/e; cat '"$FIX"'/e'
check "eval top level"      'eval "echo top"'
check "source capture"      'x=$(source '"$FIX"'/s.sh); echo "[$x]"'
check "source top level"    'source '"$FIX"'/s.sh'
check "command eval"        'x=$(command eval "echo c"); echo "[$x]"'
check "nested eval capture" 'x=$(eval "eval \"echo deep\""); echo "[$x]"'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```
`chmod +x tests/scripts/eval_source_sink_diff_check.sh`.

- [ ] **Step 4: Run the harness** — `cargo build 2>&1 | tail -2`; `bash tests/scripts/eval_source_sink_diff_check.sh`. Expected `Fail: 0`. If a case fails, report the diff (do not mask) — it may reveal a residual to document.

- [ ] **Step 5: nvm payoff (best-effort, needs network).** In a PTY (reuse the harness from Step 1 or a quick python `pty.fork`), source `~/.nvm/nvm.sh` (NOT `~/.bashrc` — creds) and run `nvm ls-remote` with a timeout; confirm it no longer hangs and lists versions. If no network, state so and rely on the synthetic PTY regression (Step 2). Report what you observed.

- [ ] **Step 6: Docs.** No open M-/L- divergence entry exists for this bug. If Steps 4-5 surfaced a residual (e.g. a specific eval/source-in-capture edge that still diverges), add a `[deferred]` Tier-4 entry for it and bump the Tier-4 count. Otherwise no `bash-divergences.md` change (the iteration is recorded in history/memory at merge). Note the pre-existing L-25 (`$(builtin 2>&1)` capture) is unchanged.

- [ ] **Step 7: Full regression + clippy** — `cargo test 2>&1 | grep -E "FAILED|error\[|test result: FAILED" | head` (none); `cargo clippy --all-targets 2>&1 | tail -3` (clean); smoke an existing source/eval harness if present.

- [ ] **Step 8: Commit**
```bash
git add tests/eval_source_hang_pty.rs tests/scripts/eval_source_sink_diff_check.sh docs/bash-divergences.md
git commit -m "$(cat <<'EOF'
test(v132): PTY hang regression + bash-diff harness for eval/source sink

PTY regression proving x=$(eval 'seq 1 500000') completes (was the nvm ls-remote
hang class); bash-diff harness for the capture + redirect cases.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```
(Drop `docs/bash-divergences.md` from the `git add` if Step 6 made no doc change.)

---

## Self-review notes
- **Spec coverage:** Task 1 = Components 1-4 + 6 (the behavior-preserving refactor + delegation); Task 2 = Component 5 (the dispatch arm — the actual fix) + capture/redirect/shadow integration tests; Task 3 = PTY hang regression + harness + payoff + docs.
- **Type/symbol consistency:** `execute_with_sink`/`process_line_in_sink`/`run_sourced_contents_in_sink`/`eval_in_sink`/`source_in_sink` all take `&mut StdoutSink`; defined in Task 1, called by the Task 2 dispatch arm. `with_redirect_scope` (executor.rs:509) reused unchanged. `StdoutSink` is `pub`.
- **No-regress is the Task 1 gate** (pure refactor → full suite green before any behavior change). Task 2's failing-then-passing tests prove the fix; the full suite re-run in Task 2 Step 5 proves nothing else broke.
- **Borrow caveat:** the eval/source arm's `let args = resolved.args;` mirrors the function-call branch verbatim; if `resolved` is referenced after the arm, scope `args` per-branch instead.
