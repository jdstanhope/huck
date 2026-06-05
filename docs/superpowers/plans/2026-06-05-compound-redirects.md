# Redirections on Compound Commands Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Support a redirect attached to any compound command — `while/until/for … done <REDIR`, `if … fi <REDIR`, `case … esac <REDIR`, `{ …; } <REDIR`, `( … ) <REDIR`, `select … done`, `(( … ))`, `[[ … ]]` — with all redirect types. Unblocks nvm.sh (`done <<EOF`).

**Architecture:** A `Command::Redirected { inner, stdin, stdout, stderr }` wrapper (mirroring `SimpleCommand`'s 3-slot redirect model). The parser factors a `parse_trailing_redirects` helper from the simple-command loop and wraps a compound when redirects follow its terminator. The executor applies the redirects at the real fd level (dup2 save/restore around the inner command), reusing the existing heredoc/file/dup helpers and guard pattern.

**Tech Stack:** Rust (binary crate `huck`). Unit `cargo test --bin huck`; integration `cargo test --test <name>`; bash-diff harness under `tests/scripts/`.

---

## File Structure

- `src/command.rs` — `Command::Redirected` variant; `parse_trailing_redirects` (factored from `parse_simple_stage`); `maybe_wrap_redirects` applied to each compound arm in `parse_command_inner`.
- `src/executor.rs` — `Redirected` arm in `run_command`; `CompoundRedirectScope` fd-guard (dup2 save/restore for fds 0/1/2; heredoc/file/dup), `io::stdout()` flush.
- `tests/compound_redirects_integration.rs`, `tests/scripts/compound_redirects_diff_check.sh` — NEW.
- `docs/bash-divergences.md`, `README.md` — new Tier-2 entry + changelog + README row.

---

### Task 1: Compound-command redirections (parser + executor, end-to-end)

Coupled change; implement together (the feature isn't testable until both land). TDD: integration test first.

**Files:** `src/command.rs`, `src/executor.rs`

- [ ] **Step 1: Write the failing integration test**

Create `tests/compound_redirects_integration.rs`:

```rust
//! v97: redirections on compound commands.
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

fn run(script: &str) -> (String, i32) {
    let mut child = Command::new(huck_bin())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn huck");
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(), out.status.code().unwrap_or(-1))
}

#[test]
fn heredoc_on_while_done() {
    assert_eq!(run("while read x; do echo \"g:$x\"; done <<EOF\na\nb\nEOF\n").0, "g:a\ng:b\n");
}

#[test]
fn redirect_out_on_for() {
    assert_eq!(run("for i in 1 2; do echo $i; done > /tmp/huck_t_for\ncat /tmp/huck_t_for\n").0, "1\n2\n");
}

#[test]
fn redirect_out_on_if() {
    assert_eq!(run("if true; then echo hi; fi > /tmp/huck_t_if\ncat /tmp/huck_t_if\n").0, "hi\n");
}

#[test]
fn redirect_out_on_brace_group() {
    assert_eq!(run("{ echo a; echo b; } > /tmp/huck_t_bg\ncat /tmp/huck_t_bg\n").0, "a\nb\n");
}

#[test]
fn redirect_out_on_subshell() {
    assert_eq!(run("( echo x ) > /tmp/huck_t_ss\ncat /tmp/huck_t_ss\n").0, "x\n");
}

#[test]
fn redirect_out_on_case() {
    assert_eq!(run("case z in z) echo m;; esac > /tmp/huck_t_case\ncat /tmp/huck_t_case\n").0, "m\n");
}

#[test]
fn herestring_on_while() {
    assert_eq!(run("while read x; do echo \"[$x]\"; done <<< 'one two'\n").0, "[one two]\n");
}

#[test]
fn append_on_brace_group() {
    assert_eq!(run("echo first > /tmp/huck_t_ap\n{ echo second; } >> /tmp/huck_t_ap\ncat /tmp/huck_t_ap\n").0,
               "first\nsecond\n");
}

#[test]
fn stderr_redirect_on_compound() {
    // 2>&1 then capture: error inside a group goes to the redirected stdout file.
    assert_eq!(run("{ echo out; echo err >&2; } > /tmp/huck_t_se 2>&1\ncat /tmp/huck_t_se\n").0, "out\nerr\n");
}

#[test]
fn no_redirect_compound_unchanged() {
    // Regression: a bare compound still works (not wrapped).
    assert_eq!(run("for i in 1 2; do echo $i; done\n").0, "1\n2\n");
}

#[test]
fn capture_with_inner_redirect() {
    // A >file inside a captured compound diverts that line to the file.
    assert_eq!(run("x=$({ echo a; echo b > /tmp/huck_t_cap; }); echo \"[$x]\"\ncat /tmp/huck_t_cap\n").0,
               "[a]\nb\n");
}
```

(The `/tmp/huck_t_*` paths keep tests independent; if parallel-test collisions are a concern, switch to `tempfile` per the integration-test convention — but distinct filenames per test are fine here.)

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test --test compound_redirects_integration 2>&1 | tail -20`
Expected: FAIL — `done <<EOF` etc. currently error `unexpected token after command`.

- [ ] **Step 3: Add the `Command::Redirected` variant**

In `src/command.rs`, `pub enum Command` (`:426`):
```rust
    /// A compound command with trailing redirections applied to its whole
    /// execution: `{ …; } >f`, `while … done <<EOF`, etc. (v97)
    Redirected {
        inner: Box<Command>,
        stdin: Option<Redirect>,
        stdout: Option<Redirect>,
        stderr: Option<Redirect>,
    },
```

- [ ] **Step 4: Factor `parse_trailing_redirects` from `parse_simple_stage`**

The redirect-handling arms in `parse_simple_stage` (`src/command.rs:~1640-1665`: the `Token::Heredoc` arm + the `Token::Op(redir-op)` → target → slot assignment) become a standalone helper:
```rust
type RedirSlots = (Option<Redirect>, Option<Redirect>, Option<Redirect>, bool);

/// Consumes a run of trailing redirect tokens (`<`, `>`, `>>`, `2>`, `2>>`,
/// `<<<`, `>&`, `2>&`, and `Token::Heredoc`) from `iter`, filling the
/// stdin/stdout/stderr slots (last-wins). Stops at the first non-redirect
/// token. The `bool` is true iff at least one redirect was consumed.
fn parse_trailing_redirects<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<RedirSlots, ParseError> {
    let mut stdin = None; let mut stdout = None; let mut stderr = None; let mut saw = false;
    loop {
        match iter.peek() {
            Some(Token::Heredoc { .. }) => {
                let Some(Token::Heredoc { body, expand, strip_tabs }) = iter.next() else { unreachable!() };
                stdin = Some(Redirect::Heredoc { body, expand, strip_tabs }); saw = true;
            }
            Some(Token::Op(op)) if is_redirect_op(op) => {
                let op = *op; iter.next();
                let target = match iter.next() {
                    Some(Token::Word(w)) => w,
                    Some(Token::Op(_)) | Some(Token::Heredoc { .. }) | Some(Token::ArithBlock(_)) =>
                        return Err(ParseError::RedirectTargetIsOperator),
                    Some(Token::Newline) | None => return Err(ParseError::MissingRedirectTarget),
                };
                match op { /* the SAME match arms currently in parse_simple_stage:
                    RedirIn=>stdin=Read, RedirOut=>stdout=Truncate, RedirAppend=>stdout=Append,
                    RedirErr=>stderr=Truncate, RedirErrAppend=>stderr=Append, HereString=>stdin=HereString,
                    DupOut=>stdout=Dup{fd:1}, DupErr=>stderr=Dup{fd:2} */ }
                saw = true;
            }
            _ => break,
        }
    }
    Ok((stdin, stdout, stderr, saw))
}
```
Add an `is_redirect_op(op: &Operator) -> bool` matching exactly the redirect operators handled above (NOT `Pipe`/`LParen`/`RParen`). Then **refactor `parse_simple_stage` to use this helper** for its redirect tokens (so simple-command behavior is byte-identical — move the heredoc/op arms to delegate, or inline-call the helper at the right point). Verify simple-command redirect tests still pass after the refactor.

- [ ] **Step 5: Wrap compounds in `parse_command_inner`**

Add:
```rust
fn maybe_wrap_redirects<I: Iterator<Item = Token>>(
    cmd: Command, iter: &mut std::iter::Peekable<I>,
) -> Result<Command, ParseError> {
    let (stdin, stdout, stderr, saw) = parse_trailing_redirects(iter)?;
    if saw {
        Ok(Command::Redirected { inner: Box::new(cmd), stdin, stdout, stderr })
    } else {
        Ok(cmd)
    }
}
```
Apply it to each COMPOUND arm in `parse_command_inner` (`src/command.rs:~738`):
- `If` → `maybe_wrap_redirects(Command::If(Box::new(parse_if(iter)?)), iter)`
- `While`/`Until` → wrap the `Command::While(...)`
- `For` → wrap the result of `parse_for_command(iter)?`
- `Select` → wrap the result of `parse_select_command(iter)?`
- `Case` → wrap the `Command::Case(...)`
- `LBrace` → wrap the `Command::BraceGroup(...)`
- `DoubleBracketOpen` → wrap the result of `parse_double_bracket(iter)?`
- the bare-`(` subshell path (`return parse_subshell(iter)`) → wrap its result
- the standalone arith block (`Command::Arith(...)`, returned early near `:735`) → wrap it
- Do NOT wrap function definitions or the simple-command/pipeline fall-through (simple commands consume their own redirects via `parse_simple_stage`).

(Mechanically: replace `Ok(Command::X(...))` with `maybe_wrap_redirects(Command::X(...), iter)`, and `return parse_subshell(iter)` with `return maybe_wrap_redirects(parse_subshell(iter)?, iter)`, etc.)

- [ ] **Step 6: Add parser unit tests**

In the command.rs test module: assert `{ echo a; } > f` parses to `Command::Redirected { inner: BraceGroup, stdout: Some(Truncate), .. }`; `while …; done <<EOF…` → `Redirected { inner: While, stdin: Some(Heredoc), .. }`; a bare `for …; done` → plain `Command::For` (NOT wrapped). Mirror neighboring parser-test helpers.

- [ ] **Step 7: Add the executor `Redirected` arm + `CompoundRedirectScope`**

In `src/executor.rs`, add to `run_command`:
```rust
        Command::Redirected { inner, stdin, stdout, stderr } =>
            run_redirected(inner, stdin, stdout, stderr, shell, sink),
```
Implement `run_redirected`: build a `CompoundRedirectScope` that applies each present redirect by `dup2`-ing onto fds 0/1/2 and saving the originals, run `inner` via `run_command`, then restore on drop. Reuse the existing helpers:
- stdin heredoc/here-string → the existing write-body-to-pipe + dup2→fd0 logic (`src/executor.rs:~1838`, `write_pipe_for_stdin`); `<file` → open read + dup2→fd0.
- stdout/stderr `>`/`>>` → `open_resolved`-style open (trunc/append) + dup2 → fd1/fd2; `>&N`/`2>&N` → resolve source fd + dup2.
- Save replaced fds; `impl Drop` restores `dup2(saved, fd)` + close.
- **Flush `io::stdout()`** before applying redirects and again before restoring (so buffered `Terminal`/builtin output lands correctly across the swap).
- On a redirect-open failure, print `huck: <target>: <err>` and return `ExecOutcome::Continue(1)` WITHOUT running `inner` (mirror the simple-command failure path).

The inner command runs with the existing `sink`; in `Terminal` mode its output (and any external child's) flows through the now-redirected fds. Status returned = inner's status.

- [ ] **Step 8: Build + run integration tests + full suite + clippy**

Run: `cargo build --bin huck && cargo test --test compound_redirects_integration 2>&1 | tail -25` (all pass).
Run: `cargo test --bin huck 2>&1 | tail -5` and `cargo test 2>&1 | grep -E 'test result' | grep -v 'ok\.' | head` (no failures — especially the existing redirect/pipeline/heredoc tests after the Step 4 refactor).
Run: `cargo clippy --all-targets 2>&1 | tail -3` (clean).

- [ ] **Step 9: Commit**

```bash
git add src/command.rs src/executor.rs tests/compound_redirects_integration.rs
git commit -m "feat: redirections on compound commands (while/if/for/case/{ }/subshell …)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```
Trailer mandatory/canonical, exactly as shown.

---

### Task 2: bash-diff harness (22nd)

**Files:** `tests/scripts/compound_redirects_diff_check.sh` (NEW)

- [ ] **Step 1: Create the harness**

Mirror `tests/scripts/dbracket_multiline_diff_check.sh`'s `check` helper + a `mktemp -d` fixture dir (`FIX`) for file targets (like the `_nt`/`_ot` harness does). Fragments (each producing deterministic stdout — write to `$FIX/x` then `cat`):
```bash
FIX="$(mktemp -d)"; trap 'rm -rf "$FIX"' EXIT
check "heredoc while" $'while read x; do echo "g:$x"; done <<EOF\na\nb\nEOF'
check "herestring while" "while read x; do echo \"[\$x]\"; done <<< 'one two'"
check "for >file"   "for i in 1 2; do echo \$i; done > '$FIX/a'; cat '$FIX/a'"
check "if >file"    "if true; then echo hi; fi > '$FIX/b'; cat '$FIX/b'"
check "brace >file"  "{ echo a; echo b; } > '$FIX/c'; cat '$FIX/c'"
check "subshell >file" "( echo x ) > '$FIX/d'; cat '$FIX/d'"
check "case >file"  "case z in z) echo m;; esac > '$FIX/e'; cat '$FIX/e'"
check "append brace" "echo first > '$FIX/f'; { echo second; } >> '$FIX/f'; cat '$FIX/f'"
check "stderr group" "{ echo out; echo err >&2; } > '$FIX/g' 2>&1; cat '$FIX/g'"
check "no-redir for" "for i in 1 2; do echo \$i; done"
```

- [ ] **Step 2: Run the harness**

Run: `cargo build --bin huck && bash tests/scripts/compound_redirects_diff_check.sh 2>&1 | tail -25`
Expected: every line PASS, `Fail: 0`. If any FAIL, bash is the oracle — investigate; if it's a real Task 1 bug, STOP and report (don't mask). Note: the `2>&1` ordering on a group can differ if buffering isn't flushed — if `stderr group` diverges, it likely indicates a missing `io::stdout()` flush in Task 1 (fix there, re-run).

- [ ] **Step 3: Commit**

```bash
git add tests/scripts/compound_redirects_diff_check.sh
git commit -m "test: bash-diff harness for compound-command redirections (22nd)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```
Trailer mandatory/canonical, exactly as shown.

---

### Task 3: Documentation

**Files:** `docs/bash-divergences.md`, `README.md`

- [ ] **Step 1: Read structure**

`grep -n 'redirect\|compound\|^## Change log\|Missing features (Tier 2)\|2026-06-05' docs/bash-divergences.md | head -20` and `grep -n '| v9' README.md`. Match v95/v96 entry style and find the next free `M-` number.

- [ ] **Step 2: Add the Tier-2 entry**

Add a new Missing-features (Tier 2) entry (next free `M-` number) `[fixed v97]`: redirections on compound commands — `while/until/for … done`, `if … fi`, `case … esac`, `{ …; }`, `( … )`, `select`, arith-`for`, `(( ))`, `[[ ]]` — with all redirect types, via a `Command::Redirected` wrapper + `parse_trailing_redirects` + an fd-level executor scope. Note the trigger: nvm.sh `done <<EOF` (a heredoc on a `while` loop), and that before this, redirects worked only on simple commands. Bump the Tier-2 count.

- [ ] **Step 3: Change-log + README row**

Add a `2026-06-05` v97 change-log entry mirroring v95/v96 style (the wrapper + helper + fd scope; 22nd harness `compound_redirects_diff_check.sh`; unblocks nvm.sh; note the `N>file` general-fd + process-substitution out-of-scope items). Add a v97 README row after v96.

- [ ] **Step 4: Verify + commit**

`grep -n 'v97\|fixed v97\|compound' docs/bash-divergences.md README.md` (confirm, no placeholders).
```bash
git add docs/bash-divergences.md README.md
git commit -m "docs: v97 compound-command redirections — Tier-2 entry, changelog, README

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```
Trailer mandatory/canonical, exactly as shown.

---

## Self-Review

- **Spec coverage:** §1 AST + §2 parser + §3 executor → Task 1; testing → Tasks 1/2; new Tier-2 entry → Task 3. Covered.
- **Placeholder scan:** none — `parse_trailing_redirects` body is shown (the op→slot match references the EXACT arms already in `parse_simple_stage`, which the implementer copies verbatim); the executor `run_redirected`/`CompoundRedirectScope` is specified against the named existing helpers (it requires real fd code, deliberately built on `write_pipe_for_stdin`/`open_resolved`/the `BuiltinStdinGuard` dup2 pattern rather than pseudo-code).
- **Type consistency:** `Command::Redirected { inner: Box<Command>, stdin/stdout/stderr: Option<Redirect> }`; `parse_trailing_redirects -> (Option<Redirect>×3, bool)`; `maybe_wrap_redirects(Command, iter) -> Result<Command>`; reuses `Redirect`, `Token::Heredoc`, `write_pipe_for_stdin`, `open_resolved`, `BuiltinStdinGuard`-style dup2.
- **Edge cases:** no-redirect compound stays unwrapped (regression-tested); capture+inner-redirect tested; `io::stdout()` flush for buffer/fd-swap ordering; redirect-open failure → status 1, inner skipped; last-wins per fd; general `N>file` + process-sub explicitly out of scope.
