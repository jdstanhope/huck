# shuck Job Control Implementation Plan (Sub-project A)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add trailing-`&` background execution to shuck, with a job table, `jobs` and `wait` builtins, SIGCHLD-driven reaping, and `[N] <state> <cmd> &` notifications before the next prompt.

**Architecture:** `Operator::Background` joins the lexer's token vocabulary. A new `background: bool` flag on `Sequence` carries the trailing-`&` decision through the parser. A new `src/jobs.rs` module owns the `JobTable` data structure, with a `reap_completed` function that loops `libc::waitpid(WNOHANG)` to drain finished children. The executor branches on `seq.background`: foreground stays unchanged; background spawns the pipeline with `Command::process_group` to put it in its own process group, redirects stdin from `/dev/null` if not explicit, registers a `Job`, and returns immediately. A SIGCHLD handler installed via `signal_hook::flag::register` toggles an `Arc<AtomicBool>` on the `Shell`; the REPL drains+notifies before every prompt.

**Tech Stack:** Rust (edition 2024). One new dependency: `libc = "0.2"`. Existing `signal-hook` gains a SIGCHLD registration.

**Spec:** `docs/superpowers/specs/2026-05-16-shuck-job-control-design.md`

---

## File Structure

| File | Change |
|------|--------|
| `Cargo.toml` | Add `libc = "0.2"`. |
| `src/lexer.rs` | `&` (not `&&`) → `Operator::Background`. Remove `LexError::BareAmpersand`. Update its two existing tests. Add a small new test. |
| `src/command.rs` | Add `background: bool` to `Sequence`. Add `ParseError::UnexpectedBackground` and `ParseError::BackgroundedMultiPipelineSequence`. Parser handles trailing-`&` (single-pipeline only). Update all `Sequence { ... }` literals in tests to include `background: false`. Add ~6 new parser tests. |
| `src/shell.rs` | `lex_error_message` loses `BareAmpersand` arm. `parse_error_message` gains the two new arms. Install SIGCHLD handler at startup (mirroring SIGINT install). Pass `&line` as `source` through `executor::execute`. REPL calls `jobs::reap_and_notify(&mut shell)` before each `readline`. |
| `src/shell_state.rs` | `Shell` gains `pub jobs: JobTable` and `pub sigchld_flag: Arc<AtomicBool>` fields. `Shell::new` initializes both. `Clone` propagates. |
| `src/jobs.rs` | **New.** `Job`, `JobState`, `JobTable`, `reap_completed`, `reap_and_notify`, status decoding via libc. ~10 unit tests. |
| `src/executor.rs` | `execute` and `execute_inner` gain a `source: &str` parameter. `execute_inner` branches on `seq.background`. New `run_background_sequence` handles the spawn path (`Command::process_group`, `Stdio::null()` stdin, register Job, print `[N] PID`) and the pure-builtin synchronous fast path (run synchronously + synthetic Done job). New unit test for the synchronous fast path. |
| `src/builtins.rs` | New `builtin_jobs` (lists table to `out`) and `builtin_wait` (no-args form). `is_builtin` recognizes `"jobs"` and `"wait"`. `run_builtin` dispatches. Unit tests. |
| `src/main.rs` | Add `mod jobs;`. |

**Why the task order:** Task 1 lands the syntax (lexer + parser) — `cmd &` parses but the executor ignores the flag (still runs foreground). Task 2 builds the standalone `JobTable` module — unit-tested in isolation, no integration yet. Task 3 wires `JobTable`, the SIGCHLD flag, and the `source` parameter through `Shell` and the executor's public API — still no behavior change because nothing reads the flag yet. Task 4 implements `run_background_sequence` so `cmd &` actually backgrounds (jobs appear in the table). Task 5 adds the SIGCHLD reap + prompt notifications so users see Done lines. Task 6 adds the `jobs` and `wait` builtins for inspection and synchronization. Task 7 is the comprehensive smoke test. Per-task verification: `cargo test`. Strict gate every task: **0 failed**, **0 warnings**.

---

## Task 1: Syntax — `Operator::Background` + `Sequence.background`

Add the lexer/parser support for trailing `&`. After this task, `cmd &` parses successfully into a `Sequence { ..., background: true }`, but the executor ignores the flag (runs it as foreground). No new dependencies; no functional change at runtime yet.

**Files:**
- Modify: `src/lexer.rs`
- Modify: `src/command.rs`
- Modify: `src/shell.rs`
- Modify: `src/expand.rs` (test fixtures only)
- Modify: `src/executor.rs` (test fixtures only)

- [ ] **Step 1: Update `src/lexer.rs`** — add `Operator::Background`, remove `LexError::BareAmpersand`.

Replace this block at the top:

```rust
#[derive(Debug, PartialEq, Eq)]
pub enum LexError {
    UnterminatedQuote,
    BareAmpersand,
    InvalidVarName,
    UnterminatedBrace,
    UnterminatedSubstitution,
    SubstitutionLexError(Box<LexError>),
    SubstitutionParseError(crate::command::ParseError),
}
```

with:

```rust
#[derive(Debug, PartialEq, Eq)]
pub enum LexError {
    UnterminatedQuote,
    InvalidVarName,
    UnterminatedBrace,
    UnterminatedSubstitution,
    SubstitutionLexError(Box<LexError>),
    SubstitutionParseError(crate::command::ParseError),
}
```

Find the `Operator` enum and add a `Background` variant:

```rust
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Operator {
    Pipe,           // |
    RedirOut,       // >
    RedirAppend,    // >>
    RedirIn,        // <
    RedirErr,       // 2>
    RedirErrAppend, // 2>>
    And,            // &&
    Or,             // ||
    Semi,           // ;
    Background,     // &
}
```

Find the `'&'` arm in `tokenize`:

```rust
            '&' => {
                if has_token {
                    flush_literal(&mut parts, &mut current);
                    tokens.push(Token::Word(Word(std::mem::take(&mut parts))));
                    has_token = false;
                }
                if chars.peek() == Some(&'&') {
                    chars.next();
                    tokens.push(Token::Op(Operator::And));
                } else {
                    return Err(LexError::BareAmpersand);
                }
            }
```

Replace with:

```rust
            '&' => {
                if has_token {
                    flush_literal(&mut parts, &mut current);
                    tokens.push(Token::Word(Word(std::mem::take(&mut parts))));
                    has_token = false;
                }
                if chars.peek() == Some(&'&') {
                    chars.next();
                    tokens.push(Token::Op(Operator::And));
                } else {
                    tokens.push(Token::Op(Operator::Background));
                }
            }
```

**Update the two existing BareAmpersand tests.** Find them in `mod tests`:

```rust
    #[test]
    fn tokenize_bare_ampersand_is_error() {
        assert_eq!(tokenize("a & b").unwrap_err(), LexError::BareAmpersand);
    }

    #[test]
    fn tokenize_bare_ampersand_at_end_is_error() {
        assert_eq!(tokenize("a &").unwrap_err(), LexError::BareAmpersand);
    }
```

Replace with:

```rust
    #[test]
    fn tokenize_bare_ampersand_is_background_op() {
        assert_eq!(
            tokenize("a & b").unwrap(),
            vec![w("a"), Token::Op(Operator::Background), w("b")]
        );
    }

    #[test]
    fn tokenize_bare_ampersand_at_end_is_background_op() {
        assert_eq!(
            tokenize("a &").unwrap(),
            vec![w("a"), Token::Op(Operator::Background)]
        );
    }

    #[test]
    fn tokenize_double_ampersand_still_and_op() {
        assert_eq!(
            tokenize("a && b").unwrap(),
            vec![w("a"), Token::Op(Operator::And), w("b")]
        );
    }

    #[test]
    fn tokenize_two_separate_backgrounds() {
        assert_eq!(
            tokenize("a & &").unwrap(),
            vec![
                w("a"),
                Token::Op(Operator::Background),
                Token::Op(Operator::Background),
            ]
        );
    }
```

- [ ] **Step 2: Update `src/command.rs`** — add `background` field and the parser logic.

Find the `Sequence` struct:

```rust
#[derive(Debug, PartialEq, Eq)]
pub struct Sequence {
    pub first: Pipeline,
    pub rest: Vec<(Connector, Pipeline)>,
}
```

Replace with:

```rust
#[derive(Debug, PartialEq, Eq)]
pub struct Sequence {
    pub first: Pipeline,
    pub rest: Vec<(Connector, Pipeline)>,
    pub background: bool,
}
```

Find `ParseError` and add two new variants:

```rust
#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    MissingCommand,
    MissingRedirectTarget,
    RedirectTargetIsOperator,
    UnexpectedBackground,
    BackgroundedMultiPipelineSequence,
}
```

Replace the entire `parse` function:

```rust
pub fn parse(tokens: Vec<Token>) -> Result<Option<Sequence>, ParseError> {
    if tokens.is_empty() {
        return Ok(None);
    }

    let mut iter = tokens.into_iter().peekable();
    let first = parse_pipeline(&mut iter)?;
    let mut rest = Vec::new();
    let mut background = false;

    while let Some(token) = iter.next() {
        match token {
            Token::Op(Operator::Background) => {
                // Trailing `&` only: nothing must follow.
                if iter.peek().is_some() {
                    return Err(ParseError::UnexpectedBackground);
                }
                // And only on a single-pipeline sequence.
                if !rest.is_empty() {
                    return Err(ParseError::BackgroundedMultiPipelineSequence);
                }
                background = true;
                break;
            }
            Token::Op(Operator::Semi) => {
                if iter.peek().is_none() {
                    break;
                }
                let pipeline = parse_pipeline(&mut iter)?;
                rest.push((Connector::Semi, pipeline));
            }
            Token::Op(Operator::And) => {
                let pipeline = parse_pipeline(&mut iter)?;
                rest.push((Connector::And, pipeline));
            }
            Token::Op(Operator::Or) => {
                let pipeline = parse_pipeline(&mut iter)?;
                rest.push((Connector::Or, pipeline));
            }
            _ => unreachable!(
                "parse_pipeline leaves only sequencing ops in the iterator; \
                 anything else it consumes itself"
            ),
        }
    }

    Ok(Some(Sequence { first, rest, background }))
}
```

Find `parse_pipeline` and update its break condition. Locate this line:

```rust
        if matches!(
            token,
            Token::Op(Operator::Semi | Operator::And | Operator::Or)
        ) {
            break;
        }
```

Replace with:

```rust
        if matches!(
            token,
            Token::Op(Operator::Semi | Operator::And | Operator::Or | Operator::Background)
        ) {
            break;
        }
```

**Update test fixtures** — find every `Sequence { first: ..., rest: ... }` literal in `mod tests` and add `background: false`. The helper `one_pipeline` is the main one:

```rust
    fn one_pipeline(commands: Vec<SimpleCommand>) -> Sequence {
        Sequence {
            first: Pipeline { commands },
            rest: vec![],
        }
    }
```

Replace with:

```rust
    fn one_pipeline(commands: Vec<SimpleCommand>) -> Sequence {
        Sequence {
            first: Pipeline { commands },
            rest: vec![],
            background: false,
        }
    }
```

Tests that match against `seq.rest[0]` etc. don't construct full Sequence literals — they're unaffected. The only other tests that build a full Sequence literal directly are in `parse_assignment_with_command_sub_value_moves_parts` (constructs `inner_seq`). Update that one too:

```rust
        let inner_seq = Sequence {
            first: Pipeline {
                commands: vec![SimpleCommand::Exec(ExecCommand {
                    program: ww("echo"),
                    args: vec![ww("bar")],
                    stdin: None,
                    stdout: None,
                    stderr: None,
                })],
            },
            rest: vec![],
        };
```

Replace with:

```rust
        let inner_seq = Sequence {
            first: Pipeline {
                commands: vec![SimpleCommand::Exec(ExecCommand {
                    program: ww("echo"),
                    args: vec![ww("bar")],
                    stdin: None,
                    stdout: None,
                    stderr: None,
                })],
            },
            rest: vec![],
            background: false,
        };
```

**Add new parser tests** at the bottom of `mod tests`:

```rust
    #[test]
    fn parse_command_with_background() {
        let seq = parse(vec![w_tok("sleep"), w_tok("1"), Token::Op(Operator::Background)])
            .unwrap()
            .unwrap();
        assert!(seq.background);
        assert!(seq.rest.is_empty());
        assert_eq!(seq.first.commands, vec![plain("sleep", &["1"])]);
    }

    #[test]
    fn parse_pipeline_backgrounded() {
        // cmd1 | cmd2 &
        let seq = parse(vec![
            w_tok("cmd1"),
            Token::Op(Operator::Pipe),
            w_tok("cmd2"),
            Token::Op(Operator::Background),
        ])
        .unwrap()
        .unwrap();
        assert!(seq.background);
        assert!(seq.rest.is_empty());
        assert_eq!(seq.first.commands.len(), 2);
    }

    #[test]
    fn parse_background_alone_is_missing_command() {
        assert_eq!(
            parse(vec![Token::Op(Operator::Background)]),
            Err(ParseError::MissingCommand)
        );
    }

    #[test]
    fn parse_background_mid_sequence_is_error() {
        assert_eq!(
            parse(vec![
                w_tok("cmd1"),
                Token::Op(Operator::Background),
                w_tok("cmd2"),
            ]),
            Err(ParseError::UnexpectedBackground)
        );
    }

    #[test]
    fn parse_two_backgrounds_is_unexpected() {
        assert_eq!(
            parse(vec![
                w_tok("cmd"),
                Token::Op(Operator::Background),
                Token::Op(Operator::Background),
            ]),
            Err(ParseError::UnexpectedBackground)
        );
    }

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

- [ ] **Step 3: Update `src/shell.rs`** — drop `BareAmpersand` arm, add new ParseError arms.

Find `lex_error_message`:

```rust
fn lex_error_message(error: LexError) -> String {
    match error {
        LexError::UnterminatedQuote => ": unterminated quote".to_string(),
        LexError::BareAmpersand => ": unexpected '&'".to_string(),
        LexError::InvalidVarName => ": invalid variable name in '${...}'".to_string(),
        LexError::UnterminatedBrace => ": unterminated '${...}'".to_string(),
        LexError::UnterminatedSubstitution => ": unterminated command substitution".to_string(),
        LexError::SubstitutionLexError(inner) => {
            format!(" in command substitution{}", lex_error_message(*inner))
        }
        LexError::SubstitutionParseError(inner) => {
            format!(" in command substitution: {}", parse_error_message(inner))
        }
    }
}
```

Replace with:

```rust
fn lex_error_message(error: LexError) -> String {
    match error {
        LexError::UnterminatedQuote => ": unterminated quote".to_string(),
        LexError::InvalidVarName => ": invalid variable name in '${...}'".to_string(),
        LexError::UnterminatedBrace => ": unterminated '${...}'".to_string(),
        LexError::UnterminatedSubstitution => ": unterminated command substitution".to_string(),
        LexError::SubstitutionLexError(inner) => {
            format!(" in command substitution{}", lex_error_message(*inner))
        }
        LexError::SubstitutionParseError(inner) => {
            format!(" in command substitution: {}", parse_error_message(inner))
        }
    }
}
```

Find `parse_error_message`:

```rust
fn parse_error_message(error: ParseError) -> &'static str {
    match error {
        ParseError::MissingCommand => "expected a command",
        ParseError::MissingRedirectTarget => "expected a filename after redirection",
        ParseError::RedirectTargetIsOperator => "expected a filename after redirection",
    }
}
```

Replace with:

```rust
fn parse_error_message(error: ParseError) -> &'static str {
    match error {
        ParseError::MissingCommand => "expected a command",
        ParseError::MissingRedirectTarget => "expected a filename after redirection",
        ParseError::RedirectTargetIsOperator => "expected a filename after redirection",
        ParseError::UnexpectedBackground => "'&' not allowed here",
        ParseError::BackgroundedMultiPipelineSequence => {
            "'&' on multi-command sequence not supported; use a single pipeline"
        }
    }
}
```

- [ ] **Step 4: Update test fixtures in `src/expand.rs`** — find every `Sequence { ... }` literal in `mod tests` and add `background: false`.

Find both `echo_sequence` and `exit_sequence` helpers near the top of `mod tests`:

```rust
    fn echo_sequence(args: &[&str]) -> Sequence {
        Sequence {
            first: Pipeline {
                commands: vec![SimpleCommand::Exec(ExecCommand {
                    program: lit("echo"),
                    args: args.iter().map(|a| lit(a)).collect(),
                    stdin: None,
                    stdout: None,
                    stderr: None,
                })],
            },
            rest: vec![],
        }
    }

    fn exit_sequence(code: i32) -> Sequence {
        Sequence {
            first: Pipeline {
                commands: vec![SimpleCommand::Exec(ExecCommand {
                    program: lit("exit"),
                    args: vec![lit(&code.to_string())],
                    stdin: None,
                    stdout: None,
                    stderr: None,
                })],
            },
            rest: vec![],
        }
    }
```

Replace with:

```rust
    fn echo_sequence(args: &[&str]) -> Sequence {
        Sequence {
            first: Pipeline {
                commands: vec![SimpleCommand::Exec(ExecCommand {
                    program: lit("echo"),
                    args: args.iter().map(|a| lit(a)).collect(),
                    stdin: None,
                    stdout: None,
                    stderr: None,
                })],
            },
            rest: vec![],
            background: false,
        }
    }

    fn exit_sequence(code: i32) -> Sequence {
        Sequence {
            first: Pipeline {
                commands: vec![SimpleCommand::Exec(ExecCommand {
                    program: lit("exit"),
                    args: vec![lit(&code.to_string())],
                    stdin: None,
                    stdout: None,
                    stderr: None,
                })],
            },
            rest: vec![],
            background: false,
        }
    }
```

- [ ] **Step 5: Update test fixtures in `src/executor.rs`** — find every `Sequence { ... }` literal in `mod tests` and add `background: false`.

Find the `one_command_sequence` helper:

```rust
    fn one_command_sequence(cmd: SimpleCommand) -> Sequence {
        Sequence {
            first: Pipeline { commands: vec![cmd] },
            rest: vec![],
        }
    }
```

Replace with:

```rust
    fn one_command_sequence(cmd: SimpleCommand) -> Sequence {
        Sequence {
            first: Pipeline { commands: vec![cmd] },
            rest: vec![],
            background: false,
        }
    }
```

Find `execute_capturing_builtin_pipeline_captures_terminal_stage` (the multi-stage test):

```rust
        let seq = Sequence {
            first: Pipeline {
                commands: vec![exec("echo", &["first"]), exec("echo", &["second"])],
            },
            rest: vec![],
        };
```

Replace with:

```rust
        let seq = Sequence {
            first: Pipeline {
                commands: vec![exec("echo", &["first"]), exec("echo", &["second"])],
            },
            rest: vec![],
            background: false,
        };
```

- [ ] **Step 6: Verify**

Run: `cargo build` — expect PASS, no warnings.
Run: `cargo test` — expect PASS, 162 tests pass (155 existing + 4 new lexer tests + 7 new parser tests − 4 old fixture-build tests that changed in shape but kept the same names... actual count may vary by a small amount; the gate is **0 failed**, **0 warnings**).

Adjust expectations: the 2 old `tokenize_bare_ampersand_*` tests were modified (not added), so net = 155 + 2 new lexer tests (the two "still_and_op" and "two_separate_backgrounds") + 7 new parser tests = 164.

- [ ] **Step 7: Commit**

```bash
git add src/lexer.rs src/command.rs src/shell.rs src/expand.rs src/executor.rs
git commit -m "feat: parse trailing & as Sequence.background flag"
```

---

## Task 2: JobTable module

Create `src/jobs.rs` with the `Job`, `JobState`, `JobTable` types and a comprehensive unit-test suite. No integration yet — this module stands alone.

**Files:**
- Modify: `Cargo.toml` (add libc)
- Modify: `src/main.rs` (add `mod jobs;`)
- Create: `src/jobs.rs`

- [ ] **Step 1: Update `Cargo.toml`** to add the libc dependency.

Find:

```toml
[dependencies]
rustyline = "18.0.0"
signal-hook = "0.4.4"
```

Replace with:

```toml
[dependencies]
rustyline = "18.0.0"
signal-hook = "0.4.4"
libc = "0.2"
```

- [ ] **Step 2: Add `mod jobs;` to `src/main.rs`**

Find:

```rust
mod builtins;
mod command;
mod executor;
mod expand;
mod lexer;
mod shell;
mod shell_state;
```

Replace with:

```rust
mod builtins;
mod command;
mod executor;
mod expand;
mod jobs;
mod lexer;
mod shell;
mod shell_state;
```

- [ ] **Step 3: Create `src/jobs.rs`** with the full module content.

```rust
//! Job table for tracking background pipelines.
//!
//! A `Job` represents one background pipeline. Its `pids` are the PIDs of
//! the pipeline stages in order; its `pgid` is the process group ID
//! (always equal to the first stage's PID). `reap` updates per-pid state
//! when a child is reaped; when all pids are reaped, the job's overall
//! state transitions to `Done` or `Signaled` based on the LAST stage's
//! status (matching bash's pipeline exit-status rule without `pipefail`).

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobState {
    Running,
    Done(i32),
    Signaled(i32),
}

#[derive(Debug, Clone)]
pub struct Job {
    pub id: u32,
    pub pgid: i32,
    pub pids: Vec<i32>,
    pub reaped: Vec<bool>,
    pub last_status: Option<i32>,
    pub command: String,
    pub state: JobState,
    pub notified: bool,
    pub created_at: u64,
}

#[derive(Debug, Clone, Default)]
pub struct JobTable {
    jobs: Vec<Job>,
    next_created_at: u64,
}

impl JobTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts a new Running job. Allocates the lowest unused job id
    /// (bash-style reuse). Returns the allocated id.
    pub fn add(&mut self, pgid: i32, pids: Vec<i32>, command: String) -> u32 {
        let id = self.next_id();
        let n = pids.len();
        let job = Job {
            id,
            pgid,
            pids,
            reaped: vec![false; n],
            last_status: None,
            command,
            state: JobState::Running,
            notified: false,
            created_at: self.next_created_at,
        };
        self.next_created_at += 1;
        self.jobs.push(job);
        self.jobs.sort_by_key(|j| j.id);
        id
    }

    /// Inserts a synthetic already-Done job — used for pure-builtin
    /// pipelines that ran synchronously in the parent shell.
    pub fn add_synthetic_done(&mut self, command: String, exit: i32) -> u32 {
        let id = self.next_id();
        let job = Job {
            id,
            pgid: 0,
            pids: Vec::new(),
            reaped: Vec::new(),
            last_status: Some(0),
            command,
            state: JobState::Done(exit),
            notified: false,
            created_at: self.next_created_at,
        };
        self.next_created_at += 1;
        self.jobs.push(job);
        self.jobs.sort_by_key(|j| j.id);
        id
    }

    pub fn iter(&self) -> impl Iterator<Item = &Job> {
        self.jobs.iter()
    }

    pub fn has_running(&self) -> bool {
        self.jobs.iter().any(|j| matches!(j.state, JobState::Running))
    }

    /// Marks `pid` as reaped with the given raw waitpid status. If the pid
    /// is the LAST stage of its job, records the status; when all pids of
    /// the job are reaped, transitions its overall state. No-op if `pid`
    /// isn't owned by any job in the table.
    pub fn reap(&mut self, pid: i32, raw_status: i32) {
        for job in self.jobs.iter_mut() {
            if let Some(idx) = job.pids.iter().position(|&p| p == pid) {
                if job.reaped[idx] {
                    return;
                }
                job.reaped[idx] = true;
                // Record the status if this is the last stage.
                if idx == job.pids.len() - 1 {
                    job.last_status = Some(raw_status);
                }
                if job.reaped.iter().all(|&b| b) {
                    let raw = job.last_status.unwrap_or(0);
                    job.state = decode_status(raw);
                }
                return;
            }
        }
        // pid not in any job — silently ignore (it could be a long-dead
        // child or one not tracked in the job table).
    }

    /// Returns all non-Running, not-yet-notified jobs (in id order),
    /// marking them notified as a side effect.
    pub fn drain_notifications(&mut self) -> Vec<Job> {
        let mut out = Vec::new();
        for job in self.jobs.iter_mut() {
            if !matches!(job.state, JobState::Running) && !job.notified {
                job.notified = true;
                out.push(job.clone());
            }
        }
        out.sort_by_key(|j| j.id);
        out
    }

    /// Drops all jobs that are non-Running AND notified.
    pub fn remove_notified(&mut self) {
        self.jobs
            .retain(|j| matches!(j.state, JobState::Running) || !j.notified);
    }

    /// Returns the most-recent and previous job ids (for `+`/`-` markers).
    /// Most-recent is the highest `created_at`; previous is the next.
    pub fn current_and_previous(&self) -> (Option<u32>, Option<u32>) {
        let mut by_age: Vec<&Job> = self.jobs.iter().collect();
        by_age.sort_by_key(|j| std::cmp::Reverse(j.created_at));
        let current = by_age.first().map(|j| j.id);
        let previous = by_age.get(1).map(|j| j.id);
        (current, previous)
    }

    fn next_id(&self) -> u32 {
        let mut id = 1u32;
        loop {
            if !self.jobs.iter().any(|j| j.id == id) {
                return id;
            }
            id += 1;
        }
    }
}

/// Decodes a raw waitpid status into a JobState terminal variant.
fn decode_status(raw: libc::c_int) -> JobState {
    if libc::WIFEXITED(raw) {
        JobState::Done(libc::WEXITSTATUS(raw))
    } else if libc::WIFSIGNALED(raw) {
        JobState::Signaled(libc::WTERMSIG(raw))
    } else {
        // Stopped or continued — sub-project A doesn't handle these; treat
        // as still running. In practice we never call decode_status until
        // all pids have been reaped, so this branch shouldn't fire here.
        JobState::Running
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_done_raw(exit: i32) -> libc::c_int {
        // WIFEXITED is true when the low 7 bits are 0; the high 8 bits
        // hold the exit code. Construct that directly.
        exit << 8
    }

    fn fake_signaled_raw(signum: i32) -> libc::c_int {
        // WIFSIGNALED is true when the low 7 bits are 1..0x7E. The signum
        // lives in those low 7 bits.
        signum
    }

    #[test]
    fn add_allocates_id_one_first() {
        let mut t = JobTable::new();
        let id = t.add(100, vec![100], "cmd".to_string());
        assert_eq!(id, 1);
    }

    #[test]
    fn add_after_remove_reuses_lowest_id() {
        let mut t = JobTable::new();
        let _ = t.add(100, vec![100], "a".to_string()); // id 1
        let _ = t.add(101, vec![101], "b".to_string()); // id 2
        let _ = t.add(102, vec![102], "c".to_string()); // id 3
        // Reap b fully so it can be removed.
        t.reap(101, fake_done_raw(0));
        let _ = t.drain_notifications();
        t.remove_notified();
        // Next add should reuse id 2.
        let new_id = t.add(200, vec![200], "d".to_string());
        assert_eq!(new_id, 2);
    }

    #[test]
    fn reap_single_pid_transitions_to_done() {
        let mut t = JobTable::new();
        let id = t.add(100, vec![100], "cmd".to_string());
        t.reap(100, fake_done_raw(0));
        let job = t.iter().find(|j| j.id == id).unwrap();
        assert!(matches!(job.state, JobState::Done(0)));
    }

    #[test]
    fn reap_pipeline_uses_last_stage_status() {
        let mut t = JobTable::new();
        let id = t.add(100, vec![100, 101], "a | b".to_string());
        // Reap first stage with exit 1 — should NOT be the final status.
        t.reap(100, fake_done_raw(1));
        // Job not yet fully reaped.
        let job = t.iter().find(|j| j.id == id).unwrap();
        assert!(matches!(job.state, JobState::Running));
        // Reap last stage with exit 0 — final status comes from this.
        t.reap(101, fake_done_raw(0));
        let job = t.iter().find(|j| j.id == id).unwrap();
        assert!(matches!(job.state, JobState::Done(0)));
    }

    #[test]
    fn reap_signaled_transitions_to_signaled() {
        let mut t = JobTable::new();
        let id = t.add(100, vec![100], "cmd".to_string());
        t.reap(100, fake_signaled_raw(15));
        let job = t.iter().find(|j| j.id == id).unwrap();
        assert!(matches!(job.state, JobState::Signaled(15)));
    }

    #[test]
    fn reap_unknown_pid_is_silent_no_op() {
        let mut t = JobTable::new();
        let _ = t.add(100, vec![100], "cmd".to_string());
        t.reap(999, fake_done_raw(0));
        let job = t.iter().next().unwrap();
        assert!(matches!(job.state, JobState::Running));
    }

    #[test]
    fn drain_notifications_returns_completed_unnotified() {
        let mut t = JobTable::new();
        let id = t.add(100, vec![100], "cmd".to_string());
        t.reap(100, fake_done_raw(0));
        let notifs = t.drain_notifications();
        assert_eq!(notifs.len(), 1);
        assert_eq!(notifs[0].id, id);
        // Second call should be empty (notified flag set).
        let notifs2 = t.drain_notifications();
        assert!(notifs2.is_empty());
    }

    #[test]
    fn drain_notifications_skips_running() {
        let mut t = JobTable::new();
        let _ = t.add(100, vec![100], "running".to_string());
        let notifs = t.drain_notifications();
        assert!(notifs.is_empty());
    }

    #[test]
    fn remove_notified_drops_only_notified_completed() {
        let mut t = JobTable::new();
        let id_a = t.add(100, vec![100], "a".to_string()); // 1, running
        let id_b = t.add(101, vec![101], "b".to_string()); // 2, running
        t.reap(100, fake_done_raw(0));
        let _ = t.drain_notifications(); // marks id_a notified
        t.remove_notified();
        let remaining: Vec<u32> = t.iter().map(|j| j.id).collect();
        assert_eq!(remaining, vec![id_b]);
        let _ = id_a;
    }

    #[test]
    fn has_running_tracks_state() {
        let mut t = JobTable::new();
        assert!(!t.has_running());
        let _ = t.add(100, vec![100], "x".to_string());
        assert!(t.has_running());
        t.reap(100, fake_done_raw(0));
        assert!(!t.has_running());
    }

    #[test]
    fn add_synthetic_done_immediate() {
        let mut t = JobTable::new();
        let id = t.add_synthetic_done("echo hi".to_string(), 0);
        let job = t.iter().find(|j| j.id == id).unwrap();
        assert!(matches!(job.state, JobState::Done(0)));
        assert!(job.pids.is_empty());
    }

    #[test]
    fn current_and_previous_tracks_insertion_order() {
        let mut t = JobTable::new();
        let id_a = t.add(100, vec![100], "a".to_string()); // 1
        let id_b = t.add(101, vec![101], "b".to_string()); // 2
        let id_c = t.add(102, vec![102], "c".to_string()); // 3
        let (cur, prev) = t.current_and_previous();
        assert_eq!(cur, Some(id_c));
        assert_eq!(prev, Some(id_b));
        let _ = id_a;
    }
}
```

- [ ] **Step 4: Verify**

Run: `cargo build` — expect PASS, no warnings.
Run: `cargo test` — expect PASS, all green (Task 1 count + 11 new jobs::tests).

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml src/main.rs src/jobs.rs
git commit -m "feat: add JobTable module for background job tracking"
```

---

## Task 3: Wire `Shell` state + `source` parameter

Add `JobTable` and the SIGCHLD `AtomicBool` to `Shell`. Thread `source: &str` through `executor::execute` so a background pipeline can later capture the original input line for display. Install the SIGCHLD handler in `shell::run`. No behavior change yet (no one reads `jobs` or `sigchld_flag`, and the executor ignores `source`).

**Files:**
- Modify: `src/shell_state.rs`
- Modify: `src/shell.rs`
- Modify: `src/executor.rs`

- [ ] **Step 1: Update `src/shell_state.rs`** to add the new fields.

Find the `Shell` struct and `Shell::new`:

```rust
#[derive(Debug, Clone)]
pub struct Shell {
    vars: HashMap<String, Variable>,
    last_status: i32,
}

impl Shell {
    pub fn new() -> Self {
        let mut vars = HashMap::new();
        for (key, value) in std::env::vars() {
            vars.insert(key, Variable { value, exported: true });
        }
        Self { vars, last_status: 0 }
    }
```

Replace with:

```rust
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::jobs::JobTable;

#[derive(Debug, Clone)]
pub struct Shell {
    vars: HashMap<String, Variable>,
    last_status: i32,
    pub jobs: JobTable,
    pub sigchld_flag: Arc<AtomicBool>,
}

impl Shell {
    pub fn new() -> Self {
        let mut vars = HashMap::new();
        for (key, value) in std::env::vars() {
            vars.insert(key, Variable { value, exported: true });
        }
        Self {
            vars,
            last_status: 0,
            jobs: JobTable::new(),
            sigchld_flag: Arc::new(AtomicBool::new(false)),
        }
    }
```

Note: `use std::collections::HashMap;` already exists at the top of the file. Leave it.

- [ ] **Step 2: Update `src/shell.rs`** to install the SIGCHLD handler and pass `source` through.

Find the existing imports near the top:

```rust
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;
use signal_hook::consts::SIGINT;

use crate::builtins::ExecOutcome;
use crate::command::{self, ParseError};
use crate::executor;
use crate::lexer::{self, LexError};
use crate::shell_state::Shell;
```

Replace with:

```rust
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;
use signal_hook::consts::{SIGCHLD, SIGINT};

use crate::builtins::ExecOutcome;
use crate::command::{self, ParseError};
use crate::executor;
use crate::lexer::{self, LexError};
use crate::shell_state::Shell;
```

Find `install_sigint_handler` and add a parallel `install_sigchld_handler` immediately after it:

```rust
/// Installs a SIGCHLD handler that toggles the supplied flag. Called once
/// at startup; the flag lives on the `Shell` so the reap path can poll it.
fn install_sigchld_handler(flag: Arc<AtomicBool>) {
    if let Err(e) = signal_hook::flag::register(SIGCHLD, flag) {
        eprintln!("shuck: warning: could not install SIGCHLD handler: {e}");
    }
}
```

Find `pub fn run() -> i32` and update its body. Find this section:

```rust
    install_sigint_handler();

    let mut editor = match DefaultEditor::new() {
        Ok(editor) => editor,
        Err(e) => {
            eprintln!("shuck: failed to initialize line editor: {e}");
            return 1;
        }
    };

    let mut shell = Shell::new();
```

Replace with:

```rust
    install_sigint_handler();

    let mut editor = match DefaultEditor::new() {
        Ok(editor) => editor,
        Err(e) => {
            eprintln!("shuck: failed to initialize line editor: {e}");
            return 1;
        }
    };

    let mut shell = Shell::new();
    install_sigchld_handler(Arc::clone(&shell.sigchld_flag));
```

Find the call to `process_line(&line, &mut shell)` and leave it for now (next step changes its signature).

Find `process_line` and update its signature + body:

```rust
/// Tokenizes, parses, and executes a single input line.
fn process_line(line: &str, shell: &mut Shell) -> ExecOutcome {
    let tokens = match lexer::tokenize(line) {
        Ok(tokens) => tokens,
        Err(e) => {
            eprintln!("shuck: syntax error{}", lex_error_message(e));
            return ExecOutcome::Continue(2);
        }
    };

    match command::parse(tokens) {
        Ok(Some(sequence)) => executor::execute(&sequence, shell),
        Ok(None) => ExecOutcome::Continue(0),
        Err(e) => {
            eprintln!("shuck: syntax error: {}", parse_error_message(e));
            ExecOutcome::Continue(2)
        }
    }
}
```

Replace with:

```rust
/// Tokenizes, parses, and executes a single input line.
fn process_line(line: &str, shell: &mut Shell) -> ExecOutcome {
    let tokens = match lexer::tokenize(line) {
        Ok(tokens) => tokens,
        Err(e) => {
            eprintln!("shuck: syntax error{}", lex_error_message(e));
            return ExecOutcome::Continue(2);
        }
    };

    match command::parse(tokens) {
        Ok(Some(sequence)) => executor::execute(&sequence, shell, line),
        Ok(None) => ExecOutcome::Continue(0),
        Err(e) => {
            eprintln!("shuck: syntax error: {}", parse_error_message(e));
            ExecOutcome::Continue(2)
        }
    }
}
```

- [ ] **Step 3: Update `src/executor.rs`** to accept the new `source: &str` parameter.

Find the top of the file with the public API:

```rust
pub fn execute(seq: &Sequence, shell: &mut Shell) -> ExecOutcome {
    let mut sink = StdoutSink::Terminal;
    execute_inner(seq, shell, &mut sink)
}

/// Runs a sequence with stdout captured to a buffer. The returned status is
/// the last command's exit code (`ExecOutcome::Exit` and `Continue` are both
/// treated as a normal status here — `exit N` inside `$(...)` terminates the
/// substitution with status N, not the parent shuck).
pub fn execute_capturing(seq: &Sequence, shell: &mut Shell) -> (String, i32) {
    let mut buf: Vec<u8> = Vec::new();
    let outcome = {
        let mut sink = StdoutSink::Capture(&mut buf);
        execute_inner(seq, shell, &mut sink)
    };
    let status = match outcome {
        ExecOutcome::Continue(c) | ExecOutcome::Exit(c) => c,
    };
    (String::from_utf8_lossy(&buf).into_owned(), status)
}

fn execute_inner(seq: &Sequence, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
```

Replace with:

```rust
pub fn execute(seq: &Sequence, shell: &mut Shell, source: &str) -> ExecOutcome {
    let mut sink = StdoutSink::Terminal;
    execute_inner(seq, shell, &mut sink, source)
}

/// Runs a sequence with stdout captured to a buffer. Used by command
/// substitution; the substituted command's `background` flag is ignored
/// (substitutions always wait), and we pass an empty `source` since job-
/// table registration is irrelevant inside a substitution.
pub fn execute_capturing(seq: &Sequence, shell: &mut Shell) -> (String, i32) {
    let mut buf: Vec<u8> = Vec::new();
    let outcome = {
        let mut sink = StdoutSink::Capture(&mut buf);
        execute_inner(seq, shell, &mut sink, "")
    };
    let status = match outcome {
        ExecOutcome::Continue(c) | ExecOutcome::Exit(c) => c,
    };
    (String::from_utf8_lossy(&buf).into_owned(), status)
}

fn execute_inner(seq: &Sequence, shell: &mut Shell, sink: &mut StdoutSink, _source: &str) -> ExecOutcome {
```

(The `_source` underscore-prefix avoids the unused-parameter warning. Task 4 starts using it.)

**Update test call sites in `src/executor.rs::tests`.** Find each `execute_capturing(&seq, &mut shell)` — those are already correct (the public API didn't change). No changes needed for the existing tests.

- [ ] **Step 4: Verify**

Run: `cargo build` — expect PASS, no warnings.
Run: `cargo test` — expect PASS, same test count as Task 2 (no new tests).

- [ ] **Step 5: Commit**

```bash
git add src/shell_state.rs src/shell.rs src/executor.rs
git commit -m "feat: thread JobTable + sigchld_flag + source through Shell/executor"
```

---

## Task 4: Executor background path

Implement `run_background_sequence`. For pure-builtin pipelines: run synchronously in the parent shell (side effects propagate) and register a synthetic Done job. Otherwise: spawn the pipeline with `Command::process_group` so children are in their own pg, redirect first-stage stdin from `/dev/null` if not explicit, register a `Job` in the table, print `[N] PID` to stderr, and return immediately.

**Files:**
- Modify: `src/executor.rs`

- [ ] **Step 1: Update `src/executor.rs`** — branch on `seq.background` and add the new path.

Find `execute_inner` (just changed in Task 3):

```rust
fn execute_inner(seq: &Sequence, shell: &mut Shell, sink: &mut StdoutSink, _source: &str) -> ExecOutcome {
    let mut status = run_pipeline(&seq.first, shell, sink);
    if matches!(status, ExecOutcome::Exit(_)) {
        return status;
    }
    for (connector, pipeline) in &seq.rest {
        let should_run = match connector {
            Connector::Semi => true,
            Connector::And => matches!(status, ExecOutcome::Continue(0)),
            Connector::Or => matches!(status, ExecOutcome::Continue(c) if c != 0),
        };
        if should_run {
            status = run_pipeline(pipeline, shell, sink);
            if matches!(status, ExecOutcome::Exit(_)) {
                return status;
            }
        }
    }
    status
}
```

Replace with:

```rust
fn execute_inner(seq: &Sequence, shell: &mut Shell, sink: &mut StdoutSink, source: &str) -> ExecOutcome {
    if seq.background {
        // Parser guarantees rest.is_empty() when background is set.
        return run_background_sequence(&seq.first, shell, sink, source);
    }
    let mut status = run_pipeline(&seq.first, shell, sink);
    if matches!(status, ExecOutcome::Exit(_)) {
        return status;
    }
    for (connector, pipeline) in &seq.rest {
        let should_run = match connector {
            Connector::Semi => true,
            Connector::And => matches!(status, ExecOutcome::Continue(0)),
            Connector::Or => matches!(status, ExecOutcome::Continue(c) if c != 0),
        };
        if should_run {
            status = run_pipeline(pipeline, shell, sink);
            if matches!(status, ExecOutcome::Exit(_)) {
                return status;
            }
        }
    }
    status
}
```

**Add the new `run_background_sequence` function** (and a helper). Insert these immediately AFTER `run_pipeline`:

```rust
// ----- background pipeline --------------------------------------------------

fn run_background_sequence(
    pipeline: &Pipeline,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    source: &str,
) -> ExecOutcome {
    let display = display_command(source);

    if pipeline_is_pure_builtin(pipeline) {
        // Run synchronously in the parent shell. Side effects (cd, exports,
        // exit) take effect on the parent — documented divergence from bash,
        // which would fork a subshell.
        let outcome = run_pipeline(pipeline, shell, sink);
        if matches!(outcome, ExecOutcome::Exit(_)) {
            return outcome;
        }
        let exit = match outcome {
            ExecOutcome::Continue(c) => c,
            ExecOutcome::Exit(_) => unreachable!(),
        };
        let id = shell.jobs.add_synthetic_done(display, exit);
        eprintln!("[{id}] Done");
        return ExecOutcome::Continue(0);
    }

    // Spawn each stage with process_group. The first stage gets
    // process_group(0) to become its own pg leader; subsequent stages join
    // that pg via process_group(first_pid). The first stage's stdin
    // defaults to /dev/null (so background commands don't fight the shell
    // for the terminal); explicit `< file` redirects override this.
    let n = pipeline.commands.len();
    let mut all_resolved: Vec<Option<ResolvedCommand>> = Vec::with_capacity(n);
    for cmd in &pipeline.commands {
        match cmd {
            SimpleCommand::Assign { .. } => {
                all_resolved.push(None);
            }
            SimpleCommand::Exec(exec) => match resolve(exec, shell) {
                Ok(r) => all_resolved.push(Some(r)),
                Err(code) => {
                    // Failed to expand; print the [N] line for the failed
                    // job so the user can see what happened, and bail.
                    return ExecOutcome::Continue(code);
                }
            },
        }
    }

    let mut spawned_pids: Vec<i32> = Vec::with_capacity(n);
    let mut first_pid: Option<i32> = None;
    let mut carry: Option<ChildStdout> = None;
    let mut children: Vec<Child> = Vec::with_capacity(n);

    for (i, resolved) in all_resolved.iter().enumerate() {
        let is_last = i == n - 1;
        let Some(cmd) = resolved else {
            // Assign stage in a background pipeline: no-op stage. The carry
            // input from the previous stage is dropped; the next stage will
            // get an empty pipe (Stdio::null instead of stdin from prev).
            carry = None;
            continue;
        };

        let files = match open_stage_files(cmd) {
            Ok(f) => f,
            Err(()) => return ExecOutcome::Continue(1),
        };

        let mut process = ProcessCommand::new(&cmd.program);
        process.args(&cmd.args);
        process.env_clear();
        process.envs(shell.exported_env());

        // Process-group: first stage = own pg leader; rest join.
        use std::os::unix::process::CommandExt;
        let pgid_target = first_pid.unwrap_or(0);
        process.process_group(pgid_target);

        // Stdin: explicit redirect wins; otherwise carry from prev stage if
        // any; otherwise /dev/null for the first stage.
        if let Some(file) = files.stdin {
            process.stdin(Stdio::from(file));
        } else if let Some(child_stdout) = carry.take() {
            process.stdin(Stdio::from(child_stdout));
        } else {
            process.stdin(Stdio::null());
        }

        // Stdout: explicit redirect wins; otherwise pipe onward if not last;
        // otherwise inherit terminal.
        if let Some(file) = files.stdout {
            process.stdout(Stdio::from(file));
        } else if !is_last {
            process.stdout(Stdio::piped());
        }

        if let Some(file) = files.stderr {
            process.stderr(Stdio::from(file));
        }

        let mut child = match process.spawn() {
            Ok(c) => c,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                eprintln!("shuck: command not found: {}", cmd.program);
                // Bail out — partial pipeline cleanup is best-effort.
                for mut c in children {
                    let _ = c.kill();
                }
                return ExecOutcome::Continue(127);
            }
            Err(e) => {
                eprintln!("shuck: {}: {e}", cmd.program);
                for mut c in children {
                    let _ = c.kill();
                }
                return ExecOutcome::Continue(1);
            }
        };

        let pid = child.id() as i32;
        spawned_pids.push(pid);
        if first_pid.is_none() {
            first_pid = Some(pid);
        }

        if !is_last {
            carry = child.stdout.take();
        }

        children.push(child);
    }

    let Some(pgid) = first_pid else {
        // No actual children spawned (all-Assign pipeline). Treat as
        // synthetic Done. This shouldn't happen in practice — the parser
        // doesn't produce all-Assign backgrounded pipelines as a typical
        // user input shape, but we handle it defensively.
        let id = shell.jobs.add_synthetic_done(display, 0);
        eprintln!("[{id}] Done");
        return ExecOutcome::Continue(0);
    };

    // Forget the Child structs so the OS doesn't try to reap them as
    // zombies via Drop — we own reaping via waitpid.
    for child in children {
        std::mem::forget(child);
    }

    let last_pid = *spawned_pids.last().unwrap();
    let id = shell.jobs.add(pgid, spawned_pids, display);
    eprintln!("[{id}] {last_pid}");
    ExecOutcome::Continue(0)
}

/// True iff every stage in the pipeline is a builtin (or an Assign).
fn pipeline_is_pure_builtin(pipeline: &Pipeline) -> bool {
    pipeline.commands.iter().all(|cmd| match cmd {
        SimpleCommand::Exec(e) => match e.program.0.first() {
            Some(crate::lexer::WordPart::Literal(name)) => builtins::is_builtin(name),
            _ => false,
        },
        SimpleCommand::Assign { .. } => true,
    })
}

/// Strips a trailing `&` and surrounding whitespace from the source line for
/// display in the job table.
fn display_command(source: &str) -> String {
    source
        .trim_end()
        .trim_end_matches('&')
        .trim_end()
        .to_string()
}
```

(The new code uses `std::process::ChildStdout`, which is already imported at the top of `executor.rs`.)

**Important note on `std::mem::forget(child)`:** Rust's `std::process::Child` auto-reaps on drop (via `wait` in some versions, or by leaking the zombie in others). We explicitly forget the Child so the OS keeps the zombie around and `waitpid` from the reaper can see it. This is the standard pattern for hand-managed wait.

- [ ] **Step 2: Add a unit test for the pure-builtin synchronous path.**

Find the `#[cfg(test)] mod tests` block in `src/executor.rs` and add at the bottom (before the closing `}`):

```rust
    use crate::jobs::JobState;

    #[test]
    fn background_pure_builtin_runs_synchronously_and_registers_done_job() {
        let seq = Sequence {
            first: Pipeline {
                commands: vec![exec("echo", &["hi"])],
            },
            rest: vec![],
            background: true,
        };
        let mut shell = Shell::new();
        let outcome = execute(&seq, &mut shell, "echo hi &");
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let jobs: Vec<_> = shell.jobs.iter().collect();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].command, "echo hi");
        assert!(matches!(jobs[0].state, JobState::Done(0)));
        assert!(jobs[0].pids.is_empty()); // synthetic — no real pids
    }

    #[test]
    fn background_pure_builtin_assignment_runs_in_parent() {
        let seq = Sequence {
            first: Pipeline {
                commands: vec![SimpleCommand::Assign {
                    name: "SHUCK_TEST_BG_ASSIGN".to_string(),
                    value: lit_word("v"),
                }],
            },
            rest: vec![],
            background: true,
        };
        let mut shell = Shell::new();
        let _ = execute(&seq, &mut shell, "SHUCK_TEST_BG_ASSIGN=v &");
        // The assignment ran in the parent (pure-builtin path).
        assert_eq!(shell.get("SHUCK_TEST_BG_ASSIGN"), Some("v"));
    }
```

(The `Shell::get` method exists; `lit_word` is already a helper in `mod tests`.)

- [ ] **Step 3: Verify**

Run: `cargo build` — expect PASS, no warnings.
Run: `cargo test` — expect PASS, +2 new executor tests.

- [ ] **Step 4: Smoke test** (manual; no commit yet)

```bash
cargo build -q
echo "--- pure builtin background ---"
printf 'echo hi &\nexit 0\n' | ./target/debug/shuck

echo "--- real subprocess background ---"
printf 'sleep 0.1 &\nsleep 0.3\nexit 0\n' | ./target/debug/shuck
```

Expected output for the pure builtin: `hi` then `[1] Done`. For the real subprocess: a `[1] <pid>` line on stderr after `sleep 0.1 &`; the second sleep blocks for 0.3s while the background sleep finishes silently (no `wait` invoked, no Done line yet — that comes in Task 5).

If pure-builtin works but the real subprocess errors, stop and investigate.

- [ ] **Step 5: Commit**

```bash
git add src/executor.rs
git commit -m "feat: executor backgrounds pipelines into own process group"
```

---

## Task 5: SIGCHLD reaping + prompt notifications

Add `reap_completed` and `reap_and_notify` to `src/jobs.rs`. Call `reap_and_notify` from the REPL right before each `editor.readline(PROMPT)`. After this task, completed jobs show `[N] Done` lines before the next prompt.

**Files:**
- Modify: `src/jobs.rs`
- Modify: `src/shell.rs`

- [ ] **Step 1: Update `src/jobs.rs`** to add the reap functions.

Add these immediately after the `JobTable` impl block:

```rust
/// Drains all reapable children via non-blocking `waitpid(WNOHANG)`, feeding
/// each into the shell's job table. Also resets the SIGCHLD flag.
pub fn reap_completed(shell: &mut crate::shell_state::Shell) {
    shell
        .sigchld_flag
        .store(false, std::sync::atomic::Ordering::Relaxed);
    loop {
        let mut raw_status: libc::c_int = 0;
        let pid = unsafe {
            libc::waitpid(-1, &mut raw_status, libc::WNOHANG)
        };
        if pid <= 0 {
            // 0 = no children changed state; -1 = no children at all (ECHILD)
            break;
        }
        shell.jobs.reap(pid as i32, raw_status);
    }
}

/// Reaps and then prints `[N]<flag> <state> <cmd> &` for any newly-completed
/// jobs. Drops the printed jobs from the table.
pub fn reap_and_notify(shell: &mut crate::shell_state::Shell) {
    reap_completed(shell);
    let (current, previous) = shell.jobs.current_and_previous();
    let notifs = shell.jobs.drain_notifications();
    for job in notifs {
        let flag = if Some(job.id) == current {
            '+'
        } else if Some(job.id) == previous {
            '-'
        } else {
            ' '
        };
        let state = render_state(&job.state);
        eprintln!("[{}]{} {:<20} {} &", job.id, flag, state, job.command);
    }
    shell.jobs.remove_notified();
}

fn render_state(state: &JobState) -> String {
    match state {
        JobState::Running => "Running".to_string(),
        JobState::Done(0) => "Done".to_string(),
        JobState::Done(n) => format!("Exit {n}"),
        JobState::Signaled(s) => format!("Killed (signal {s})"),
    }
}
```

Make `render_state` `pub` so the `jobs` builtin in Task 6 can reuse it:

```rust
pub fn render_state(state: &JobState) -> String {
```

- [ ] **Step 2: Update `src/shell.rs`** — call `jobs::reap_and_notify` before each prompt.

Find the REPL loop in `pub fn run() -> i32`:

```rust
    loop {
        match editor.readline(PROMPT) {
            Ok(line) => {
                if !line.trim().is_empty() {
                    let _ = editor.add_history_entry(line.as_str());
                }
                match process_line(&line, &mut shell) {
                    ExecOutcome::Exit(code) => return code,
                    ExecOutcome::Continue(status) => shell.set_last_status(status),
                }
            }
            Err(ReadlineError::Interrupted) => continue,
            Err(ReadlineError::Eof) => return shell.last_status(),
            Err(e) => {
                eprintln!("shuck: input error: {e}");
                return 1;
            }
        }
    }
```

Replace with:

```rust
    loop {
        crate::jobs::reap_and_notify(&mut shell);
        match editor.readline(PROMPT) {
            Ok(line) => {
                if !line.trim().is_empty() {
                    let _ = editor.add_history_entry(line.as_str());
                }
                match process_line(&line, &mut shell) {
                    ExecOutcome::Exit(code) => return code,
                    ExecOutcome::Continue(status) => shell.set_last_status(status),
                }
            }
            Err(ReadlineError::Interrupted) => continue,
            Err(ReadlineError::Eof) => return shell.last_status(),
            Err(e) => {
                eprintln!("shuck: input error: {e}");
                return 1;
            }
        }
    }
```

- [ ] **Step 3: Verify**

Run: `cargo build` — expect PASS, no warnings.
Run: `cargo test` — expect PASS, no new tests (functions are exercised in Task 7 smoke).

- [ ] **Step 4: Smoke test** (manual; no commit yet)

```bash
cargo build -q
printf 'sleep 0.1 &\nsleep 0.3\necho after\nexit 0\n' | ./target/debug/shuck
```

Expected output (loose ordering — the `[1] Done` line appears before the prompt that would precede `echo after`):

- A `[1] <pid>` line on stderr immediately after `sleep 0.1 &`.
- The shell waits 0.3s for the foreground sleep.
- Before the next prompt (where `echo after` is read), a `[1]+ Done                 sleep 0.1 &` line on stderr.
- `after` on stdout.

Visual check; if Done line is missing, the reaper isn't being called or the SIGCHLD flag isn't right.

- [ ] **Step 5: Commit**

```bash
git add src/jobs.rs src/shell.rs
git commit -m "feat: reap finished jobs and notify before each prompt"
```

---

## Task 6: `jobs` and `wait` builtins

Add the two new builtins. `jobs` lists the table to its `out` writer. `wait` (no-args only in sub-project A) blocks until all jobs are non-Running.

**Files:**
- Modify: `src/builtins.rs`

- [ ] **Step 1: Update `src/builtins.rs`** — extend `is_builtin`, `run_builtin`, and add `builtin_jobs` + `builtin_wait`.

Find `is_builtin`:

```rust
pub fn is_builtin(name: &str) -> bool {
    matches!(name, "cd" | "exit" | "pwd" | "echo" | "export" | "unset")
}
```

Replace with:

```rust
pub fn is_builtin(name: &str) -> bool {
    matches!(
        name,
        "cd" | "exit" | "pwd" | "echo" | "export" | "unset" | "jobs" | "wait"
    )
}
```

Find `run_builtin`:

```rust
pub fn run_builtin(
    name: &str,
    args: &[String],
    out: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    match name {
        "cd" => builtin_cd(args, shell),
        "pwd" => builtin_pwd(out),
        "echo" => builtin_echo(args, out),
        "exit" => builtin_exit(args),
        "export" => builtin_export(args, out, shell),
        "unset" => builtin_unset(args, shell),
        _ => unreachable!("run_builtin called with non-builtin: {name}"),
    }
}
```

Replace with:

```rust
pub fn run_builtin(
    name: &str,
    args: &[String],
    out: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    match name {
        "cd" => builtin_cd(args, shell),
        "pwd" => builtin_pwd(out),
        "echo" => builtin_echo(args, out),
        "exit" => builtin_exit(args),
        "export" => builtin_export(args, out, shell),
        "unset" => builtin_unset(args, shell),
        "jobs" => builtin_jobs(args, out, shell),
        "wait" => builtin_wait(args, out, shell),
        _ => unreachable!("run_builtin called with non-builtin: {name}"),
    }
}
```

Add the two new builtin functions immediately before the `#[cfg(test)] mod tests` block:

```rust
fn builtin_jobs(args: &[String], out: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    if !args.is_empty() {
        eprintln!("shuck: jobs: arguments not supported in this version");
        return ExecOutcome::Continue(2);
    }
    let (current, previous) = shell.jobs.current_and_previous();
    for job in shell.jobs.iter() {
        let flag = if Some(job.id) == current {
            '+'
        } else if Some(job.id) == previous {
            '-'
        } else {
            ' '
        };
        let state = crate::jobs::render_state(&job.state);
        if let Err(e) = writeln!(out, "[{}]{} {:<20} {} &", job.id, flag, state, job.command) {
            eprintln!("shuck: jobs: {e}");
            return ExecOutcome::Continue(1);
        }
    }
    ExecOutcome::Continue(0)
}

fn builtin_wait(args: &[String], _out: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    if !args.is_empty() {
        eprintln!("shuck: wait: arguments not supported in this version");
        return ExecOutcome::Continue(2);
    }
    // Blocking wait loop until no Running jobs remain (or no children at all).
    while shell.jobs.has_running() {
        let mut raw_status: libc::c_int = 0;
        let pid = unsafe { libc::waitpid(-1, &mut raw_status, 0) };
        if pid <= 0 {
            // -1 with ECHILD or 0 (shouldn't happen without WNOHANG); bail.
            break;
        }
        shell.jobs.reap(pid as i32, raw_status);
    }
    // Print Done lines for anything that just transitioned during the wait.
    crate::jobs::reap_and_notify(shell);
    ExecOutcome::Continue(0)
}
```

**Add unit tests** at the bottom of the existing `mod tests` block:

```rust
    #[test]
    fn jobs_with_empty_table_prints_nothing_and_returns_zero() {
        let mut shell = Shell::new();
        let mut out: Vec<u8> = Vec::new();
        let outcome = builtin_jobs(&[], &mut out, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert!(out.is_empty());
    }

    #[test]
    fn jobs_lists_synthetic_done_entry() {
        let mut shell = Shell::new();
        let _ = shell.jobs.add_synthetic_done("echo hi".to_string(), 0);
        let mut out: Vec<u8> = Vec::new();
        let outcome = builtin_jobs(&[], &mut out, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("[1]"));
        assert!(s.contains("Done"));
        assert!(s.contains("echo hi"));
    }

    #[test]
    fn jobs_with_args_errors() {
        let mut shell = Shell::new();
        let mut out: Vec<u8> = Vec::new();
        let outcome = builtin_jobs(&["-l".to_string()], &mut out, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn wait_with_no_jobs_returns_zero_immediately() {
        let mut shell = Shell::new();
        let mut out: Vec<u8> = Vec::new();
        let outcome = builtin_wait(&[], &mut out, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
    }

    #[test]
    fn wait_with_args_errors() {
        let mut shell = Shell::new();
        let mut out: Vec<u8> = Vec::new();
        let outcome = builtin_wait(&["%1".to_string()], &mut out, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn is_builtin_recognizes_jobs_and_wait() {
        assert!(is_builtin("jobs"));
        assert!(is_builtin("wait"));
    }
```

- [ ] **Step 2: Verify**

Run: `cargo build` — expect PASS, no warnings.
Run: `cargo test` — expect PASS, +6 new builtin tests.

- [ ] **Step 3: Smoke test** (manual; no commit yet)

```bash
cargo build -q
printf 'sleep 0.2 &\njobs\nwait\njobs\nexit 0\n' | ./target/debug/shuck
```

Expected:
- After `sleep 0.2 &`: `[1] <pid>` on stderr.
- After `jobs`: `[1]+ Running              sleep 0.2 &` line on stdout.
- After `wait`: blocks for ~0.2s, then `[1]+ Done                 sleep 0.2 &` on stderr.
- After second `jobs`: nothing (table empty).
- Shell exits.

- [ ] **Step 4: Commit**

```bash
git add src/builtins.rs
git commit -m "feat: add jobs and wait builtins"
```

---

## Task 7: Full smoke test

End-to-end verification of background jobs combined with v1–v5 features. Verification only — no commit.

**Files:** none

- [ ] **Step 1: Run the combined smoke script**

```bash
cargo build -q

echo "--- 1: single background subprocess ---"
printf 'sleep 0.1 &\nwait\nexit 0\n' | ./target/debug/shuck
# Expected: [1] <pid>; (blank); [1]+ Done sleep 0.1 &

echo "--- 2: jobs builtin shows running jobs ---"
printf 'sleep 0.3 &\njobs\nwait\nexit 0\n' | ./target/debug/shuck
# Expected: [1] <pid>; [1]+ Running sleep 0.3 & (on stdout from jobs);
#           after wait: [1]+ Done sleep 0.3 & (on stderr)

echo "--- 3: multiple background jobs ---"
printf 'sleep 0.2 &\nsleep 0.3 &\njobs\nwait\njobs\nexit 0\n' | ./target/debug/shuck
# Expected: two [N] <pid> lines; jobs prints two Running entries;
#           wait blocks; two Done notifications; final jobs is empty.

echo "--- 4: pure builtin background runs synchronously ---"
printf 'echo from-bg &\nexit 0\n' | ./target/debug/shuck
# Expected: from-bg printed to stdout; [N] Done on stderr immediately.

echo "--- 5: assignment in background mutates parent ---"
printf 'SHUCK_BG_TEST=val &\necho got $SHUCK_BG_TEST\nexit 0\n' | ./target/debug/shuck
# Expected: [N] Done; "got val"

echo "--- 6: backgrounded false captures exit code; \$? after wait is 0 ---"
printf 'false &\nwait\necho $?\nexit 0\n' | ./target/debug/shuck
# Expected: [1] <pid>; Done line; "0"

echo "--- 7: pipeline backgrounded ---"
printf 'echo data | head -c 4 &\nwait\nexit 0\n' | ./target/debug/shuck
# Expected: [1] <pid>; the "data" output (truncated to "data" — 4 chars); Done line

echo "--- 8: \$? after background spawn is 0 ---"
printf 'false\nsleep 0.05 &\necho $?\nwait\nexit 0\n' | ./target/debug/shuck
# Expected: [1] <pid>; "0" (the spawn succeeded, regardless of prior false);
#           wait completes silently; Done line on next prompt (but exit comes first).
```

- [ ] **Step 2: Verify error paths**

```bash
echo "--- E1: mid-sequence & ---"
printf 'cmd1 & cmd2\n' | ./target/debug/shuck
# Expected: shuck: syntax error: '&' not allowed here

echo "--- E2: && followed by & ---"
printf 'cmd1 && cmd2 &\n' | ./target/debug/shuck
# Expected: shuck: syntax error: '&' on multi-command sequence not supported; use a single pipeline

echo "--- E3: ; followed by & ---"
printf 'cmd1 ; cmd2 &\n' | ./target/debug/shuck
# Expected: same as E2

echo "--- E4: jobs with args ---"
printf 'jobs -l\n' | ./target/debug/shuck
# Expected (on stderr): shuck: jobs: arguments not supported in this version

echo "--- E5: wait with args ---"
printf 'wait %%1\n' | ./target/debug/shuck
# Expected: shuck: wait: arguments not supported in this version

echo "--- E6: lone & ---"
printf '&\n' | ./target/debug/shuck
# Expected: shuck: syntax error: expected a command
```

- [ ] **Step 3: Confirm**

All output matches (allowing for the inherent flakiness of timestamp-dependent display in subprocess-spawn tests). If any line is unexpectedly different, stop and fix the relevant module before completing the plan.

---
