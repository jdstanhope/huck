# huck v150 — Process substitution `<(…)` / `>(…)` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Implement bash process substitution — `<(cmd)` (read from `cmd`'s stdout) and
`>(cmd)` (write to `cmd`'s stdin) — usable as command arguments and redirect targets.

**Architecture:** A new word part `WordPart::ProcessSub { sequence, dir }` (sibling of
`CommandSub`). The lexer recognizes unquoted `<(`/`>(`. At word-expansion time, a new
`src/procsub.rs` module realizes a `/dev/fd/N` filename (FIFO fallback), forks the inner
command via the existing `fork_and_run_in_subshell`, and records a cleanup entry on the
`Shell`. The executor snapshots/drains those entries around each command so the inner
processes live exactly as long as the outer command. POSIX-only; macOS-portable
(runtime `/dev/fd` detection, `mkfifo` fallback, no `/proc`).

**Tech Stack:** Rust; `libc` (`pipe`, `fork`, `mkfifo`, `waitpid`, `close`, `unlink`).

**Reference:** spec at `docs/superpowers/specs/2026-06-12-process-substitution-design.md`.

**GIT SAFETY:** Do NOT `git checkout <sha>`. Stay on `v150-process-substitution`. Commit
trailer: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

**Verified facts:**
- `WordPart` enum: `src/lexer.rs:199` (variants include `CommandSub { sequence: crate::command::Sequence, quoted: bool }`).
- Word scanner: `tokenize_partial_inner` (`src/lexer.rs:316`); the unquoted `'<'` and
  `'>'` operator arms are inside its main char loop (search for `'<' => {` and `'>' => {`).
  The `$(` branch (search `CommandSub { sequence`) shows the balanced-body capture +
  `tokenize`/`parse`-into-`Sequence` pattern to mirror.
- `Command::Subshell { body: Box<Sequence> }` (`src/command.rs:452`) wraps a `Sequence`
  for forking.
- `pub fn fork_and_run_in_subshell(cmd: &Command, shell: &mut Shell, stdin_fd: RawFd,
  stdout_fd: RawFd, stderr_fd: RawFd, pgid_target: i32, parent_fds_to_close: &[RawFd],
  stdout_dup_target: Option<i32>, stderr_dup_target: Option<i32>) -> Result<i32, io::Error>`
  (`src/executor.rs:4838`) — returns the child pid; the child `dup2`s the given fds and
  closes `parent_fds_to_close`; the parent retains its fds.
- `expand(word: &Word, shell: &mut Shell) -> Vec<Field>` (`src/expand.rs:766`); its part
  loop `match part {` is at `src/expand.rs:777` (two more `match part` sites at 1052 and
  1147 produce the joined/quoted views).
- `Shell` is defined in `src/shell_state.rs`; `Shell::new` sets `shell_pgid` via `getpgrp()`.
- Exhaustive `WordPart` matches live in `src/expand.rs`, `src/generate.rs`,
  `src/command.rs`, `src/lexer.rs`. The compiler flags every non-exhaustive one — add a
  `ProcessSub` arm to each.

---

### Task 1: AST + lexer recognition of `<(` / `>(`

**Files:**
- Modify: `src/lexer.rs` (`WordPart` enum, new `ProcDir` enum, word scanner, mod tests)
- Modify: `src/expand.rs`, `src/generate.rs`, `src/command.rs` (stub `ProcessSub` arms so it compiles)

- [ ] **Step 1: Add the AST.** In `src/lexer.rs`, add a `ProcDir` enum near `WordPart` and a new variant:
```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcDir {
    In,  // <(cmd)
    Out, // >(cmd)
}
```
and in `pub enum WordPart` (line 199), after `CommandSub { … }`:
```rust
    /// Process substitution `<(cmd)` / `>(cmd)`. Only produced when UNQUOTED
    /// (inside double/single quotes `<(`/`>(` are literal). Expands to a
    /// `/dev/fd/N` (or FIFO) path at runtime.
    ProcessSub { sequence: crate::command::Sequence, dir: ProcDir },
```

- [ ] **Step 2: Write the failing lexer tests** — add to `src/lexer.rs` `mod tests`:
```rust
#[test]
fn process_sub_in_is_a_word_part() {
    let toks = tokenize("cat <(echo hi)").unwrap();
    // Expect: word "cat", then a word whose single part is ProcessSub{In}.
    let words: Vec<&Word> = toks.iter().filter_map(|t| match t {
        Token::Word(w) => Some(w), _ => None,
    }).collect();
    assert_eq!(words.len(), 2, "cat + one process-sub word");
    match &words[1].0[..] {
        [WordPart::ProcessSub { dir: ProcDir::In, .. }] => {}
        other => panic!("expected ProcessSub In, got {other:?}"),
    }
}

#[test]
fn process_sub_out_direction() {
    let toks = tokenize("tee >(cat)").unwrap();
    let w = toks.iter().find_map(|t| match t {
        Token::Word(w) if matches!(w.0.first(), Some(WordPart::ProcessSub { .. })) => Some(w),
        _ => None,
    }).expect("a process-sub word");
    assert!(matches!(w.0[0], WordPart::ProcessSub { dir: ProcDir::Out, .. }));
}

#[test]
fn quoted_process_sub_is_literal() {
    // Inside double quotes, <( is NOT a process substitution.
    let toks = tokenize("echo \"<(echo hi)\"").unwrap();
    let has_procsub = toks.iter().any(|t| matches!(t,
        Token::Word(w) if w.0.iter().any(|p| matches!(p, WordPart::ProcessSub { .. }))));
    assert!(!has_procsub, "quoted <( must stay literal");
}

#[test]
fn lone_redirect_still_redirect() {
    // `< file` (space, no paren) and `<<EOF` must remain redirect operators.
    let toks = tokenize("cat < file").unwrap();
    assert!(toks.iter().any(|t| matches!(t, Token::Op(Operator::RedirIn))),
        "`< file` is still a redirect");
}

#[test]
fn nested_process_sub_balances() {
    let toks = tokenize("cat <(cat <(echo deep))").unwrap();
    let outer = toks.iter().find_map(|t| match t {
        Token::Word(w) if matches!(w.0.first(), Some(WordPart::ProcessSub { .. })) => Some(w),
        _ => None,
    }).expect("outer process sub");
    // The outer body itself parsed (contains the inner) — just assert it lexed without error.
    assert!(matches!(outer.0[0], WordPart::ProcessSub { dir: ProcDir::In, .. }));
}
```

- [ ] **Step 3: Run — verify failure**
`cargo test --bin huck process_sub 2>&1 | tail -20` → compile error / FAIL (variant or
lexing missing). Record.

- [ ] **Step 4: Implement lexer recognition.** In `tokenize_partial_inner`
(`src/lexer.rs:316`), at the unquoted `'<'` and `'>'` operator arms, BEFORE the existing
redirect handling, peek for `(`:
  - For `'<'`: if `chars.peek() == Some(&'(')`, this is `<(` → consume the `(`, capture the
    balanced inner body (reuse the SAME balanced-paren capture + `tokenize`/`command::parse`
    into a `Sequence` that the `$(` branch uses — locate the `CommandSub { sequence` build
    site and mirror its body scanner; it already balances nested `()`, quotes, `$()`,
    backticks up to the matching `)`), then push
    `WordPart::ProcessSub { sequence, dir: ProcDir::In }` onto the CURRENT word's parts and
    `continue` the loop (do NOT flush the word or emit `Operator::RedirIn`).
  - For `'>'`: same with `ProcDir::Out`.
  - Otherwise (`peek != '('`), fall through to the existing redirect-operator handling
    (untouched — `<<`, `<<<`, `<&`, `>>`, `&>`, `2>`, plain `<`/`>` etc. behave exactly as
    before; the multi-char operators are matched before the bare `<`/`>` already, so only a
    bare `<`/`>` immediately followed by `(` is diverted).
  - This is reached only in the UNQUOTED scanner state, so the quoted-literal requirement is
    satisfied automatically (double/single-quote handling never calls this arm).

- [ ] **Step 5: Add stub `ProcessSub` arms so the crate compiles.** The compiler will flag
non-exhaustive `match`es. Add minimal arms:
  - `src/expand.rs` (the three `match part` sites at 777 / 1052 / 1147): for now,
    `WordPart::ProcessSub { .. } => { /* realized in Task 3 */ }` producing NO output
    (push nothing). Add a `// TODO(Task 3)` only if a value is required by the arm's type;
    prefer an explicit empty/no-op that type-checks.
  - `src/generate.rs`: render as source — `WordPart::ProcessSub { sequence, dir }` →
    `<(` or `>(` + the generated inner + `)` (full impl lands in Task 5; a correct render
    here is fine to write now).
  - `src/command.rs`: any `WordPart` match (e.g. `try_split_assignment`, reconstruction) —
    a pass-through arm that treats `ProcessSub` like `CommandSub` (it is not an assignment
    prefix and is not split).

- [ ] **Step 6: Build + test**
`cargo build 2>&1 | tail -5` → clean.
`cargo test --bin huck process_sub 2>&1 | tail -15` → the 5 lexer tests pass.
`cargo test 2>&1 | grep -E "test result: FAILED|[1-9][0-9]* failed|error\[" | head || echo NONE` → NONE.

- [ ] **Step 7: Commit**
```bash
git add src/lexer.rs src/expand.rs src/generate.rs src/command.rs
git commit -m "$(printf 'feat: lex unquoted <(...) / >(...) as WordPart::ProcessSub\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 2: `procsub.rs` — FD realization + fork + cleanup

**Files:**
- Create: `src/procsub.rs`
- Modify: `src/main.rs` (or wherever modules are declared — add `mod procsub;`)
- Modify: `src/shell_state.rs` (`Shell.procsub_pending` field + helper)

- [ ] **Step 1: Add the `Shell` field.** In `src/shell_state.rs`, add to `struct Shell`:
```rust
    /// Live process substitutions whose inner process + fd must be cleaned up
    /// after the current command (see src/procsub.rs). Snapshot/drained by the
    /// executor around each command.
    pub procsub_pending: Vec<crate::procsub::ProcSub>,
```
initialize `procsub_pending: Vec::new(),` in `Shell::new` (and any other `Shell`
constructor / test builder the compiler flags).

- [ ] **Step 2: Write `src/procsub.rs`** with the record, runtime `/dev/fd` detection,
`realize`, and `cleanup`:
```rust
//! Process substitution `<(cmd)` / `>(cmd)` runtime support (v150).
//!
//! `realize` creates a pipe (or FIFO fallback), forks the inner command via
//! `fork_and_run_in_subshell`, and returns the `/dev/fd/N` (or FIFO) path plus a
//! `ProcSub` cleanup record. `cleanup` closes the parent fd, unlinks any FIFO, and
//! reaps the inner pid. POSIX-only; macOS-portable (no `/proc`).

use std::io;
use std::os::unix::io::RawFd;
use std::path::PathBuf;
use crate::lexer::ProcDir;
use crate::command::{Command, Sequence};
use crate::shell_state::Shell;

#[derive(Debug)]
pub struct ProcSub {
    pub pid: i32,
    pub parent_fd: RawFd,
    pub fifo_path: Option<PathBuf>,
}

/// `/dev/fd` is a directory on Linux (→ /proc/self/fd) and macOS (fdescfs). Checked
/// once and cached. We only ever name `/dev/fd/N`; never `/proc`.
fn dev_fd_available() -> bool {
    use std::sync::OnceLock;
    static OK: OnceLock<bool> = OnceLock::new();
    *OK.get_or_init(|| std::path::Path::new("/dev/fd").is_dir())
}

/// Realize one process substitution. Returns the path string to substitute into the
/// word, plus the cleanup record (also pushed onto `shell.procsub_pending` by the caller).
pub fn realize(seq: &Sequence, dir: ProcDir, shell: &mut Shell) -> io::Result<(String, ProcSub)> {
    let mut fds = [0 as RawFd; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let (read_fd, write_fd) = (fds[0], fds[1]);

    // Which end the PARENT keeps and which fd the INNER gets on 0/1.
    let (parent_fd, inner_stdin, inner_stdout, child_closes) = match dir {
        // <(cmd): parent reads cmd's stdout; inner writes to the pipe on fd 1.
        ProcDir::In  => (read_fd,  libc::STDIN_FILENO, write_fd, read_fd),
        // >(cmd): parent writes cmd's stdin; inner reads from the pipe on fd 0.
        ProcDir::Out => (write_fd, read_fd, libc::STDOUT_FILENO, write_fd),
    };

    // Fork the inner sequence as a subshell. pgid_target = the shell's group so the
    // procsub child is NOT a foreground job and the terminal is never handed to it
    // (avoids the SIGTTOU / terminal-handoff deadlocks of v108/v124). No give_terminal_to.
    let inner = Command::Subshell { body: Box::new(seq.clone()) };
    let child_close_list = [child_closes];
    let pid = crate::executor::fork_and_run_in_subshell(
        &inner, shell,
        inner_stdin, inner_stdout, libc::STDERR_FILENO,
        shell.shell_pgid, &child_close_list, None, None,
    )?;

    // Parent closes the end the inner owns (the inner has its own copy via dup2).
    let inner_end = match dir { ProcDir::In => write_fd, ProcDir::Out => read_fd };
    unsafe { libc::close(inner_end); }

    // /dev/fd path (FIFO fallback only if /dev/fd is unavailable — rare on Linux/macOS).
    let path = format!("/dev/fd/{parent_fd}");
    let _ = dev_fd_available(); // (FIFO fallback path added below if needed)
    Ok((path, ProcSub { pid, parent_fd, fifo_path: None }))
}

/// Tear down one realized process substitution: close the parent fd, unlink any FIFO,
/// reap the inner pid (it has finished once the pipe is closed).
pub fn cleanup(ps: ProcSub) {
    unsafe { libc::close(ps.parent_fd); }
    if let Some(p) = &ps.fifo_path {
        let _ = std::fs::remove_file(p);
    }
    let mut status = 0;
    unsafe { libc::waitpid(ps.pid, &mut status, 0); }
}
```
> **FIFO fallback note for the implementer:** the `/dev/fd`-available case is the
> default and the only path exercised on Linux/macOS dev boxes. If `dev_fd_available()`
> returns `false`, replace the pipe with a `mkfifo` under `std::env::var("TMPDIR")`
> (default `/tmp`) named uniquely (`format!("huck-procsub-{}-{}", getpid, counter)`),
> set `fifo_path = Some(path)`, fork the inner opening the FIFO on its end, and return
> the FIFO path instead of `/dev/fd/N`. Keep this branch isolated in `realize`. Because
> it cannot be exercised where `/dev/fd` exists, it is verified by code review, not a test.

- [ ] **Step 3: Declare the module.** Add `mod procsub;` alongside the other `mod` lines
in `src/main.rs` (match the existing module-declaration style/visibility).

- [ ] **Step 4: Write an integration test** — create `tests/process_sub.rs`:
```rust
// End-to-end: a <(echo hi) realized fd yields "hi"; cleanup reaps without zombies.
// Driven through the binary so the full lex→expand→exec path is exercised.
use std::process::Command;

fn huck(script: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_huck"))
        .args(["-c", script]).output().expect("run huck");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn cat_input_process_sub() {
    assert_eq!(huck("cat <(echo hi)"), "hi\n");
}
```
(This test will FAIL until Tasks 3–4 wire expansion + execution; that is expected — it is
the payoff gate. Mark it `#[ignore]` now with a comment, then un-ignore in Task 4.)

- [ ] **Step 5: Build + targeted test**
`cargo build 2>&1 | tail -5` → clean.
`cargo test 2>&1 | grep -E "test result: FAILED|[1-9][0-9]* failed|error\[" | head || echo NONE` → NONE (the payoff test is `#[ignore]`d).

- [ ] **Step 6: Commit**
```bash
git add src/procsub.rs src/shell_state.rs src/main.rs tests/process_sub.rs
git commit -m "$(printf 'feat: procsub module — realize /dev/fd path + fork inner + cleanup\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 3: Expansion — wire `ProcessSub` through `expand()`

**Files:**
- Modify: `src/expand.rs` (the `ProcessSub` arm at the `match part` site `:777`)

- [ ] **Step 1: Implement the expansion arm.** Replace the Task-1 stub at
`src/expand.rs:777`'s `match part` with the real behavior: realize the process sub, push
the cleanup record onto `shell.procsub_pending`, and emit the path as a SINGLE field
(no IFS split, no glob). Mirror how the arm builds a `Field`:
```rust
WordPart::ProcessSub { sequence, dir } => {
    match crate::procsub::realize(sequence, dir.clone(), shell) {
        Ok((path, ps)) => {
            shell.procsub_pending.push(ps);
            // Single, non-split, non-glob field segment (like a quoted literal).
            // Use the same Field-append the Literal{quoted:true} arm uses.
            push_literal_segment(&path); // adapt to the local field-builder
        }
        Err(e) => {
            eprintln!("huck: process substitution: {e}");
            // leave no field; the command will see a missing argument
        }
    }
}
```
> Adapt `push_literal_segment` to whatever the surrounding arm uses to append a
> non-splitting literal segment to the in-progress field (look at the
> `WordPart::Literal { quoted: true, .. }` arm in the same `match`). The two view-only
> matches at `:1052` and `:1147` (joined / quoted-mask) should treat `ProcessSub` like a
> non-splitting literal — for `:1147` (`is_quoted`-style) return `true` (do not split).

- [ ] **Step 2: Build**
`cargo build 2>&1 | tail -5` → clean.

- [ ] **Step 3: Commit** (execution lifecycle lands in Task 4)
```bash
git add src/expand.rs
git commit -m "$(printf 'feat: expand ProcessSub to a /dev/fd path + record pending cleanup\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 4: Executor lifecycle — snapshot/drain (the payoff)

**Files:**
- Modify: `src/executor.rs` (`run_exec_single` `:2980`; `with_redirect_scope` `:582` / `run_redirected` `:699`)
- Modify: `tests/process_sub.rs` (un-ignore the payoff test, add more)

- [ ] **Step 1: Add a drain helper.** In `src/executor.rs` (or `Shell`):
```rust
fn drain_procsubs(shell: &mut Shell, base: usize) {
    while shell.procsub_pending.len() > base {
        if let Some(ps) = shell.procsub_pending.pop() {
            crate::procsub::cleanup(ps);
        }
    }
}
```

- [ ] **Step 2: Snapshot/drain in `run_exec_single`.** At the top of `run_exec_single`
(`:2980`), BEFORE the command's argument words are expanded, capture
`let procsub_base = shell.procsub_pending.len();`. After the command has been dispatched
(every return path — external/builtin/function), call `drain_procsubs(shell, procsub_base)`.
Easiest correct shape: compute the `ExecOutcome` into a local, `drain_procsubs(shell,
procsub_base)`, then return the local. Ensure EARLY returns also drain (wrap the body so
the drain runs on all paths — e.g. a small inner closure/`let outcome = (||{…})();` then
drain then return, or a scope-guard struct).

- [ ] **Step 3: Snapshot/drain around redirect-target expansion.** In `with_redirect_scope`
(`:582`) / `run_redirected` (`:699`), capture `let procsub_base = shell.procsub_pending.len();`
BEFORE the redirect target words are expanded/opened, and `drain_procsubs(shell,
procsub_base)` AFTER the wrapped command returns (so a `< <(cmd)` redirect-target procsub
outlives the command, then is cleaned up). The two layers nest: `run_exec_single` drains
the argument procsubs with the command; the redirect layer drains the redirect-target
procsubs after. Each drains only `[its own base..]`.

- [ ] **Step 4: Un-ignore + expand the payoff tests** in `tests/process_sub.rs`:
```rust
#[test] fn cat_input_process_sub() { assert_eq!(huck("cat <(echo hi)"), "hi\n"); }

#[test] fn two_input_process_subs() { assert_eq!(huck("cat <(echo a) <(echo b)"), "a\nb\n"); }

#[test] fn redirect_source_process_sub() {
    assert_eq!(huck("wc -c < <(printf hello)"), "5\n".to_string());
}

#[test] fn while_read_from_process_sub() {
    assert_eq!(huck("while read x; do echo \"[$x]\"; done < <(seq 3)"), "[1]\n[2]\n[3]\n");
}

#[test] fn output_process_sub_tee() {
    // tee writes to >(cat); the inner cat echoes the line to stdout.
    assert_eq!(huck("printf 'foo\\n' | tee >(cat) >/dev/null"), "foo\n");
}

#[test] fn nested_process_sub() { assert_eq!(huck("cat <(cat <(echo deep))"), "deep\n"); }

#[test] fn quoted_is_literal() {
    assert_eq!(huck("echo \"<(echo hi)\""), "<(echo hi)\n");
}
```

- [ ] **Step 5: Build + test**
`cargo build 2>&1 | tail -5` → clean.
`cargo test --test process_sub 2>&1 | tail -20` → all pass.
`cargo test 2>&1 | grep -E "test result: FAILED|[1-9][0-9]* failed|error\[" | head || echo NONE` → NONE.
Zombie check: `target/debug/huck -c 'cat <(echo hi) >/dev/null; ps -o stat= -p $$' ` should
show no `<defunct>` children (manual sanity).

- [ ] **Step 6: Commit**
```bash
git add src/executor.rs tests/process_sub.rs
git commit -m "$(printf 'feat: run + clean up process substitutions around each command\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 5: `generate.rs` — render `ProcessSub` back to source

**Files:**
- Modify: `src/generate.rs` (the `WordPart` render match)
- Test: `src/generate.rs` mod tests (or wherever generate round-trip tests live)

- [ ] **Step 1: Write the failing test** — a function body containing a process sub must
`declare -f`/serialize back to `<(…)` / `>(…)`:
```rust
#[test]
fn renders_process_substitution() {
    // f() { diff <(echo a) >(cat); }  round-trips with both directions.
    let src = "f() { cat <(echo a); }";
    let seq = crate::command::parse(crate::lexer::tokenize(src).unwrap()).unwrap().unwrap();
    let rendered = /* render seq.first (the FunctionDef body) via generate */;
    assert!(rendered.contains("<(echo a)"), "got: {rendered}");
}
```
(Adapt to `generate.rs`'s actual entry point — mirror an existing `CommandSub`
round-trip test in that file.)

- [ ] **Step 2: Implement** (if not already correct from Task 1's stub). In the
`WordPart` render match:
```rust
WordPart::ProcessSub { sequence, dir } => {
    out.push_str(match dir { ProcDir::In => "<(", ProcDir::Out => ">(" });
    out.push_str(&command_to_source(/* sequence as the cmdsub body is rendered */));
    out.push(')');
}
```
Mirror exactly how the `CommandSub { sequence, .. }` arm renders its inner sequence
(same helper, no surrounding `$`).

- [ ] **Step 3: Build + test**
`cargo test --bin huck renders_process_substitution 2>&1 | tail -8` → pass.

- [ ] **Step 4: Commit**
```bash
git add src/generate.rs
git commit -m "$(printf 'feat: render ProcessSub <(...) / >(...) in generate (declare -f)\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 6: bash-diff harness + full regression

**Files:**
- Create: `tests/scripts/process_sub_diff_check.sh`

- [ ] **Step 1: Write the harness** (content-consuming cases only — never assert the
literal `/dev/fd/N`, whose number differs between shells):
```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v150: process substitution <(...) / >(...).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1; echo "rc=$?")
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
check "cat input"        'cat <(echo hi)'
check "two inputs"       'cat <(echo a) <(echo b)'
check "diff"             'diff <(printf "a\nb\n") <(printf "a\nc\n"); echo "rc=$?"'
check "comm"             'comm -12 <(printf "a\nb\nc\n") <(printf "b\nc\nd\n")'
check "redirect source"  'wc -c < <(printf hello)'
check "while read"       'while read x; do echo "[$x]"; done < <(seq 3)'
check "tee output"       'printf "foo\n" | tee >(cat) >/dev/null'
check "nested"           'cat <(cat <(echo deep))'
check "quoted literal"   'echo "<(echo hi)"'
check "paste"            'paste <(seq 2) <(seq 2)'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 2: Run the harness**
`chmod +x tests/scripts/process_sub_diff_check.sh && cargo build 2>&1 | tail -2 && bash tests/scripts/process_sub_diff_check.sh` → all PASS.

- [ ] **Step 3: Full regression + clippy**
`cargo test 2>&1 | grep -E "test result: FAILED|[1-9][0-9]* failed|error\[" | head || echo NONE` → NONE.
`for f in tests/scripts/*_diff_check.sh; do bash "$f" >/dev/null 2>&1 || echo "FAIL: $f"; done; echo done` → only `done` (no FAIL lines).
`cargo clippy --all-targets 2>&1 | grep -E "^warning|^error" | head || echo CLEAN` → CLEAN.

- [ ] **Step 4: Commit**
```bash
git add tests/scripts/process_sub_diff_check.sh
git commit -m "$(printf 'test: bash-diff harness for process substitution\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Notes for the implementer
- **macOS portability:** only POSIX `libc` calls; runtime `/dev/fd` detection (no `/proc`);
  `mkfifo` fallback. Guard any platform-specific constant with `#[cfg(target_os = …)]`.
- **No terminal handoff for procsub children:** fork with `pgid_target = shell.shell_pgid`
  and never call a give-terminal routine — process subs are background helpers, and huck
  has a history of tty-handoff deadlocks (v108/v124) when a non-foreground child touches
  the terminal.
- **Quoting gate is automatic:** `ProcessSub` is only produced from the unquoted scanner
  state, so no extra quoted-context check is needed at expansion time.
- **Drain on every path:** the `run_exec_single` drain must run even on early returns —
  use a scope guard or compute-then-drain-then-return so no procsub leaks an fd/zombie.
- **Single field:** the realized path is never IFS-split or globbed (treat like a quoted
  literal segment).
- **Git safety:** stay on `v150-process-substitution`; do NOT `git checkout <sha>`.
