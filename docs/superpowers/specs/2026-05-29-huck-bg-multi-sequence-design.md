# huck v49 — Backgrounded multi-pipeline sequences (M-64)

## Goal

Allow `cmd1 && cmd2 &`, `cmd1 ; cmd2 &`, `cmd1 || cmd2 &`, and longer
chains to parse and run. Currently huck rejects these with a parse
error `BackgroundedMultiPipelineSequence`. Semantics match bash:
fork once, child runs the entire sequence to completion, parent
registers a single-PID job.

This is a new tracked divergence: **M-64: Backgrounded multi-pipeline
sequences**, added to `docs/bash-divergences.md`.

## Scope decisions (locked)

This is a tight, small-surface change. No scope questions surfaced
worth asking. The design matches bash exactly:

1. `cmd1 && cmd2 &` ≡ `(cmd1 && cmd2) &` (same fork-once-and-run
   semantics).
2. Job-table command text is the original source line (existing
   `display_command(source)` behavior).
3. Single-PID job from parent's perspective (the child PID).

## Out of scope (deferred)

- Job-control signal propagation nuances beyond what
  `run_background_subshell` already does (existing infrastructure is
  reused).
- New `wait`/`jobs`/`disown` semantics — they already work because
  the bg sequence registers as a single-PID job indistinguishable
  from `(cmd) &`.

## Architecture

Two-file change. Parser unblocks the previously-rejected shape;
executor routes the new shape through the existing subshell-bg
machinery.

### Parser change

In `src/command.rs` around line 525-543, the `Token::Op(Operator::Background)` arm currently rejects when `rest` is non-empty:

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

Remove the `!rest.is_empty()` check. The variant
`ParseError::BackgroundedMultiPipelineSequence` stays in the enum
(removing it would require touching the error-message arm in
`src/shell.rs::parse_error_message`); it becomes unused at runtime
but the variant remains for backwards-compatibility of the enum
shape.

Actually, since the variant only fires from these now-removed sites,
and no external code depends on the variant existing, we should
remove it entirely to keep the enum clean. See "ParseError enum
cleanup" below.

### ParseError enum cleanup

After removing the rejection, the `BackgroundedMultiPipelineSequence`
variant is unreferenced. Remove:

- The variant from `ParseError` in `src/command.rs:432`.
- The arm in `parse_error_message` in `src/shell.rs:276` (which
  produces the user-facing error message).

The removal will surface any other references — the parser-test
suite has 4 tests around lines 2189, 2203, 2210, 2219 that assert
the error. These tests need to be rewritten to assert successful
parse with `background: true` and non-empty `rest`.

### Executor change

In `src/executor.rs::execute` (lines 22-34), the current dispatch is:

```rust
pub fn execute(seq: &Sequence, shell: &mut Shell, source: &str) -> ExecOutcome {
    let mut sink = StdoutSink::Terminal;
    if seq.background {
        if let Command::Pipeline(p) = &seq.first {
            return run_background_sequence(p, shell, &mut sink, source);
        }
        if let Command::Subshell { .. } = &seq.first {
            return run_background_subshell(&seq.first, shell, &mut sink, source);
        }
    }
    execute_sequence_body(seq, shell, &mut sink)
}
```

The existing two arms handle single-pipeline and subshell. For v49
we add a third path for multi-element backgrounded sequences:

```rust
pub fn execute(seq: &Sequence, shell: &mut Shell, source: &str) -> ExecOutcome {
    let mut sink = StdoutSink::Terminal;
    if seq.background {
        if seq.rest.is_empty() {
            if let Command::Pipeline(p) = &seq.first {
                return run_background_sequence(p, shell, &mut sink, source);
            }
            if let Command::Subshell { .. } = &seq.first {
                return run_background_subshell(&seq.first, shell, &mut sink, source);
            }
        } else {
            // v49: multi-pipeline sequence backgrounded. Synthesize
            // (seq) & by wrapping in a Subshell with background=false.
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

The wrapped sequence has `background: false` because the child runs
foreground inside its own process. The parent reaches
`run_background_subshell` which forks and registers the child PID.

### Cloning cost

The wrap requires cloning `seq.first` and `seq.rest`. `Command` and
`Connector` both derive `Clone`. For multi-element sequences this is
typically a handful of `SimpleCommand`/`Pipeline` clones — negligible
cost compared to the fork that follows.

## Edge cases

| Input | Behavior |
|---|---|
| `cmd1 && cmd2 &` | forks once; child runs `cmd1 && cmd2`; parent registers PID; `[N] PID` print |
| `cmd1 ; cmd2 &` | forks once; child runs the semicolon sequence |
| `cmd1 \|\| cmd2 &` | forks once; child runs the `\|\|` chain |
| `cmd1 && cmd2 && cmd3 &` | works for arbitrary chain length |
| `(cmd1 && cmd2) &` | unchanged (already worked via subshell-bg) |
| `cmd1 && cmd2` (no bg) | unchanged foreground sequence |
| `cmd &` (single pipeline) | unchanged single-pipeline-bg path |
| `cmd1 && (cmd2 ; cmd3) &` | works — first is pipeline, rest has subshell |

## Test plan

### Parser unit tests in `src/command.rs#[cfg(test)] mod tests`

Replace the 4 existing tests that assert the rejection (currently at
lines ~2189, 2203, 2210, 2219). The implementer needs to locate them
by their content (each constructs a `cmd1 && cmd2 &`-style input and
asserts `Err(ParseError::BackgroundedMultiPipelineSequence)`).

Repurpose each as a successful-parse test:

```rust
#[test]
fn parse_and_then_bg_is_backgrounded_sequence() {
    let toks = tokenize("true && true &").unwrap();
    let seq = parse(toks).unwrap().unwrap();
    assert!(seq.background, "expected background=true");
    assert_eq!(seq.rest.len(), 1, "expected one connector entry");
}

#[test]
fn parse_semi_chain_bg_is_backgrounded_sequence() {
    let toks = tokenize("true ; true &").unwrap();
    let seq = parse(toks).unwrap().unwrap();
    assert!(seq.background);
    assert_eq!(seq.rest.len(), 1);
}

#[test]
fn parse_or_chain_bg_is_backgrounded_sequence() {
    let toks = tokenize("true || true &").unwrap();
    let seq = parse(toks).unwrap().unwrap();
    assert!(seq.background);
    assert_eq!(seq.rest.len(), 1);
}

#[test]
fn parse_long_chain_bg() {
    let toks = tokenize("true && true || true ; true &").unwrap();
    let seq = parse(toks).unwrap().unwrap();
    assert!(seq.background);
    assert_eq!(seq.rest.len(), 3);
}
```

### Executor unit tests in `src/executor.rs#[cfg(test)] mod tests`

Use the existing `exec_script` helper pattern:

```rust
#[test]
fn execute_bg_chain_returns_immediately_status_0() {
    let mut shell = Shell::new();
    let outcome = exec_script("true && true &", &mut shell);
    // Parent should return Continue(0) immediately — child runs in
    // background and parent's status is 0 (bash convention).
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
}

#[test]
fn execute_bg_chain_registers_job() {
    let mut shell = Shell::new();
    let _ = exec_script("sleep 30 && true &", &mut shell);
    // The bg sequence should appear in the job table as one entry.
    assert_eq!(shell.jobs.iter().count(), 1);
    // Cleanup: kill the bg sleep so the test doesn't leave a zombie.
    if let Some(job) = shell.jobs.iter().next() {
        unsafe { libc::kill(job.pgid, libc::SIGTERM); }
    }
}
```

### Integration tests at `tests/bg_sequence_integration.rs`

3 binary-driven tests:

1. `bg_and_chain_runs_to_completion` — script:
   `echo A && echo B & wait\nexit\n`. Stdout contains `A` and `B`
   lines. Both run because `&&` short-circuit is satisfied.

2. `bg_semi_chain_runs_both` — script:
   `echo X ; echo Y & wait\nexit\n`. Stdout contains `X` and `Y`.

3. `bg_chain_short_circuits` — script:
   `false && echo SKIP & wait\necho DONE\nexit\n`. Stdout contains
   `DONE` but NOT `SKIP` (the `false &&` short-circuits).

The `wait` is required so the parent waits for the bg sequence to
complete before printing further output (otherwise the test would
race).

### Smoke

`cargo test --all-targets` must pass. PTY flake tolerated.

## Implementation tasks

1. **Parser + executor + unit tests**:
   - Remove `!rest.is_empty()` rejection from `src/command.rs`.
   - Remove `ParseError::BackgroundedMultiPipelineSequence` variant.
   - Remove its arm in `src/shell.rs::parse_error_message`.
   - Rewrite 4 parser tests to assert successful parse.
   - Add new arm in `src/executor.rs::execute` for the multi-element
     backgrounded case (Subshell synthesis).
   - Add 2 executor unit tests.

2. **Integration tests**: create
   `tests/bg_sequence_integration.rs` with the 3 scenarios.

3. **Docs**:
   - Add **M-64: Backgrounded multi-pipeline sequences** entry to
     `docs/bash-divergences.md` as `[fixed v49]`.
   - Change-log entry.
   - README v49 row.
   - Remove `backgrounded multi-pipeline sequences (\`cmd1 && cmd2
     &\`)` from "Not yet implemented" stanza.

Three tasks. TDD per task.

## Acceptance criteria

- 4 parser tests + 2 executor unit tests pass.
- All 3 integration tests pass.
- All pre-existing tests still pass.
- `cargo test --all-targets` passes (modulo PTY flake).
- `cargo clippy --all-targets -- -D warnings` passes.
- `docs/bash-divergences.md` has the new M-64 entry as
  `[fixed v49]`.
- `cmd1 && cmd2 &` parses and runs; the job appears in `jobs` as a
  single-PID entry.
- The bg sequence honors `&&`/`||` short-circuit semantics inside
  the child.
- `wait %1` on a bg-sequence job works (inherited from existing
  subshell-bg path).
