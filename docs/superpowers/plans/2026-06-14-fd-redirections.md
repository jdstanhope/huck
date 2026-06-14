# Arbitrary-fd (fd > 2) Redirections Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace huck's fixed `stdin`/`stdout`/`stderr` redirect slots with one ordered, fd-tagged redirect list, enabling `N>`, `N<`, `N>>`, `N>|`, `N<>`, `N>&M`, `N<&M`, `N>&-`, `N<&-`, and `{var}` named-fd redirections.

**Architecture:** A new parse-side `Redirection { fd: RedirFd, op: RedirOp }` ordered list replaces the three `Option<Redirect>` slots on `ExecCommand`/`Redirected`. A temporary `legacy_slots()` bridge keeps the executor running on fds 0/1/2 (behavior-identical) while the front-end lands; the executor is then migrated path-by-path to an ordered applier (`RedirectScope` in-process, `pre_exec` replay for forks) that handles arbitrary fds, `{var}` allocation, and close. The bridge and old `Redirect` enum are deleted last. Source order is preserved (fixes L-08).

**Tech Stack:** Rust, `libc` (`dup2`, `close`, `fcntl(F_DUPFD_CLOEXEC)`, `open` via `std::fs`).

**Spec:** `docs/superpowers/specs/2026-06-14-fd-redirections-design.md`. Read it first.

**Conventions:** Commit trailer `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. NEVER `git checkout <sha>` (detaches HEAD, orphans commits). Run the FULL suite (`cargo test`) + clippy + the redirect harnesses after every task — this refactor's safety net. Branch: `v156-fd-redirections`.

**Migration invariant:** after EVERY task the build compiles, `cargo test` is fully green, and the existing redirect harnesses (`compound_redirects_diff_check.sh`, `function_redirect_diff_check.sh`, `assign_redirect_diff_check.sh`, `exec_diff_check.sh`) stay byte-identical. New fd>2 behavior is added incrementally; nothing regresses.

---

## File map

- `src/command.rs` — new `Redirection`/`RedirFd`/`RedirOp`/`FileMode` types; `ExecCommand`/`Redirected` carry `redirects: Vec<Redirection>`; parser builds the ordered list; `legacy_slots()` bridge. The old `Redirect` enum is kept until Task 8, then deleted.
- `src/lexer.rs` — fd-prefix detection; `Token::RedirFd(RedirFd)`; new `Operator::DupIn` (`<&`) and `Operator::RedirReadWrite` (`<>`).
- `src/executor.rs` — `RedirectScope` (generalize `CompoundRedirectScope`); ordered in-process applier; `pre_exec` ordered applier for `run_subprocess` + pipeline spawn; `{var}` allocation; rewrite `apply_redirects_permanently` (exec); delete the bridge usage.
- `tests/scripts/fd_redirect_diff_check.sh` — new bash-diff harness.
- `docs/bash-divergences.md` — delete M-124, L-08, M-20.

---

### Task 1: Redirect AST types

**Files:**
- Modify: `src/command.rs` (add types near the existing `Redirect` enum, ~line 254)
- Test: `src/command.rs` `#[cfg(test)] mod tests`

- [ ] **Step 1: Add the new types** (additive — nothing else changes yet, so it compiles).

```rust
/// One redirection, applied in source order. Replaces the old fixed
/// stdin/stdout/stderr slots so fd>2 and source-ordering (`2>&1 >file`) work.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Redirection {
    pub fd: RedirFd,
    pub op: RedirOp,
}

/// The target file descriptor of a redirection.
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum RedirFd {
    /// No explicit prefix: resolves to 0 for input ops, 1 for output ops.
    Default,
    /// `3>` / `2<&` — an explicit numeric fd.
    Number(u16),
    /// `{name}>` — allocate a free fd (>=10) at apply time and assign $name.
    Var(String),
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum FileMode {
    ReadOnly,   // <     default fd 0
    Truncate,   // >     default fd 1
    Append,     // >>    default fd 1
    Clobber,    // >|    default fd 1
    ReadWrite,  // <>    default fd 0
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum RedirOp {
    File { mode: FileMode, target: Word },
    /// `>&w` (output=true) / `<&w` (output=false). `source` is an fd-number
    /// word; a `-` source is normalized to `Close` by the parser.
    Dup { source: Word, output: bool },
    /// `N>&-` / `N<&-`.
    Close,
    Heredoc { body: Word, expand: bool, strip_tabs: bool },
    HereString(Word),
}

impl RedirOp {
    /// The fd this op targets when `RedirFd::Default` (no explicit prefix).
    pub fn default_fd(&self) -> u16 {
        match self {
            RedirOp::File { mode: FileMode::ReadOnly | FileMode::ReadWrite, .. } => 0,
            RedirOp::File { .. } => 1,
            RedirOp::Dup { output: true, .. } => 1,
            RedirOp::Dup { output: false, .. } => 0,
            RedirOp::Close => 0,
            RedirOp::Heredoc { .. } | RedirOp::HereString(_) => 0,
        }
    }
}

impl Redirection {
    /// The concrete numeric target fd for non-`Var` redirections (`Var` is
    /// resolved at apply time). Used by the legacy bridge + applier.
    pub fn target_fd(&self) -> Option<u16> {
        match &self.fd {
            RedirFd::Default => Some(self.op.default_fd()),
            RedirFd::Number(n) => Some(*n),
            RedirFd::Var(_) => None,
        }
    }
}
```

- [ ] **Step 2: Add unit tests for `default_fd` / `target_fd`.**

```rust
#[test]
fn redirop_default_fds() {
    use crate::command::{RedirOp, FileMode, RedirFd, Redirection};
    let w = ww("f");
    assert_eq!(RedirOp::File { mode: FileMode::ReadOnly, target: w.clone() }.default_fd(), 0);
    assert_eq!(RedirOp::File { mode: FileMode::Truncate, target: w.clone() }.default_fd(), 1);
    assert_eq!(RedirOp::File { mode: FileMode::ReadWrite, target: w.clone() }.default_fd(), 0);
    assert_eq!(RedirOp::Dup { source: ww("1"), output: true }.default_fd(), 1);
    let r = Redirection { fd: RedirFd::Number(3), op: RedirOp::Close };
    assert_eq!(r.target_fd(), Some(3));
    let v = Redirection { fd: RedirFd::Var("x".into()), op: RedirOp::Close };
    assert_eq!(v.target_fd(), None);
}
```

- [ ] **Step 3:** `cargo test --bin huck -- redirop_default_fds` → PASS. `cargo build` green.
- [ ] **Step 4: Commit** (`git add src/command.rs && git commit`).

---

### Task 2: Front-end migration — lexer fd-prefix, parser builds `Vec<Redirection>`, AST slots → list, executor bridge

This is the large atomic front-end change (the type swap forces lexer + parser + AST + a bridge to land together). After it: all redirects parse into `redirects: Vec<Redirection>`; the executor consumes fds 0/1/2 via `legacy_slots()` with IDENTICAL behavior; fd>2 / `<&` / `{var}` parse but are dropped by the bridge (made real in Tasks 3–6).

**Files:** `src/lexer.rs`, `src/command.rs`, `src/executor.rs`.

- [ ] **Step 1 (lexer): add tokens + operators.** In `src/lexer.rs`:
  - Add `Token::RedirFd(crate::command::RedirFd)` to the `Token` enum.
  - Add `Operator::DupIn` (`<&`) and `Operator::RedirReadWrite` (`<>`) to `Operator`.
  - In the `<` scanner branch (~line 732): after `<`, if next is `&` emit `Operator::DupIn`; if next is `>` emit `Operator::RedirReadWrite`; else `RedirIn` (existing). Keep `<<`/`<<-`/`<<<` handling as-is.

- [ ] **Step 2 (lexer): fd-prefix detection.** A redirect operator is fd-prefixed when a digit-run or `{ident}` immediately precedes it with NO whitespace. Implementation: the lexer already tracks the byte offset (`CharCursor`). When about to push a redirect operator token, check the just-emitted token: if it is a `Token::Word` whose source was glued (its end offset == the operator's start offset) AND its text is all ASCII digits (→ `RedirFd::Number`) or matches `{ident}` (→ `RedirFd::Var(ident)`), POP that word token and push `Token::RedirFd(..)` before the operator token. Concretely, add a helper run right before each redirect-operator `tokens.push`:

```rust
// In lexer.rs, near the redirect-emitting branches. `tokens` is the token vec,
// `glued` is true iff no delimiter (space/tab) was consumed since the previous
// token ended (track with a `prev_token_end == op_start_offset` check on the
// CharCursor offset; the lexer already stamps offsets via push_pos!).
fn take_fd_prefix(tokens: &mut Vec<Token>, glued: bool) -> Option<crate::command::RedirFd> {
    if !glued { return None; }
    let Some(Token::Word(w)) = tokens.last() else { return None; };
    let text = crate::command::word_literal_text(w)?; // None if not a single plain literal
    let fd = if !text.is_empty() && text.chars().all(|c| c.is_ascii_digit()) {
        text.parse::<u16>().ok().map(crate::command::RedirFd::Number)
    } else if let Some(inner) = text.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
        // {name}: a valid identifier
        if !inner.is_empty()
            && inner.chars().next().map(|c| c.is_alphabetic() || c == '_').unwrap_or(false)
            && inner.chars().all(|c| c.is_alphanumeric() || c == '_')
        { Some(crate::command::RedirFd::Var(inner.to_string())) } else { None }
    } else { None };
    if fd.is_some() { tokens.pop(); }
    fd
}
```
  Then at each redirect-operator emit site, build the token as `Token::RedirFd(prefix)` (when `Some`) immediately followed by the existing `Token::Op(...)`. Track `glued` by comparing the operator's start offset to the previous token's recorded end offset (add an `end`-offset field to `push_pos!` if not already available, or compare against the CharCursor offset captured before scanning the operator vs after the previous token). NOTE: this MUST NOT fire across whitespace — `echo 2 >&1` keeps `2` as a word; `file2>x` keeps `file2` as a word (not all-digits → `take_fd_prefix` returns None). Add lexer unit tests for all three (`3>`, `echo 2 >&1`, `file2>x`).

- [ ] **Step 3 (command.rs): map tokens → `RedirOp`.** Add `fn op_kind_to_redirop(op: Operator, target_or_source: Word) -> RedirOp`:
  - `RedirIn` → `File { ReadOnly, target }`; `RedirOut` → `File { Truncate, .. }`; `RedirAppend` → `File { Append, .. }`; `RedirClobber`/`RedirErrClobber` → `File { Clobber, .. }`; `RedirReadWrite` → `File { ReadWrite, .. }`.
  - `DupOut` → if source text == "-" → `Close`, else `Dup { source, output: true }`.
  - `DupIn`/`DupErr` → `DupOut` analog: `DupErr` is `2>&` (target fd 2, output true) — represent as a `RedirFd::Number(2)` + `Dup { output: true }` when no explicit prefix; `DupIn` (`<&`) → `Dup { output: false }` (source "-" → Close).
  - `AndRedirOut`/`AndRedirAppend` (`&>`/`&>>`) stay special (redirect both): the parser emits TWO `Redirection`s — `{Default(1), File{Truncate/Append}}` and `{Number(2), Dup{source:"1", output:true}}` — preserving the both-to-file semantics WITHOUT a `RedirOp::Dup` carrying a filename (per the spec edge case).

- [ ] **Step 4 (command.rs): parser builds the ordered list.** Rewrite `parse_trailing_redirects` to return `Vec<Redirection>` (push in order; no last-wins merge). In its loop: optionally consume a leading `Token::RedirFd` → `fd`; then the redirect `Operator`/`Heredoc`; then the target/source `Word`; build `Redirection { fd: fd.unwrap_or(Default), op: op_kind_to_redirop(...) }`. The simple-stage main loop's `is_redirect_token` predicate must also return true for a `Token::RedirFd` (so the stage keeps consuming). Heredoc tokens → `Redirection { fd: RedirFd::Number(0)-or-prefix, op: Heredoc {..} }`.

- [ ] **Step 5 (command.rs): AST fields.** Change `ExecCommand` to replace `stdin/stdout/stderr: Option<Redirect>` with `pub redirects: Vec<Redirection>`. Same for the `Redirected` compound variant (`redirects: Vec<Redirection>`). `finalize_stage` attaches `redirects`. The empty-program / assignment-only check becomes `redirects.is_empty()` (was `no_redirs`). Update ALL `ExecCommand { .. }` and `Redirected { .. }` literals across `src/command.rs` (parser + tests) — the compiler enumerates them.

- [ ] **Step 6 (command.rs): the bridge.** Keep the OLD `Redirect` enum for now. Add:

```rust
/// TEMPORARY bridge (removed in Task 8): collapse the ordered list back to the
/// old fixed 0/1/2 slots so the not-yet-migrated executor keeps working with
/// IDENTICAL behavior. fd>2 targets, `RedirFd::Var`, `RedirOp::Close`, and
/// `<&`/`Dup{output:false}` are DROPPED here (made real in Tasks 3-6). Last-wins
/// per slot, matching pre-v156 semantics.
pub fn legacy_slots(redirs: &[Redirection]) -> (Option<Redirect>, Option<Redirect>, Option<Redirect>) {
    let (mut sin, mut sout, mut serr) = (None, None, None);
    for r in redirs {
        let Some(fd) = r.target_fd() else { continue }; // drop Var for now
        let legacy = match &r.op {
            RedirOp::File { mode: FileMode::ReadOnly, target } => Some(Redirect::Read(target.clone())),
            RedirOp::File { mode: FileMode::Truncate, target } => Some(Redirect::Truncate(target.clone())),
            RedirOp::File { mode: FileMode::Append, target } => Some(Redirect::Append(target.clone())),
            RedirOp::File { mode: FileMode::Clobber, target } => Some(Redirect::Clobber(target.clone())),
            RedirOp::File { mode: FileMode::ReadWrite, .. } => None, // unsupported pre-Task3; drop
            RedirOp::Dup { source, output: true } => Some(Redirect::Dup { fd: fd as i32, source: source.clone() }),
            RedirOp::Dup { output: false, .. } | RedirOp::Close => None, // drop until Task 3
            RedirOp::Heredoc { body, expand, strip_tabs } =>
                Some(Redirect::Heredoc { body: body.clone(), expand: *expand, strip_tabs: *strip_tabs }),
            RedirOp::HereString(w) => Some(Redirect::HereString(w.clone())),
        };
        match fd { 0 => sin = legacy, 1 => sout = legacy, 2 => serr = legacy, _ => {} }
    }
    (sin, sout, serr)
}
```

- [ ] **Step 7 (executor.rs): consume via the bridge.** At each point that currently reads `cmd.stdin`/`cmd.stdout`/`cmd.stderr` (and the `Redirected` variant's slots), derive them once: `let (stdin, stdout, stderr) = crate::command::legacy_slots(&cmd.redirects);` then use the locals exactly as before. `ResolvedCommand` keeps its `stdin/stdout/stderr: Option<...>` fields; `resolve()` populates them from `legacy_slots(&cmd.redirects)`. `has_any_redirect(cmd)` becomes `!cmd.redirects.is_empty()`. The compiler lists every site; this is mechanical.

- [ ] **Step 8: Verify no behavior change.** `cargo build`; `cargo test` fully green; run ALL four redirect harnesses → byte-identical. Add lexer unit tests:

```rust
#[test]
fn lexer_fd_prefix_numeric() {
    let t = tokenize("echo 2>&1").unwrap();
    // expect: Word(echo), RedirFd(Number(2)), Op(DupOut), Word(1)
    assert!(t.iter().any(|tok| matches!(tok, Token::RedirFd(crate::command::RedirFd::Number(2)))));
}
#[test]
fn lexer_fd_prefix_space_is_not_prefix() {
    let t = tokenize("echo 2 >&1").unwrap();
    assert!(!t.iter().any(|tok| matches!(tok, Token::RedirFd(_))));
}
#[test]
fn lexer_fd_prefix_glued_word_is_not_prefix() {
    let t = tokenize("file2>x").unwrap(); // `file2` is a word, then `>x`
    assert!(!t.iter().any(|tok| matches!(tok, Token::RedirFd(_))));
}
#[test]
fn lexer_named_fd_prefix() {
    let t = tokenize("exec {fd}>log").unwrap();
    assert!(t.iter().any(|tok| matches!(tok, Token::RedirFd(crate::command::RedirFd::Var(n)) if n == "fd")));
}
```
  And parser unit tests asserting the ORDERED list (source order preserved, fd-tagged):

```rust
#[test]
fn parser_redirects_preserve_source_order() {
    use crate::command::{RedirFd, RedirOp, FileMode};
    // `>a 2>&1` ⇒ [ {Default, File(Truncate,a)}, {Number(2), Dup(source=1, out)} ]
    let r = redirs_of("cmd >a 2>&1"); // helper: parse, pull ExecCommand.redirects
    assert_eq!(r.len(), 2);
    assert!(matches!(&r[0], Redirection { fd: RedirFd::Default, op: RedirOp::File { mode: FileMode::Truncate, .. } }));
    assert!(matches!(&r[1].fd, RedirFd::Number(2)));
    assert!(matches!(&r[1].op, RedirOp::Dup { output: true, .. }));
    // `2>&1 >a` ⇒ reversed order (this is the L-08 distinction)
    let r2 = redirs_of("cmd 2>&1 >a");
    assert!(matches!(&r2[0].fd, RedirFd::Number(2)));
    assert!(matches!(&r2[1].op, RedirOp::File { mode: FileMode::Truncate, .. }));
}

#[test]
fn parser_readwrite_and_named_fd() {
    use crate::command::{RedirFd, RedirOp, FileMode};
    let r = redirs_of("cmd 3<>f");
    assert!(matches!(&r[0].fd, RedirFd::Number(3)));
    assert!(matches!(&r[0].op, RedirOp::File { mode: FileMode::ReadWrite, .. }));
    let r2 = redirs_of("cmd {fd}>f");
    assert!(matches!(&r2[0].fd, RedirFd::Var(n) if n == "fd"));
    // `3>&-` ⇒ Close
    let r3 = redirs_of("cmd 3>&-");
    assert!(matches!(&r3[0], Redirection { fd: RedirFd::Number(3), op: RedirOp::Close }));
}
```
  (Add a `redirs_of(src: &str) -> Vec<Redirection>` test helper that tokenizes+parses and returns the first `ExecCommand`'s `redirects`.)

- [ ] **Step 9: Commit.** This is the front-end; behavior is unchanged for 0/1/2.

---

### Task 3: In-process ordered applier (`RedirectScope`) — arbitrary fds, dup, close, L-08

Make fd>2 / `<&` / `N>&-` / source-ordering real for the IN-PROCESS paths (builtins, compound commands, `with_redirect_scope`, the shell itself). `{var}` deferred to Task 5.

**Files:** `src/executor.rs`.

- [ ] **Step 1: Generalize the scope.** Rename/extend `CompoundRedirectScope` to `RedirectScope` and add `fn apply(&mut self, redir: &Redirection, shell: &mut Shell) -> Result<(), ExecOutcome>` that:
  - resolves the numeric target fd via `redir.target_fd()` (skip `Var` → handled in Task 5; for now return an error if encountered);
  - `RedirOp::File { mode, target }`: expand `target`, open with flags per `mode` (ReadOnly=O_RDONLY; Truncate=O_WRONLY|O_CREAT|O_TRUNC honoring noclobber; Append=O_WRONLY|O_CREAT|O_APPEND; Clobber=force truncate; ReadWrite=O_RDWR|O_CREAT) → `dup2(file_fd, target)` via `self.redirect(file_fd, target)`;
  - `RedirOp::Dup { source, output: _ }`: resolve `source` via `resolve_fd_target` → `self.redirect(source_fd, target)` (saving the prior target);
  - `RedirOp::Close`: save the prior target fd, then `close(target)` (lenient: ignore EBADF);
  - `RedirOp::Heredoc`/`HereString`: spawn writer, `redirect(read_end, target)`.
  The existing `apply_out_redirect` logic folds into the `File` arm; keep the `noclobber`/`>|` handling.

- [ ] **Step 2: Drive it from the in-process paths.** Replace `with_redirect_scope`'s body (currently hardcoded stdin/stdout/stderr blocks) with a loop over `cmd.redirects` calling `scope.apply(redir, shell)` IN ORDER. The builtin path (`open_stage_files` + `prepare_builtin_stdin/stderr`) likewise iterates `cmd.redirects` applying via a `RedirectScope` for the duration of the builtin call (save/restore around it). Stop deriving 0/1/2 via `legacy_slots` in these paths.

- [ ] **Step 3: Add an in-process integration test (`tests/` or inline).** A `huck -c` round-trip that exercises L-08 and a dup-and-close in-process via a builtin (e.g. `echo`):

```
huck -c 'echo both 2>&1 1>/tmp/hk_l08; echo "after"' 2>&1   # ordering
```
  Assert against bash. Defer full coverage to the harness (Task 8).

- [ ] **Step 4:** `cargo test` green; the four existing harnesses byte-identical (compound/function redirect paths now go through `RedirectScope`). Manually verify `>&3` etc. work in a compound. Commit.

---

### Task 4: External (forked) ordered applier — `pre_exec` replay

Make arbitrary-fd redirects work for EXTERNAL commands and pipeline stages.

**Files:** `src/executor.rs` (`run_subprocess`, the pipeline `spawn_external_with_fds` path).

- [ ] **Step 1: Build a replay list in the parent.** In `run_subprocess`, instead of the `legacy_slots`/`open_stage_files` 0/1/2 setup, iterate `cmd.redirects` IN ORDER in the parent: for each `File` op, expand + open the file → push `(target_fd, source_fd = file_fd, kind=Dup)`; for `Dup`, resolve source → push `(target_fd, source_fd, Dup)`; for `Close`, push `(target_fd, Close)`; heredocs as today (forked writer → its read fd is a source). Result: an ordered `Vec<ChildRedirOp>` of pure `dup2`/`close` operations plus the parent-side `OwnedFd`s to keep alive until spawn.

```rust
enum ChildRedirOp { Dup { target: i32, source: i32 }, Close { target: i32 } }
```

- [ ] **Step 2: Apply in `pre_exec`.** Add ONE `pre_exec` closure (after the signal-reset one) that replays the `Vec<ChildRedirOp>` in order with `libc::dup2`/`libc::close` (both async-signal-safe). Drop the `std::process::Command` `.stdin/.stdout/.stderr` setters for the redirect case (still used for pipe-stage wiring). Keep the opened `OwnedFd`s in the parent until after `spawn()` so they aren't closed early; the child's `dup2` copies them onto the targets.

- [ ] **Step 3: Test.** `huck -c 'exec 3>/tmp/x; date >&3 ...'` won't work until Task 6 (exec), so test via a self-contained external case that doesn't need a held fd, e.g. a fd-swap into a file:

```
huck -c 'sh -c "echo out; echo err >&2" >/tmp/hk_o 2>/tmp/hk_e; cat /tmp/hk_o /tmp/hk_e'
```
  plus `cmd 3>&1 ...` patterns. Compare to bash.

- [ ] **Step 4:** `cargo test` green; harnesses byte-identical; commit.

---

### Task 5: `{var}` named-fd allocation

**Files:** `src/executor.rs` (the `RedirectScope::apply` + the parent replay-list builder).

- [ ] **Step 1: Allocator helper.**

```rust
/// Allocate a free fd >= 10 duped from `src_fd` (close-on-exec OFF so it
/// survives into children, matching bash). Returns the new fd.
fn alloc_high_fd(src_fd: RawFd) -> io::Result<RawFd> {
    let fd = unsafe { libc::fcntl(src_fd, libc::F_DUPFD, 10) };
    if fd < 0 { return Err(io::Error::last_os_error()); }
    Ok(fd)
}
```

- [ ] **Step 2: Wire `RedirFd::Var(name)`.** When a redirection's `fd` is `Var(name)`: open/resolve the source as usual, then `alloc_high_fd` → `H`; `shell.set(name, H.to_string())`; the redirection targets `H` (not via dup2-onto-a-fixed-target — the allocated fd IS the target). For a normal command the fd is closed after (scope restore / child exit); the var keeps the number (bash-style). For `{fd}>&-` / `Close` with `RedirFd::Var(name)`: read `shell.lookup_var(name)` → fd number → close it.

- [ ] **Step 3: Test.** `exec {fd}>...` needs Task 6; test the non-exec form: `huck -c '{fd}>/tmp/x; echo "$fd"'` → a number ≥10 (the fd closes after but `$fd` is set), compare to bash (`$fd ≥ 10`; exact number may differ — assert `-ge 10`, not equality).

- [ ] **Step 4:** `cargo test` green; commit.

---

### Task 6: `exec` integration

**Files:** `src/executor.rs` (`apply_redirects_permanently`, written in v155).

- [ ] **Step 1: Rewrite `apply_redirects_permanently`** to iterate `cmd.redirects` IN ORDER via a `RedirectScope`, but on SUCCESS make permanent (the existing drain-saved-originals trick), supporting arbitrary fds, `RedirFd::Var` allocation (the held fd persists; `$name` set), and `RedirOp::Close`. On any open failure, roll back atomically (scope Drop), return `Err` (exec returns `Continue(1)`, does not exit — unchanged).

- [ ] **Step 2: Test end-to-end** against bash:
```
huck -c 'exec 3>/tmp/x; echo held >&3; exec 3>&-; cat /tmp/x; rm /tmp/x'   # → held
huck -c 'exec 3</etc/hostname; read h <&3; echo "$h"'                       # → hostname line
huck -c 'exec {fd}>/tmp/y; echo "$fd"; echo z >&"$fd"; exec {fd}>&-; cat /tmp/y; rm /tmp/y'
```

- [ ] **Step 3:** `cargo test` green; `exec_diff_check.sh` byte-identical; commit.

---

### Task 7: Remove the bridge + old `Redirect` enum

**Files:** `src/command.rs`, `src/executor.rs`.

- [ ] **Step 1:** Confirm no path still calls `legacy_slots` (grep). Every executor redirect path now consumes `cmd.redirects` directly (Tasks 3–6). Delete `legacy_slots`.
- [ ] **Step 2:** Delete the old `Redirect` enum from `command.rs` and any remaining references; `ResolvedCommand` no longer needs the bridge (resolve builds whatever the applier consumes — keep `redirects` raw on `ResolvedCommand` or resolve at apply time, whichever the Task-3/4 appliers settled on; make it consistent).
- [ ] **Step 3:** `cargo build` (compiler confirms nothing references the deleted enum); `cargo test` green; clippy clean; harnesses byte-identical. Commit.

---

### Task 8: Harness + retire divergences + regression

**Files:** Create `tests/scripts/fd_redirect_diff_check.sh`; modify `docs/bash-divergences.md`.

- [ ] **Step 1: Write `fd_redirect_diff_check.sh`** (model on `exec_diff_check.sh`'s `check` helper; byte-identical bash↔huck; failure cases status-only with stderr suppressed):

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for arbitrary-fd (fd>2) redirections (v156).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() { local l="$1" f="$2" b h
  b=$(printf '%s\n' "$f" | bash --norc --noprofile 2>&1; echo "EXIT:$?")
  h=$(printf '%s\n' "$f" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
  if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$l"; PASS=$((PASS+1))
  else printf 'FAIL: %s\n' "$l"; diff <(echo "$b") <(echo "$h")|sed 's/^/  /'; FAIL=$((FAIL+1)); fi; }

check "exec hold/write/close" 'f=$(mktemp); exec 3>"$f"; echo x >&3; exec 3>&-; cat "$f"; rm -f "$f"'
check "exec read via <&3"     'f=$(mktemp); printf "a\nb\n">"$f"; exec 3<"$f"; read u <&3; read v <&3; echo "$u$v"; exec 3<&-; rm -f "$f"'
check "L-08 2>&1 >file"       'f=$(mktemp); { echo out; echo err >&2; } 2>&1 >"$f"; echo "file=[$(cat "$f")]"; rm -f "$f"'
check "L-08 >file 2>&1"       'f=$(mktemp); { echo out; echo err >&2; } >"$f" 2>&1; echo "file=[$(cat "$f")]"; rm -f "$f"'
check "fd swap stdout/stderr" 'sh -c "echo O; echo E >&2" 3>&1 1>&2 2>&3 3>&- 2>/dev/null'
check "<> read-write"         'f=$(mktemp); printf abc>"$f"; exec 3<>"$f"; echo -n X>&3; exec 3>&-; cat "$f"; rm -f "$f"'
check "named {fd} >=10"       'f=$(mktemp); exec {fd}>"$f"; [ "$fd" -ge 10 ] && echo okfd; echo z>&"$fd"; exec {fd}>&-; cat "$f"; rm -f "$f"'
check "10>>file append"       'f=$(mktemp); printf head>"$f"; exec 10>>"$f"; echo body>&10; exec 10>&-; cat "$f"; rm -f "$f"'
check "bad source fd EBADF"   '(echo x >&9) 2>/dev/null; echo "rc=$?"'
check "missing input file"    '(exec 3</no/such_xyz) 2>/dev/null; echo "rc=$?"'

printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
```
  Run: `bash tests/scripts/fd_redirect_diff_check.sh` → all PASS. (If `named {fd}` diverges on the exact number, the test already only asserts `-ge 10` + behavior.)

- [ ] **Step 2: Retire divergences.** In `docs/bash-divergences.md` DELETE the M-124, L-08, and M-20 entries; update the Summary tier counts (Tier 2 −2 for M-124+M-20; Tier 4/relevant tier −1 for L-08 — verify exact tiers in the doc and adjust).
- [ ] **Step 3: Full regression.** `cargo test` (entire suite) + `cargo clippy` clean + EVERY `tests/scripts/*_diff_check.sh` green. Commit.

---

## Notes for the implementer
- The two genuinely large tasks are **Task 2** (front-end type swap — the compiler drives the ~88 mechanical edits; lean on `cargo build` errors) and **Tasks 3–4** (the appliers). Everything else is bounded.
- Keep the `legacy_slots` bridge EXACTLY behavior-preserving in Task 2 — it is the green-between-tasks safety net; do not "improve" 0/1/2 behavior there.
- `resolve_fd_target` (executor.rs:2255) already parses an fd-number Word and is the right primitive for `Dup` sources.
- macOS: use `F_DUPFD` (portable) — `F_DUPFD_CLOEXEC` exists on macOS too but bash leaves coproc/exec fds NON-cloexec, so `F_DUPFD` (no cloexec) matches bash semantics for `{var}`/`exec` fds. Confirm against the macOS-portability memory.
