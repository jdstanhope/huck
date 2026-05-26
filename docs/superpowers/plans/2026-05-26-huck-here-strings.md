# v27: Here-Strings `<<<word` — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement POSIX/bash here-string redirection `cmd <<<word` — feeds the expansion of `word` (with trailing `\n`) as `cmd`'s stdin. Closes M-13 from `docs/bash-divergences.md`.

**Architecture:** New `Operator::HereString` token for `<<<`; new `Redirect::HereString(Word)` AST variant; parser wires the next-word into `stdin`; executor reuses v24's deferred-expansion + stdin-pipe machinery with a new `StdinInput::DeferredHereString(Word)` variant. The expansion uses `expand_assignment` (no split/glob) + a literal trailing `\n`.

**Tech Stack:** Rust 1.95; existing huck modules (`src/lexer.rs`, `src/command.rs`, `src/executor.rs`). No new dependencies.

**Spec:** `docs/superpowers/specs/2026-05-26-huck-here-strings-design.md`.

**Branch:** `v27-here-strings` (off `main` at commit `3594243`).

**Baseline:** 1058 tests pass, 0 clippy warnings.

---

## File structure

- `src/command.rs` — `Redirect` enum grows `HereString(Word)`; parser dispatches `Token::Op(HereString)` → `cmd.stdin = Some(Redirect::HereString(target_word))`.
- `src/lexer.rs` — new `Operator::HereString`; `<` arm extended to peek for `<<<`.
- `src/executor.rs` — `ResolvedStdin::HereString(Word)` + `StdinInput::DeferredHereString(Word)` variants; `resolve()` and `open_stage_files` route them; the existing 3 expansion sites (`run_subprocess`, `run_multi_stage`, `run_background_sequence`) gain a new branch for `DeferredHereString`.
- `tests/here_string_integration.rs` (new) — end-to-end coverage.
- `docs/bash-divergences.md` — M-13 → fixed; change-log entry.
- `README.md` — v27 status row.

---

## Task 1: AST + lexer + parser (front-end)

After this task, `cat <<< hi` parses into an `ExecCommand` with `stdin = Some(Redirect::HereString(...))`. The executor `unreachable!`s on the new variant (no test runs it through execution yet).

**Files:** `src/lexer.rs`, `src/command.rs`, `src/executor.rs` (minimal — just add `unreachable!` arms).

- [ ] **Step 1: Snapshot baseline**

```bash
cd /home/john/projects/shuck
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print p, f}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```
Expected: `1058 0` and `0`.

- [ ] **Step 2: Add `Operator::HereString` + `Redirect::HereString(Word)`**

In `src/lexer.rs`, add the operator variant:
```rust
pub enum Operator {
    // existing...
    HereString,  // NEW: <<<
}
```

In `src/command.rs`, add the AST variant:
```rust
pub enum Redirect {
    Read(Word),
    Truncate(Word),
    Append(Word),
    Heredoc { body: Word, expand: bool, strip_tabs: bool },
    HereString(Word),    // NEW
}
```

- [ ] **Step 3: Failing lexer tests**

In `src/lexer.rs::tests`:
```rust
#[test]
fn tokenize_here_string_op_alone() {
    let tokens = tokenize("<<<").unwrap();
    assert_eq!(tokens, vec![Token::Op(Operator::HereString)]);
}

#[test]
fn tokenize_here_string_with_unquoted_word() {
    let tokens = tokenize("cat <<< hello").unwrap();
    assert_eq!(tokens.len(), 3);
    assert!(matches!(tokens[0], Token::Word(_)));
    assert!(matches!(tokens[1], Token::Op(Operator::HereString)));
    assert!(matches!(tokens[2], Token::Word(_)));
}

#[test]
fn tokenize_here_string_with_quoted_word() {
    let tokens = tokenize("cat <<< \"hi there\"").unwrap();
    let Token::Word(Word(parts)) = &tokens[2] else { panic!("got {:?}", tokens[2]) };
    assert!(matches!(&parts[0], WordPart::Literal { text, quoted: true } if text == "hi there"));
}

#[test]
fn tokenize_here_string_with_var_in_body() {
    let tokens = tokenize("cat <<< $FOO").unwrap();
    let Token::Word(Word(parts)) = &tokens[2] else { panic!() };
    assert!(matches!(&parts[0], WordPart::Var { name, .. } if name == "FOO"));
}

#[test]
fn tokenize_here_string_with_command_sub_in_body() {
    let tokens = tokenize("cat <<< $(echo hi)").unwrap();
    let Token::Word(Word(parts)) = &tokens[2] else { panic!() };
    assert!(matches!(&parts[0], WordPart::CommandSub { .. }));
}

#[test]
fn tokenize_double_less_still_heredoc() {
    // Regression: `<<EOF` must still lex as Heredoc, not split into `<<` + `<EOF`.
    let tokens = tokenize("cat <<EOF\nbody\nEOF\n").unwrap();
    assert!(tokens.iter().any(|t| matches!(t, Token::Heredoc { .. })),
        "expected Heredoc token, got {:?}", tokens);
}
```

Run: `cargo test --bin huck tokenize_here_string tokenize_double_less` — expect failures.

- [ ] **Step 4: Implement `<<<` recognition in the lexer**

In `src/lexer.rs`, find the `<` arm (around the existing Heredoc/HeredocStrip dispatch). Extend the peek-chain:

```rust
'<' => {
    if has_token {
        // flush current word (existing logic)
    }
    if chars.peek() == Some(&'<') {
        chars.next(); // second '<'
        if chars.peek() == Some(&'<') {
            chars.next(); // third '<' — here-string
            tokens.push(Token::Op(Operator::HereString));
            in_assignment_value = false;
        } else if chars.peek() == Some(&'-') {
            // existing HeredocStrip path
        } else {
            // existing Heredoc path
        }
    } else {
        // existing RedirIn path
    }
}
```

Don't touch the pending-heredoc queue — here-strings have no body-collection phase. The Word that follows comes via normal `Token::Word` lexing on the same line.

- [ ] **Step 5: Verify lexer tests pass**

```bash
cargo test --bin huck tokenize_here_string tokenize_double_less 2>&1 | tail -10
```
Expected: all 6 pass.

- [ ] **Step 6: Failing parser tests**

In `src/command.rs::tests`:
```rust
#[test]
fn parse_here_string_attaches_to_stdin() {
    let tokens = tokenize("cat <<< hi").unwrap();
    let parsed = parse(tokens).unwrap().expect("non-empty parse");
    let Command::Pipeline(p) = parsed.first else { panic!() };
    let Command::Simple(SimpleCommand::Exec(e)) = &p.commands[0] else { panic!() };
    assert!(matches!(&e.stdin, Some(Redirect::HereString(_))));
}

#[test]
fn parse_here_string_last_wins_over_file() {
    let tokens = tokenize("cat <file <<< hi").unwrap();
    let parsed = parse(tokens).unwrap().expect("non-empty parse");
    let Command::Pipeline(p) = parsed.first else { panic!() };
    let Command::Simple(SimpleCommand::Exec(e)) = &p.commands[0] else { panic!() };
    assert!(matches!(&e.stdin, Some(Redirect::HereString(_))));
}

#[test]
fn parse_here_string_last_wins_over_heredoc() {
    let tokens = tokenize("cat <<EOF <<< override\nignored\nEOF\n").unwrap();
    let parsed = parse(tokens).unwrap().expect("non-empty parse");
    let Command::Pipeline(p) = parsed.first else { panic!() };
    let Command::Simple(SimpleCommand::Exec(e)) = &p.commands[0] else { panic!() };
    assert!(matches!(&e.stdin, Some(Redirect::HereString(_))));
}

#[test]
fn parse_here_string_missing_word_errors() {
    let tokens = tokenize("cat <<<").unwrap();
    let result = parse(tokens);
    assert!(result.is_err(), "expected parse error, got {:?}", result);
}

#[test]
fn parse_here_string_in_pipeline_stage() {
    let tokens = tokenize("cat <<< x | grep x").unwrap();
    let parsed = parse(tokens).unwrap().expect("non-empty parse");
    let Command::Pipeline(p) = parsed.first else { panic!() };
    assert_eq!(p.commands.len(), 2);
    let Command::Simple(SimpleCommand::Exec(stage0)) = &p.commands[0] else { panic!() };
    assert!(matches!(&stage0.stdin, Some(Redirect::HereString(_))));
    let Command::Simple(SimpleCommand::Exec(stage1)) = &p.commands[1] else { panic!() };
    assert!(stage1.stdin.is_none());
}
```

Run: expect failures.

- [ ] **Step 7: Implement the parser arm**

Find the per-stage redirect-consumption code in `src/command.rs` (look for where `Token::Op(RedirIn)` is handled — it should be in `parse_simple_stage` or wherever per-stage redirects are processed). Add:

```rust
Token::Op(Operator::HereString) => {
    let target = match iter.next() {
        Some(Token::Word(w)) => w,
        _ => return Err(/* missing-redirect-target error variant */),
    };
    stdin = Some(Redirect::HereString(target));
}
```

(Use whatever the project's existing `MissingRedirectTarget`-style error variant is — look at how `RedirIn` handles the same case.)

- [ ] **Step 8: Add `unreachable!` arms in the executor**

The executor's stdin-handling code in `src/executor.rs` (look in `resolve` and `open_stage_files`) currently matches on `Redirect::Read` and `Redirect::Heredoc { .. }` and has `unreachable!` arms for stdout-side variants. Add a corresponding `unreachable!` for `Redirect::HereString(_)`:

```rust
Some(Redirect::HereString(_)) => unreachable!(
    "Redirect::HereString handling lands in Task 2; parser produces this now \
     but the executor doesn't route it yet"
)
```

This makes the build pass for Task 1 without execution. The `unreachable!` will fire if Task 1's parser tests accidentally invoke execution — but they should only inspect AST shapes, not run anything.

- [ ] **Step 9: Verify build/tests/clippy**

```bash
cargo build 2>&1 | tail -3
cargo test --bin huck parse_here_string 2>&1 | tail -10
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print p, f}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```
Expected: 5 new parser tests pass; full suite 1058 + 6 + 5 = 1069, 0 fails, 0 warnings.

- [ ] **Step 10: Commit**

```bash
git add -A
git commit -m "ast+lex+parse: <<<word here-string tokenization and parsing

New Operator::HereString for the <<< token; new Redirect::HereString(Word)
AST variant; parser routes Token::Op(HereString) + next Word into
ExecCommand.stdin (last-wins like other redirects). Executor stub:
unreachable!() pending Task 2 wiring."
```

---

## Task 2: Executor — DeferredHereString through to bytes

After this task, `cat <<< hi` actually pipes `hi\n` into cat's stdin. Reuses v24's deferred-expansion + stdin-pipe machinery.

**Files:** `src/executor.rs`.

- [ ] **Step 1: Failing smoke test**

Create `tests/here_string_integration.rs`:
```rust
//! End-to-end tests for v27 here-strings.

use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run(script: &str) -> (String, String) {
    let mut child = Command::new(huck_binary())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");
    child.stdin.as_mut().unwrap().write_all(script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let output = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn here_string_simple_word() {
    let (out, _) = run("cat <<< hello\nexit\n");
    assert!(out.contains("hello"), "got: {out}");
}
```

Run: expect failure (executor `unreachable!`s on HereString).

- [ ] **Step 2: Add the deferred-expansion variants**

In `src/executor.rs`, find the `ResolvedStdin` enum (added in v24):
```rust
enum ResolvedStdin {
    File(String),
    Heredoc(Word),
    HereString(Word),       // NEW
}
```

And `StdinInput`:
```rust
enum StdinInput {
    File(File),
    DeferredHeredoc(Word),
    DeferredHereString(Word),  // NEW
}
```

- [ ] **Step 3: Route in `resolve` and `open_stage_files`**

In `resolve()`:
```rust
Some(Redirect::HereString(w)) => Some(ResolvedStdin::HereString(w.clone())),
```
(Replace the `unreachable!` from Task 1.)

In `open_stage_files`:
```rust
ResolvedStdin::HereString(body) => StdinInput::DeferredHereString(body),
```
(Match the existing `Heredoc → DeferredHeredoc` pattern.)

- [ ] **Step 4: Expansion site — three locations**

Find where `DeferredHeredoc(body)` is currently expanded to bytes. There are three call sites: `run_subprocess` (single-stage external), `run_multi_stage` (per-stage), and `run_background_sequence` (backgrounded). Each has a match arm like:
```rust
StdinInput::DeferredHeredoc(body) => {
    let bytes = expand_assignment(&body, shell).into_bytes();
    // ... pipe bytes to child's stdin ...
}
```

In each, add a parallel arm:
```rust
StdinInput::DeferredHereString(body) => {
    let mut bytes = expand_assignment(&body, shell).into_bytes();
    bytes.push(b'\n');  // per bash — trailing newline
    // ... same pipe-to-child code ...
}
```

The plumbing (pipe creation, write-then-close, fd cleanup) is shared — just the byte-production step differs.

- [ ] **Step 5: Verify smoke test + full suite**

```bash
cargo test --test here_string_integration 2>&1 | tail -10
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print p, f}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```
Expected: smoke test passes; full suite ~1070, 0 fails, 0 warnings.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "exec: pipe here-string bytes into child stdin (deferred expansion)

ResolvedStdin::HereString + StdinInput::DeferredHereString mirror the v24
Heredoc plumbing exactly. Expansion at spawn time via expand_assignment
(no split/glob) + trailing \\n per bash. Single smoke test wired; full
test table follows in Task 3."
```

---

## Task 3: Full integration test suite

Cover every spec test-table row.

**Files:** `tests/here_string_integration.rs`.

- [ ] **Step 1: Add the remaining 12 integration tests from the spec**

Append to `tests/here_string_integration.rs` (the smoke test added in Task 2 already covers `here_string_simple_word`):

```rust
#[test]
fn here_string_quoted_word() {
    let (out, _) = run("cat <<< \"hello world\"\nexit\n");
    assert!(out.contains("hello world"), "got: {out}");
}

#[test]
fn here_string_expands_var() {
    let (out, _) = run("FOO=hi\ncat <<< $FOO\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "hi"), "got: {out}");
}

#[test]
fn here_string_expands_command_sub() {
    let (out, _) = run("cat <<< $(echo via-sub)\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "via-sub"), "got: {out}");
}

#[test]
fn here_string_with_inline_assignment() {
    let (out, _) = run("FOO=val cat <<< $FOO\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "val"), "got: {out}");
}

#[test]
fn here_string_in_pipeline_stage() {
    let (out, _) = run("cat <<< marker | grep marker\nexit\n");
    assert!(out.contains("marker"), "got: {out}");
}

#[test]
fn here_string_empty_word() {
    // cat <<< "" produces just a newline; verify by piping to wc -c.
    let (out, _) = run("cat <<< \"\" | wc -c\nexit\n");
    // wc -c output is "1" (just the trailing \n).
    assert!(out.lines().any(|l| l.trim() == "1"), "got: {out}");
}

#[test]
fn here_string_no_split_with_spaces() {
    let (out, _) = run("FOO=\"a b c\"\ncat <<< $FOO\nexit\n");
    // Should appear as one line "a b c", NOT three separate lines.
    assert!(out.lines().any(|l| l.trim() == "a b c"), "got: {out}");
}

#[test]
fn here_string_last_wins_over_file() {
    let tmp = format!("/tmp/huck_v27_lastwins_{}", std::process::id());
    let script = format!(
        "echo wrong > {tmp}\ncat <{tmp} <<< right\nrm {tmp}\nexit\n"
    );
    let (out, _) = run(&script);
    assert!(out.lines().any(|l| l.trim() == "right"), "got: {out}");
    assert!(!out.contains("wrong"), "file content leaked through; got: {out}");
}

#[test]
fn here_string_trailing_newline_present() {
    // cat <<< hi produces "hi\n" — piping to wc -l should report 1 line.
    let (out, _) = run("cat <<< hi | wc -l\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "1"), "got: {out}");
}

#[test]
fn here_string_dollar_question_snapshot() {
    // After `false`, $? = 1. The here-string's expansion should see 1
    // (B-07 snapshot semantics via expand_assignment).
    let (out, _) = run("false\ncat <<< $?\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "1"), "got: {out}");
}

#[test]
fn here_string_single_quoted_no_expand() {
    // Single quotes prevent $FOO expansion; child sees literal "$FOO".
    let (out, _) = run("FOO=hi\ncat <<< '$FOO'\nexit\n");
    assert!(out.lines().any(|l| l.trim() == "$FOO"), "got: {out}");
}

#[test]
fn here_string_backgrounded() {
    // Background a here-string redirected to a temp file; verify the
    // file contents include the body.
    let tmp = format!("/tmp/huck_v27_bg_{}", std::process::id());
    let script = format!(
        "cat <<< body > {tmp} &\nwait\ncat {tmp}\nrm -f {tmp}\nexit\n"
    );
    let (out, _) = run(&script);
    assert!(out.lines().any(|l| l.trim() == "body"), "got: {out}");
}
```

- [ ] **Step 2: Verify**

```bash
cargo test --test here_string_integration 2>&1 | tail -20
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print p, f}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```
Expected: 13 integration tests pass; full suite ~1082, 0 fails, 0 warnings.

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "test: full v27 here-string integration coverage

12 new tests covering quoted/unquoted body, var/cmd-sub/inline-assignment
expansion, pipeline composition, empty body, no-split semantics,
last-wins, trailing-newline, \$? snapshot, single-quote literal,
backgrounded redirect. Smoke test from Task 2 stays as the simplest case."
```

---

## Task 4: Doc updates

Mark M-13 fixed; add v27 row to README.

**Files:** `docs/bash-divergences.md`, `README.md`.

- [ ] **Step 1: Update `docs/bash-divergences.md`**

Find the M-13 entry under "Redirects" (or wherever it lives). Replace its body to:
```markdown
- **M-13: Here-strings `<<<word`** — `[fixed (2026-05-26)]` medium. Now supported: `<<<word` feeds the expanded word (no split/glob) plus a trailing newline as stdin to the command. Reuses v24's deferred-expansion + stdin-pipe machinery — per-stage scoping, backgrounded forms, and pipeline composition all work.
```

Update the summary table count (Tier 2: 56 → 55).

Add change-log entry:
```markdown
- **2026-05-26**: M-13 (here-strings `<<<word`) shipped as v27.
```

- [ ] **Step 2: Update `README.md`**

Add the v27 row to the status table, consistent with v22-v26 format:
```
| v27       | Here-strings (`<<<word`)                                 |
```

If the "Not yet implemented" paragraph lists here-strings, remove the mention.

- [ ] **Step 3: Verify**

```bash
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print p, f}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```
Expected: full suite still ~1082, 0 fails, 0 warnings (no code changes, just doc updates).

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "docs: M-13 fixed; v27 in README status table"
```

---

## Final verification (no separate task)

```bash
cargo build 2>&1 | tail -3
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print "Pass: " p ", Fail: " (f+0)}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```

Acceptance: 0 failures, 0 warnings, clean build. Then dispatch the final cross-cutting opus review. After approval:

```bash
git -C /home/john/projects/shuck checkout main
git -C /home/john/projects/shuck merge --ff-only v27-here-strings
git -C /home/john/projects/shuck branch -d v27-here-strings
```

---

## Self-review checklist

1. **Spec coverage**: every spec section maps to a task.
   - Lexer changes → Task 1.
   - AST changes → Task 1.
   - Parser changes → Task 1.
   - Executor changes → Task 2.
   - Edge cases → Task 3 integration tests.
   - Doc updates → Task 4.

2. **Placeholders**: every step shows concrete code. The "find the per-stage redirect-consumption code" hint in Task 1 Step 7 is a navigation prompt — the implementer reads `src/command.rs` to find the right site, following the `RedirIn` pattern as a model.

3. **Type consistency**: `Redirect::HereString(Word)` flows through `ResolvedStdin::HereString(Word)` and `StdinInput::DeferredHereString(Word)` consistently. Parser → resolve → open_stage_files → expansion-site, all four shapes carry the same Word.

4. **Order dependencies**:
   - Task 1 must precede Task 2 (executor wiring needs the AST variant).
   - Task 2 must precede Task 3 (integration tests need execution to work).
   - Task 4 is independent of code; can ship any time after Task 3.

5. **Backward-compat callouts**: no breaking changes. `<<<` was a parse error pre-v27; now it's a valid redirect. Existing tests don't exercise it; no test should break.
