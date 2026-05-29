# huck v49 — Backgrounded Multi-Pipeline Sequences Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Allow `cmd1 && cmd2 &`, `cmd1 ; cmd2 &`, `cmd1 || cmd2 &`, and
longer chains to parse and execute as a single backgrounded job
(bash-faithful: fork once, child runs the whole sequence, parent
registers a single-PID job).

**Architecture:** Two-file change. Parser unblocks the
previously-rejected shape (one-line removal + enum cleanup). Executor
adds a third arm in `execute()` for the multi-element backgrounded
case that synthesizes `Command::Subshell { body: <seq> }` and reuses
the existing `run_background_subshell` infrastructure. Four broken
parser tests are rewritten to assert successful parse.

**Tech Stack:** Rust. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-05-29-huck-bg-multi-sequence-design.md`

**Branch:** `v49-bg-multi-sequence` (created in preamble step P.1).

**Commit trailer convention**:

```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Preamble: Create the working branch

- [ ] **Step P.1: Create branch from main and check it out**

```bash
git checkout main
git pull --ff-only
git checkout -b v49-bg-multi-sequence
```

Expected: `Switched to a new branch 'v49-bg-multi-sequence'`.

The spec + this plan are committed as the first commit on this branch
(handled by the controller before Task 1 begins).

---

## Task 1: Parser + executor + unit tests

**Files:**
- Modify: `src/command.rs` — remove the rejection at line 530; remove
  the `BackgroundedMultiPipelineSequence` enum variant at line 432;
  rewrite 3 broken parser tests (around lines 2179, 2194, 2210).
- Modify: `src/shell.rs` — remove the
  `BackgroundedMultiPipelineSequence` arm in `parse_error_message`
  at line 276.
- Modify: `src/executor.rs` — add the new dispatch arm in `execute`
  for `seq.background && !seq.rest.is_empty()`; add 2 executor unit
  tests.

### Step 1.1: Remove the rejection in `src/command.rs`

In `src/command.rs`, find the `Token::Op(Operator::Background)` arm
inside the main parsing loop. It's around line 525-543:

```rust
Token::Op(Operator::Background) => {
    if !at_top_level {
        return Err(ParseError::UnexpectedBackground);
    }
    if !rest.is_empty() {
        return Err(ParseError::BackgroundedMultiPipelineSequence);
    }
    skip_newlines(iter);
    if iter.peek().is_some() {
        return Err(ParseError::UnexpectedBackground);
    }
    background = true;
    break;
}
```

Remove the 3-line `if !rest.is_empty() { return Err(...); }` block. The arm becomes:

```rust
Token::Op(Operator::Background) => {
    if !at_top_level {
        return Err(ParseError::UnexpectedBackground);
    }
    skip_newlines(iter);
    if iter.peek().is_some() {
        return Err(ParseError::UnexpectedBackground);
    }
    background = true;
    break;
}
```

- [ ] **Step 1.1: Remove the rejection block**

### Step 1.2: Remove the `BackgroundedMultiPipelineSequence` variant

In `src/command.rs:432`, the `ParseError` enum currently includes:

```rust
#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    MissingCommand,
    MissingRedirectTarget,
    RedirectTargetIsOperator,
    UnexpectedBackground,
    BackgroundedMultiPipelineSequence,
    UnterminatedIf,
    // ...
}
```

Delete the `BackgroundedMultiPipelineSequence,` line:

```rust
#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    MissingCommand,
    MissingRedirectTarget,
    RedirectTargetIsOperator,
    UnexpectedBackground,
    UnterminatedIf,
    // ...
}
```

- [ ] **Step 1.2: Remove the enum variant**

### Step 1.3: Remove the arm in `parse_error_message`

In `src/shell.rs`, find `fn parse_error_message` (around line 257).
The current code around line 276 has:

```rust
        ParseError::BackgroundedMultiPipelineSequence => {
            "'&' on multi-command sequence not supported; use a single pipeline".to_string()
        }
```

Delete this arm (the `ParseError::` keyword through the closing `}`).

- [ ] **Step 1.3: Remove parse_error_message arm**

### Step 1.4: Build to confirm the enum cleanup

Run: `cargo build`
Expected: build fails at the 3 parser test sites that reference the
deleted variant. Proceed to step 1.5 to fix them.

- [ ] **Step 1.4: Confirm expected build failures**

### Step 1.5: Rewrite the 3 broken parser tests

In `src/command.rs`, find these 3 tests (currently around lines
2179-2222 — find by searching for `BackgroundedMultiPipelineSequence`):

Old test 1 (`parse_background_after_andor_is_unsupported`):

```rust
    #[test]
    fn parse_background_after_andor_is_unsupported() {
        // cmd1 && cmd2 &
        assert_eq!(
            parse(vec![
                w_tok("cmd1"),
                Token::Op(Operator::And),
                w_tok("cmd2"),
                Token::Op(Operator::Background),
            ]),
            Err(ParseError::BackgroundedMultiPipelineSequence)
        );
    }
```

Replace with:

```rust
    #[test]
    fn parse_and_then_bg_is_backgrounded_sequence() {
        // cmd1 && cmd2 &
        let seq = parse(vec![
            w_tok("cmd1"),
            Token::Op(Operator::And),
            w_tok("cmd2"),
            Token::Op(Operator::Background),
        ])
        .unwrap()
        .unwrap();
        assert!(seq.background, "expected background=true");
        assert_eq!(seq.rest.len(), 1, "expected one connector entry");
    }
```

Old test 2 (`parse_background_after_semi_is_unsupported`):

```rust
    #[test]
    fn parse_background_after_semi_is_unsupported() {
        // cmd1 ; cmd2 &
        assert_eq!(
            parse(vec![
                w_tok("cmd1"),
                Token::Op(Operator::Semi),
                w_tok("cmd2"),
                Token::Op(Operator::Background),
            ]),
            Err(ParseError::BackgroundedMultiPipelineSequence)
        );
    }
```

Replace with:

```rust
    #[test]
    fn parse_semi_chain_bg_is_backgrounded_sequence() {
        // cmd1 ; cmd2 &
        let seq = parse(vec![
            w_tok("cmd1"),
            Token::Op(Operator::Semi),
            w_tok("cmd2"),
            Token::Op(Operator::Background),
        ])
        .unwrap()
        .unwrap();
        assert!(seq.background, "expected background=true");
        assert_eq!(seq.rest.len(), 1);
    }
```

Old test 3 (`parse_background_mid_sequence_after_andor_prefers_multipipeline_error`):

```rust
    #[test]
    fn parse_background_mid_sequence_after_andor_prefers_multipipeline_error() {
        // cmd1 && cmd2 & cmd3 — both errors apply; the more specific
        // BackgroundedMultiPipelineSequence wins.
        assert_eq!(
            parse(vec![
                w_tok("cmd1"),
                Token::Op(Operator::And),
                w_tok("cmd2"),
                Token::Op(Operator::Background),
                w_tok("cmd3"),
            ]),
            Err(ParseError::BackgroundedMultiPipelineSequence)
        );
    }
```

Replace with:

```rust
    #[test]
    fn parse_background_mid_sequence_after_andor_is_unexpected_background() {
        // cmd1 && cmd2 & cmd3 — the `&` is followed by another token,
        // which is now the only error (UnexpectedBackground). Previously
        // BackgroundedMultiPipelineSequence won; with that variant
        // removed, UnexpectedBackground is the appropriate error.
        assert_eq!(
            parse(vec![
                w_tok("cmd1"),
                Token::Op(Operator::And),
                w_tok("cmd2"),
                Token::Op(Operator::Background),
                w_tok("cmd3"),
            ]),
            Err(ParseError::UnexpectedBackground)
        );
    }
```

(Note: the spec called for 4 broken tests, but inspection shows only 3 distinct test functions. The "lines 2210/2219" range covers the `parse_background_mid_sequence_after_andor_prefers_multipipeline_error` test which spans those line numbers via its body.)

- [ ] **Step 1.5: Rewrite 3 broken parser tests**

### Step 1.6: Add a 4th new parser test for long chains

While the spec called for 4 tests, the existing test set yields 3 rewrites. Add a 4th NEW test below the rewrites that covers a longer chain:

```rust
    #[test]
    fn parse_long_chain_bg() {
        // cmd1 && cmd2 || cmd3 ; cmd4 &
        let seq = parse(vec![
            w_tok("cmd1"),
            Token::Op(Operator::And),
            w_tok("cmd2"),
            Token::Op(Operator::Or),
            w_tok("cmd3"),
            Token::Op(Operator::Semi),
            w_tok("cmd4"),
            Token::Op(Operator::Background),
        ])
        .unwrap()
        .unwrap();
        assert!(seq.background, "expected background=true");
        assert_eq!(seq.rest.len(), 3, "expected 3 connector entries");
    }
```

- [ ] **Step 1.6: Add long-chain parser test**

### Step 1.7: Build + run parser tests

Run: `cargo build`
Expected: clean.

Run: `cargo test --bin huck command::tests::parse_and_then_bg command::tests::parse_semi_chain_bg command::tests::parse_background_mid_sequence command::tests::parse_long_chain_bg`
Expected: 4 tests pass.

- [ ] **Step 1.7: 4 parser tests pass**

### Step 1.8: Add the new executor arm in `src/executor.rs::execute`

Find `pub fn execute` at `src/executor.rs:22`. The current body is:

```rust
pub fn execute(seq: &Sequence, shell: &mut Shell, source: &str) -> ExecOutcome {
    let mut sink = StdoutSink::Terminal;
    if seq.background {
        if let Command::Pipeline(p) = &seq.first {
            // Parser guarantees rest.is_empty() when background is set.
            return run_background_sequence(p, shell, &mut sink, source);
        }
        if let Command::Subshell { .. } = &seq.first {
            return run_background_subshell(&seq.first, shell, &mut sink, source);
        }
    }
    execute_sequence_body(seq, shell, &mut sink)
}
```

Replace with:

```rust
pub fn execute(seq: &Sequence, shell: &mut Shell, source: &str) -> ExecOutcome {
    let mut sink = StdoutSink::Terminal;
    if seq.background {
        if seq.rest.is_empty() {
            // Single-pipeline or subshell backgrounded — existing paths.
            if let Command::Pipeline(p) = &seq.first {
                return run_background_sequence(p, shell, &mut sink, source);
            }
            if let Command::Subshell { .. } = &seq.first {
                return run_background_subshell(&seq.first, shell, &mut sink, source);
            }
        } else {
            // v49: multi-pipeline sequence backgrounded. Synthesize
            // (seq) & by wrapping the whole sequence in a Subshell.
            // The wrapped sequence has background=false because the
            // child process runs it foreground inside its own pid.
            let inner = Sequence {
                first: seq.first.clone(),
                rest: seq.rest.clone(),
                background: false,
            };
            let subshell = Command::Subshell { body: Box::new(inner) };
            return run_background_subshell(&subshell, shell, &mut sink, source);
        }
    }
    execute_sequence_body(seq, shell, &mut sink)
}
```

- [ ] **Step 1.8: Update `execute()`**

### Step 1.9: Build to confirm

Run: `cargo build`
Expected: clean. `Command::Subshell { body: Box<Sequence> }` exists at
`src/command.rs:354`; `Sequence` derives `Clone`.

If the build fails on `Sequence` not being `Clone`: it's derived
already per `src/command.rs:419`. The `Command` enum also derives
`Clone`. If a variant doesn't, the implementer needs to investigate —
report BLOCKED.

- [ ] **Step 1.9: Build clean**

### Step 1.10: Add 2 executor unit tests

In `src/executor.rs`, find `#[cfg(test)] mod tests` (search for that
marker). At the end of the mod block (before the closing `}`), add:

```rust
    #[test]
    fn execute_bg_chain_returns_immediately_status_0() {
        // `true && true &` — parent should return Continue(0) without
        // waiting for the child.
        use crate::shell_state::Shell;
        let mut shell = Shell::new();
        let toks = crate::lexer::tokenize("true && true &").unwrap();
        let seq = crate::command::parse(toks).unwrap().unwrap();
        let outcome = execute(&seq, &mut shell, "true && true &");
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        // Cleanup: SIGTERM any bg job so the test doesn't leak.
        for job in shell.jobs.iter() {
            unsafe { libc::kill(job.pgid, libc::SIGTERM); }
        }
    }

    #[test]
    fn execute_bg_chain_registers_job() {
        // After `sleep 30 && true &`, the bg sequence should register
        // as one job entry. The sleep ensures the child is alive long
        // enough to observe.
        use crate::shell_state::Shell;
        let mut shell = Shell::new();
        let toks = crate::lexer::tokenize("sleep 30 && true &").unwrap();
        let seq = crate::command::parse(toks).unwrap().unwrap();
        let _ = execute(&seq, &mut shell, "sleep 30 && true &");
        assert_eq!(shell.jobs.iter().count(), 1, "expected exactly one job");
        // Cleanup.
        for job in shell.jobs.iter() {
            unsafe { libc::kill(job.pgid, libc::SIGTERM); }
        }
    }
```

- [ ] **Step 1.10: Add 2 executor tests**

### Step 1.11: Run the new tests

Run: `cargo test --bin huck execute_bg_chain -- --nocapture`
Expected: both tests pass.

If `execute_bg_chain_registers_job` fails because the child finishes before the parent observes: bump the sleep to 60.

- [ ] **Step 1.11: 2 executor tests pass**

### Step 1.12: Full unit suite

Run: `cargo test --bin huck`
Expected: all unit tests pass. If a pre-existing test fails because
it depended on `BackgroundedMultiPipelineSequence` existing: that's
a regression — the rewrites in steps 1.5/1.6 should cover the 3
direct references. Anything else (rare): inspect and fix.

- [ ] **Step 1.12: Full unit suite passes**

### Step 1.13: Clippy

Run: `cargo clippy --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 1.13: Clippy clean**

### Step 1.14: Commit

```bash
git add src/command.rs src/shell.rs src/executor.rs
git commit -m "$(cat <<'EOF'
parse+exec: backgrounded multi-pipeline sequences (v49 task 1)

Allow `cmd1 && cmd2 &`, `cmd1 ; cmd2 &`, `cmd1 || cmd2 &`, and
longer chains to parse and run. Match bash semantics: fork once,
child runs the entire sequence to completion, parent registers a
single-PID job and returns immediately.

Parser:
- Remove the !rest.is_empty() rejection in src/command.rs that
  forced these inputs to error.
- Drop the unused ParseError::BackgroundedMultiPipelineSequence
  variant and its arm in src/shell.rs::parse_error_message.

Executor:
- New arm in execute() for seq.background && !seq.rest.is_empty().
  Synthesizes Command::Subshell { body: Box::new(<seq>) } with
  background=false on the inner Sequence (the child runs foreground
  inside its own pid), then dispatches to the existing
  run_background_subshell which already handles fork + JobTable
  registration + [N] PID notice.

3 pre-existing parser tests asserting the rejection are rewritten
to assert successful parse with background=true and non-empty
rest. A 4th new test covers a longer chain (`cmd1 && cmd2 || cmd3
; cmd4 &`). 2 executor unit tests verify the parent returns
Continue(0) immediately and the bg sequence registers as exactly
one job.

The pre-existing parse_background_mid_sequence_after_andor test
now expects UnexpectedBackground (the only remaining error when
`&` appears mid-input).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 1.14: Commit Task 1**

---

## Task 2: Integration tests

**Files:**
- Create: `tests/bg_sequence_integration.rs`

Three binary-driven tests.

### Step 2.1: Create the integration test file

Create `tests/bg_sequence_integration.rs` with this content:

```rust
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run_capture(script: &str) -> (String, String) {
    let mut child = Command::new(huck_binary())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn bg_and_chain_runs_to_completion() {
    // `echo A && echo B &` then `wait` so the test sees both lines
    // before huck exits.
    let script = "echo A && echo B &\nwait\nexit\n";
    let (out, _) = run_capture(script);
    assert!(
        out.lines().any(|l| l == "A"),
        "expected line A in: {:?}",
        out
    );
    assert!(
        out.lines().any(|l| l == "B"),
        "expected line B in: {:?}",
        out
    );
}

#[test]
fn bg_semi_chain_runs_both() {
    let script = "echo X ; echo Y &\nwait\nexit\n";
    let (out, _) = run_capture(script);
    assert!(
        out.lines().any(|l| l == "X"),
        "expected line X in: {:?}",
        out
    );
    assert!(
        out.lines().any(|l| l == "Y"),
        "expected line Y in: {:?}",
        out
    );
}

#[test]
fn bg_chain_short_circuits() {
    // `false && echo SKIP &` — the bg sequence's && short-circuits
    // so SKIP is NEVER printed. Use `wait` then a foreground echo to
    // verify ordering.
    let script = "false && echo SKIP &\nwait\necho DONE\nexit\n";
    let (out, _) = run_capture(script);
    assert!(
        out.lines().any(|l| l == "DONE"),
        "expected DONE line in: {:?}",
        out
    );
    assert!(
        !out.lines().any(|l| l == "SKIP"),
        "SKIP should NOT appear (short-circuit): {:?}",
        out
    );
}
```

- [ ] **Step 2.1: Create the file**

### Step 2.2: Run the integration suite

Run: `cargo test --test bg_sequence_integration -- --nocapture`
Expected: all 3 tests pass.

If a test fails: inspect actual stdout via `--nocapture`. The most
likely failure: huck's [job number] PID notice (printed by
`run_background_subshell` to stderr — line 555) could contaminate
the stderr stream but shouldn't affect stdout. If stdout is missing
expected lines: report BLOCKED with the actual output.

- [ ] **Step 2.2: Tests pass**

### Step 2.3: Full integration suite

Run: `cargo test --tests`
Expected: all integration tests pass. PTY flake tolerated.

- [ ] **Step 2.3: Full integration suite green**

### Step 2.4: Clippy

Run: `cargo clippy --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 2.4: Clippy clean**

### Step 2.5: Commit

```bash
git add tests/bg_sequence_integration.rs
git commit -m "$(cat <<'EOF'
test: backgrounded multi-pipeline sequence coverage (v49 task 2)

Three binary-driven tests verifying that the new
multi-element-backgrounded path runs end-to-end through the huck
binary. bg_and_chain_runs_to_completion verifies that
`echo A && echo B &` runs both echos. bg_semi_chain_runs_both
verifies that `echo X ; echo Y &` runs both. bg_chain_short_circuits
verifies that `false && echo SKIP &` honors the `&&` short-circuit
inside the child (SKIP is NEVER printed).

Each test follows the bg sequence with `wait` so the parent observes
the child's output before exiting.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 2.5: Commit Task 2**

---

## Task 3: Docs

**Files:**
- Modify: `docs/bash-divergences.md` — add new M-64 entry,
  change-log entry.
- Modify: `README.md` — v49 row + trim "backgrounded multi-pipeline
  sequences" from "Not yet implemented" stanza.

### Step 3.1: Add M-64 entry in `docs/bash-divergences.md`

Backgrounded multi-pipeline sequences don't currently have a tracked
M-* entry. M-63 was claimed by v48 (aliases); M-64 is new for this
iteration.

Find the appropriate section. Job-control or "Compound commands"
subsection is the natural home. Search for `### Job control` or
similar. Add this entry alongside other job-control or
control-flow entries:

```markdown
- **M-64: Backgrounded multi-pipeline sequences (`cmd1 && cmd2 &`)** — `[fixed v49]` medium. Sequences using `&&`, `||`, `;`, or any combination can now end with `&` to background the whole sequence as a single job. Implementation forks once; child runs the entire sequence to completion (honoring `&&`/`||` short-circuit); parent registers a single-PID job and returns immediately. Equivalent to bash's `(cmd1 && cmd2) &` semantics. `jobs`, `wait %N`, `kill %N`, `disown %N` all work because the bg sequence registers as a single-PID job indistinguishable from `(cmd) &`.
```

- [ ] **Step 3.1: Add M-64 entry**

### Step 3.2: Add v49 change-log entry

In `docs/bash-divergences.md`, find `## Change log` and the most
recent `**2026-05-29**` entry (v48, M-63 aliases). Add IMMEDIATELY
after it:

```markdown
- **2026-05-29**: M-64 (backgrounded multi-pipeline sequences) shipped as v49. Two-file change. Parser unblocked: removed `!rest.is_empty()` rejection and the now-unused `ParseError::BackgroundedMultiPipelineSequence` variant. Executor extended: `execute()` gains a new arm for `seq.background && !seq.rest.is_empty()` that synthesizes `Command::Subshell { body: Box::new(<seq>) }` and dispatches to the existing `run_background_subshell`. Reuses fork + JobTable + `[N] PID` notice infrastructure. No new L-* divergences.
```

- [ ] **Step 3.2: Add change-log entry**

### Step 3.3: Add v49 row to README

In `README.md`, find the version table. After the v48 row (search
for `| v48       |`), add IMMEDIATELY after it:

```markdown
| v49       | Backgrounded multi-pipeline sequences (M-64)                   |
```

Match column padding to v47/v48 (count actual trailing spaces in
the file).

- [ ] **Step 3.3: Add README v49 row**

### Step 3.4: Trim "backgrounded multi-pipeline sequences" from "Not yet implemented"

In `README.md`, find the block around lines ~233-238. Post-v48
should read:

```markdown
**Not yet implemented:**
backgrounded multi-pipeline sequences (`cmd1 && cmd2 &`).
```

Replace with (removing the now-shipped item):

```markdown
**Not yet implemented:**
*(none — see [bash-divergences.md](docs/bash-divergences.md) for the
deferred items in `[deferred]` status)*
```

Or, if you'd prefer to delete the "Not yet implemented" block entirely
since it's now empty:

```markdown
```

(Delete the whole block plus the blank line above. Match whichever
form the existing repo prefers.)

- [ ] **Step 3.4: Trim README stanza**

### Step 3.5: Full suite

Run: `cargo test --all-targets`
Expected: all tests pass (modulo PTY flake).

- [ ] **Step 3.5: Full suite green**

### Step 3.6: Clippy

Run: `cargo clippy --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 3.6: Clippy clean**

### Step 3.7: Commit

```bash
git add docs/bash-divergences.md README.md
git commit -m "$(cat <<'EOF'
docs: add M-64 (bg multi-pipeline sequences) fixed v49

New M-64 entry in docs/bash-divergences.md tracks backgrounded
multi-pipeline sequences as [fixed v49]. Covers the bash-faithful
fork-once-and-run semantics, single-PID job registration, and
reuse of the existing run_background_subshell path.

Change log: 2026-05-29 v49 entry summarizing the parser unblock,
the dropped ParseError variant, and the new execute() arm.

README: v49 row added to the version table; "Not yet implemented"
stanza trimmed (the last item shipped this iteration).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 3.7: Commit Task 3**

---

## Final verification (controller, not a task)

After the three task commits land:

1. Run `cargo test --all-targets` once more.
2. Run `cargo clippy --all-targets -- -D warnings`.
3. Confirm the branch has exactly four commits ahead of `main`:
   docs preamble (spec + plan), task 1, task 2, task 3.
4. Dispatch a final cross-task code-reviewer subagent over the
   full diff (`main..v49-bg-multi-sequence`).
5. Merge to `main` with `--no-ff`, push, delete the branch, update
   the `huck iterations` memory with v49.
