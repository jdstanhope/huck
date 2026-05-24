# v24: Here-Documents — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement POSIX here-document redirection — `<<DELIM`, `<<'DELIM'` (literal), and `<<-DELIM` (tab-strip) — including multiple here-docs per command, per-stage scoping in pipelines, full POSIX expansion (`$var`/`${var}`/`$(cmd)`/`` `cmd` ``/`\$`/`\\`/`` \` ``) inside expanding heredocs, and lossless multi-line history persistence via escape encoding.

**Architecture:** The lexer collects bodies inline (during the same `tokenize` pass) and emits a new `Token::Heredoc { body: Word, expand: bool, strip_tabs: bool }`. The parser routes that into `ExecCommand.stdin`, which widens from `Option<Word>` to `Option<Redirect>` so that `<file`, `<<EOF`, and stdout-style redirects share a uniform type. The executor expands the body via the existing `expand_assignment` machinery, then pipes the resulting bytes into the child's piped stdin using the same `pending_input` pathway today used for buffered upstream pipeline output. The continuation classifier gains a `Heredoc` reason so the REPL collects body lines via the existing multi-line input loop. History persistence adds tiny `\\` + `\n` escape encoding to round-trip embedded newlines.

**Tech Stack:** Rust 1.95, existing huck modules (`src/lexer.rs`, `src/command.rs`, `src/executor.rs`, `src/continuation.rs`, `src/history.rs`), existing `expand_assignment` (handles B-07 snapshot + tilde + var + command-sub), existing `pending_input: Option<Vec<u8>>` plumbing in `run_multi_stage` and (after Task 4) `run_exec_single`.

**Spec:** `docs/superpowers/specs/2026-05-24-huck-heredocs-design.md`.

**Branch:** `v24-here-docs` (off `main` at commit `dec6621`).

**Baseline:** 944 tests pass, 0 clippy warnings.

---

## File structure

- `src/command.rs` — `Redirect` enum grows `Read(Word)` and `Heredoc{...}` variants; `ExecCommand.stdin` widens to `Option<Redirect>`; parser routes `Token::Heredoc` into `stdin`.
- `src/lexer.rs` — new `Operator::Heredoc` / `HeredocStrip`; new `Token::Heredoc { body, expand, strip_tabs }`; pending-heredoc queue + body collection state machine; `LexError::UnterminatedHeredoc`; delimiter quoted-detection.
- `src/continuation.rs` — `ContinuationReason::Heredoc` variant; `classify` maps `UnterminatedHeredoc`; `joiner_for(Heredoc, _) = "\n"`.
- `src/executor.rs` — `open_stage_files` expands heredoc body and routes through `pending_input` for the stdin side; `run_exec_single` gains the same `pending_input` plumbing as `run_multi_stage`.
- `src/history.rs` — `save` escapes `\\` and `\n`; `load` unescapes.
- `tests/heredoc_integration.rs` (new) — end-to-end coverage.
- `tests/pty_interactive.rs` — 3 new PTY tests for interactive heredoc collection.
- `docs/bash-divergences.md` — M-12 → fixed; change log entry.
- `README.md` — v24 status row.

---

## Task 1: AST refactor — `Redirect::Read` + `Redirect::Heredoc`; `stdin: Option<Redirect>`

Pure AST refactor. After this task, `<file` still works exactly as before (now wrapped as `Redirect::Read(word)`), `Heredoc` variant exists but is never produced by the parser, and zero observable behavior changes.

**Files:**
- Modify: `src/command.rs` (Redirect enum + ExecCommand struct + parser wrapping + all match sites)
- Modify: `src/executor.rs` (every `cmd.stdin` access — currently expects `Word`, now gets `Redirect`)
- Modify: every test helper that builds `ExecCommand { stdin: Some(word), … }` — update to `Some(Redirect::Read(word))`

- [ ] **Step 1: Snapshot baseline**

```bash
cd /home/john/projects/shuck
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print p, f}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```
Expected: `944 0` and `0`.

- [ ] **Step 2: Widen the `Redirect` enum**

In `src/command.rs`, replace:
```rust
pub enum Redirect {
    Truncate(Word),
    Append(Word),
}
```
with:
```rust
pub enum Redirect {
    /// `<file` — open file for reading on stdin.
    Read(Word),
    /// `>file` — open file for writing (truncate first).
    Truncate(Word),
    /// `>>file` — open file for writing (append).
    Append(Word),
    /// `<<DELIM` (and friends) — heredoc body.
    /// `expand` is false for `<<'DELIM'` (any quoted part of the delim
    /// word triggers literal mode). `strip_tabs` is true for `<<-`.
    /// The body has tabs already stripped at lex time for `<<-`.
    Heredoc { body: Word, expand: bool, strip_tabs: bool },
}
```

- [ ] **Step 3: Widen `ExecCommand.stdin`**

In `src/command.rs`:
```rust
pub struct ExecCommand {
    pub inline_assignments: Vec<(String, Word)>,
    pub program: Word,
    pub args: Vec<Word>,
    // BREAKING CHANGE (v24): was Option<Word>; now Option<Redirect> so
    // `<file` (Read), `<<EOF` (Heredoc), and (future) `<<<` share a
    // uniform shape. Last-wins: a later redirect to stdin overwrites
    // an earlier one.
    pub stdin: Option<Redirect>,
    pub stdout: Option<Redirect>,
    pub stderr: Option<Redirect>,
}
```

- [ ] **Step 4: Update the parser**

Find where `cmd.stdin = Some(word)` is set (look for `stdin: Some(` and `cmd.stdin =`). Change every site to wrap the word as `Some(Redirect::Read(word))`. There should be one or two sites in `parse_pipeline_with_first` (or wherever stdin redirects are consumed).

- [ ] **Step 5: Walk the compile-error fanout**

```bash
cargo build 2>&1 | grep "^error\[" | head -50
```

Fix each error:
- `cmd.stdin: Option<Word>` → `Option<Redirect>`: every match arm / destructure / constructor needs to expect or produce a `Redirect`.
- In `src/executor.rs::open_stage_files` (or wherever stdin is opened): the existing `Some(word)` arm becomes `Some(Redirect::Read(word))`; ignore `Some(Redirect::Heredoc { … })` for this task (just `unreachable!("Heredoc handling lands in Task 4")` or treat as a no-op that returns `Ok(None)` — the parser doesn't produce Heredoc yet so it's truly unreachable). Add `_ => unreachable!()` arms for Truncate/Append (invalid for stdin).
- Test helpers: every `stdin: Some(word)` becomes `stdin: Some(Redirect::Read(word))`. Every `stdin: None` is unchanged.

- [ ] **Step 6: Verify zero behavior change**

```bash
cargo build 2>&1 | tail -3
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print p, f}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```
Expected: clean build, 944 0, 0 warnings.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "refactor(ast): Redirect grows Read+Heredoc; ExecCommand.stdin widens to Option<Redirect>

No behavior change. The parser still produces only Read for <file; the
Heredoc variant exists but is unreachable. Sets up the AST shape for
v24's <<EOF lexer and executor work."
```

---

## Task 2: Lexer — `<<` / `<<-` operators, body collection, all variants

The big one. Adds operator tokenization, pending-heredoc queue, body collection (with two modes — literal vs expanding), delimiter quoted-detection, and `UnterminatedHeredoc` error. After this task, `tokenize("cat <<EOF\nhello\nEOF\n")` produces `[Word("cat"), Heredoc { body: …, expand: true, strip_tabs: false }, Newline]`.

**Files:** `src/lexer.rs` only.

- [ ] **Step 1: Add the new operator + token + error variants**

In `src/lexer.rs`:
```rust
pub enum Operator {
    // ... existing ...
    Heredoc,        // <<
    HeredocStrip,   // <<-
}

pub enum Token {
    Word(Word),
    Op(Operator),
    Newline,
    /// A complete here-doc with its body already collected. The lexer
    /// builds this in two phases: the `<<DELIM` opener is seen on one
    /// line, the body lines are consumed after the line's `\n`. The
    /// resulting Token::Heredoc occupies the position where `<<DELIM`
    /// appeared (the delim word itself is not emitted).
    Heredoc { body: Word, expand: bool, strip_tabs: bool },
}

pub enum LexError {
    // ... existing ...
    UnterminatedHeredoc,
}
```

- [ ] **Step 2: Failing test — operator recognition (skeleton)**

In `src/lexer.rs::tests`, add:
```rust
#[test]
fn tokenize_heredoc_op_recognized() {
    // The skeleton: just verify <<EOF parses without panic and emits a
    // single Token::Heredoc with empty body (full body collection comes
    // in later steps).
    let result = tokenize("cat <<EOF\nhello\nEOF\n");
    let tokens = result.expect("parse ok");
    assert_eq!(tokens.len(), 3, "got: {tokens:?}");  // Word("cat"), Heredoc{...}, Newline
    assert!(matches!(tokens[0], Token::Word(_)));
    assert!(matches!(tokens[1], Token::Heredoc { .. }));
    assert!(matches!(tokens[2], Token::Newline));
}
```

Run: `cargo test --bin huck tokenize_heredoc_op_recognized` — expect compile or runtime failure.

- [ ] **Step 3: Implement `<<` / `<<-` operator emission**

In `tokenize`, find the `<` arm (currently emits `RedirIn`). Extend to peek for a second `<` (heredoc) and then a third `-` (strip-tabs):

```rust
'<' => {
    if has_token {
        flush_literal(&mut parts, &mut current, false);
        tokens.push(Token::Word(Word(std::mem::take(&mut parts))));
        has_token = false;
    }
    if chars.peek() == Some(&'<') {
        chars.next(); // consume second '<'
        let strip_tabs = if chars.peek() == Some(&'-') {
            chars.next(); // consume '-'
            true
        } else {
            false
        };
        // Parse the delimiter word and queue this heredoc.
        // (Implemented in next step; for now, push a placeholder.)
        let placeholder_idx = tokens.len();
        tokens.push(Token::Heredoc {
            body: Word(Vec::new()),
            expand: true,
            strip_tabs,
        });
        // Placeholder — actual body collection happens after the line's \n.
        pending_heredocs.push_back(PendingHeredoc {
            delim: parse_heredoc_delim(&mut chars)?,
            expand: /* will compute in delim parser */ true,
            strip_tabs,
            token_idx: placeholder_idx,
        });
    } else {
        tokens.push(Token::Op(Operator::RedirIn));
    }
    in_assignment_value = false;
}
```

Wire `pending_heredocs: VecDeque<PendingHeredoc>` as a local at the top of `tokenize`. Define `PendingHeredoc`:

```rust
struct PendingHeredoc {
    delim: String,
    expand: bool,
    strip_tabs: bool,
    token_idx: usize,
}
```

- [ ] **Step 4: Implement `parse_heredoc_delim`**

After `<<` (or `<<-`), the next chars form the delimiter word. Bash allows the delim to be quoted in various ways; ANY quoted character makes the heredoc literal-mode. Implementation:

```rust
fn parse_heredoc_delim(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
) -> Result<(String, bool /* expand */), LexError> {
    // Skip whitespace (POSIX: `<< EOF` is allowed, though unusual).
    while matches!(chars.peek(), Some(&' ') | Some(&'\t')) {
        chars.next();
    }
    let mut delim = String::new();
    let mut any_quoted = false;
    while let Some(&c) = chars.peek() {
        match c {
            '\n' | ' ' | '\t' | ';' | '&' | '|' | '<' | '>' => break,
            '\'' => {
                chars.next();
                any_quoted = true;
                while let Some(&ch) = chars.peek() {
                    chars.next();
                    if ch == '\'' { break; }
                    delim.push(ch);
                }
            }
            '"' => {
                chars.next();
                any_quoted = true;
                while let Some(&ch) = chars.peek() {
                    chars.next();
                    if ch == '"' { break; }
                    if ch == '\\' {
                        if let Some(&next) = chars.peek() {
                            chars.next();
                            delim.push(next);
                            continue;
                        }
                    }
                    delim.push(ch);
                }
            }
            '\\' => {
                chars.next();
                any_quoted = true;
                if let Some(&next) = chars.peek() {
                    chars.next();
                    delim.push(next);
                }
            }
            _ => {
                chars.next();
                delim.push(c);
            }
        }
    }
    if delim.is_empty() {
        return Err(LexError::UnterminatedHeredoc);
    }
    Ok((delim, !any_quoted))
}
```

Update Step 3 to call this and propagate the `expand` flag:

```rust
let (delim, expand) = parse_heredoc_delim(&mut chars)?;
// ... update both the placeholder token's `expand` and pending_heredocs ...
```

- [ ] **Step 5: Implement body collection on `\n` with pending heredocs**

Find the `\n` (whitespace) arm. Just before emitting `Token::Newline`, drain `pending_heredocs`:

```rust
if !pending_heredocs.is_empty() {
    // The line just ended; collect bodies for each pending heredoc in
    // queue order. Subsequent input chars are consumed as bodies until
    // each close-delimiter line is matched.
    collect_heredoc_bodies(&mut chars, &mut pending_heredocs, &mut tokens)?;
}
```

Define `collect_heredoc_bodies`:

```rust
fn collect_heredoc_bodies(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    pending: &mut VecDeque<PendingHeredoc>,
    tokens: &mut [Token],
) -> Result<(), LexError> {
    while let Some(ph) = pending.pop_front() {
        let body = collect_one_heredoc_body(chars, &ph)?;
        // Patch the placeholder token in tokens[ph.token_idx].
        if let Token::Heredoc { body: slot, expand, strip_tabs } = &mut tokens[ph.token_idx] {
            *slot = body;
            *expand = ph.expand;
            *strip_tabs = ph.strip_tabs;
        } else {
            unreachable!("placeholder token at index was not Heredoc");
        }
    }
    Ok(())
}
```

And `collect_one_heredoc_body`:

```rust
fn collect_one_heredoc_body(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    ph: &PendingHeredoc,
) -> Result<Word, LexError> {
    let mut body_parts: Vec<WordPart> = Vec::new();
    let mut current_line = String::new();
    loop {
        // Read one full line (until \n or EOF).
        current_line.clear();
        loop {
            match chars.next() {
                Some('\n') | None => break,
                Some(c) => current_line.push(c),
            }
        }
        let at_eof = chars.peek().is_none() && !current_line.contains('\n');
        // For <<-, strip leading tabs from both body and close lines.
        let line_for_check = if ph.strip_tabs {
            current_line.trim_start_matches('\t').to_string()
        } else {
            current_line.clone()
        };
        if line_for_check == ph.delim {
            // Found the close. Don't include this line in the body.
            return Ok(Word(body_parts));
        }
        // Not the close — this line is part of the body.
        let body_line = if ph.strip_tabs {
            current_line.trim_start_matches('\t').to_string()
        } else {
            current_line.clone()
        };
        if ph.expand {
            scan_expanding_body_line(&body_line, &mut body_parts)?;
        } else {
            body_parts.push(WordPart::Literal {
                text: body_line,
                quoted: true,
            });
        }
        // Append the line's terminating newline (literal, quoted).
        body_parts.push(WordPart::Literal {
            text: "\n".to_string(),
            quoted: true,
        });
        if at_eof {
            return Err(LexError::UnterminatedHeredoc);
        }
    }
}
```

(The implementer should think carefully about EOF detection — if `chars.next()` returns None mid-line, that's EOF with no close — error. If the line equals the delim, that's a successful close.)

- [ ] **Step 6: Implement `scan_expanding_body_line`**

This scans one body line for `$`, `` ` ``, and `\` per POSIX 2.7.4. Reuse as much of the existing double-quote scanner as possible — the semantics are very similar:

```rust
fn scan_expanding_body_line(
    line: &str,
    parts: &mut Vec<WordPart>,
) -> Result<(), LexError> {
    let mut chars = line.chars().peekable();
    let mut current = String::new();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                // POSIX 2.7.4: inside expanding heredoc, `\` is special
                // only before `$`, `` ` ``, `\`, and newline. Other
                // backslashes are literal.
                match chars.peek().copied() {
                    Some('$') | Some('`') | Some('\\') => {
                        let next = chars.next().unwrap();
                        // Flush current as unquoted, then push escaped char as quoted Literal.
                        flush_body_literal(parts, &mut current, false);
                        parts.push(WordPart::Literal { text: next.to_string(), quoted: true });
                    }
                    // \<NL> handled at caller (we never see literal \n inside one body line).
                    _ => current.push('\\'),
                }
            }
            '$' => {
                flush_body_literal(parts, &mut current, false);
                read_dollar_expansion(&mut chars, parts, /* quoted = */ true)?;
                // NOTE: heredoc body chars are conceptually inside double-quote-like
                // semantics, so `$var` here is the QUOTED variant (no word splitting).
            }
            '`' => {
                flush_body_literal(parts, &mut current, false);
                let sequence = scan_backtick_substitution(&mut chars)?;
                parts.push(WordPart::CommandSub { sequence, quoted: true });
            }
            other => current.push(other),
        }
    }
    flush_body_literal(parts, &mut current, false);
    Ok(())
}

fn flush_body_literal(parts: &mut Vec<WordPart>, current: &mut String, quoted: bool) {
    if !current.is_empty() {
        parts.push(WordPart::Literal {
            text: std::mem::take(current),
            quoted,
        });
    }
}
```

Note: the `quoted: true` flag on `read_dollar_expansion`'s `$var` is critical — it matches POSIX (heredoc bodies are quoted-context for the purposes of word-splitting suppression).

- [ ] **Step 7: Handle end-of-input with unresolved heredocs**

After the main `while let Some(c) = chars.next()` loop, BEFORE emitting any trailing Word, check:
```rust
if !pending_heredocs.is_empty() {
    return Err(LexError::UnterminatedHeredoc);
}
```

This catches `cat <<EOF` (with no body at all — end-of-input immediately) and `cat <<EOF\nbody` (with body but no close).

- [ ] **Step 8: Add the full test suite from the spec**

Add the 15 lexer tests from the spec's Lexer test table to `src/lexer.rs::tests`. Cover:
- Simple expanding
- Literal (quoted delim)
- Strip-tabs
- Strip-tabs composed with literal
- Unclosed errors
- Close-must-match-exactly (trailing whitespace fails)
- Close-no-leading-spaces (without `<<-`, leading spaces fail)
- Multiple in order
- Body var part
- Body command-sub
- Body escape-dollar
- Body backslash-passthrough (`\d` → literal `\d`)
- Empty body
- Delim partially quoted → literal mode
- Delim backslash-escaped → literal mode

For each test, write the assertion against the expected `Token::Heredoc { body, expand, strip_tabs }` shape.

- [ ] **Step 9: Verify**

```bash
cargo test --bin huck tokenize_heredoc 2>&1 | tail -10
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print p, f}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```
Expected: all 15+ heredoc tests pass, 0 fails total, 0 warnings.

- [ ] **Step 10: Commit**

```bash
git add -A
git commit -m "lex: <<EOF / <<'EOF' / <<-EOF heredoc tokenization

Lexer now emits Token::Heredoc { body, expand, strip_tabs } for the three
POSIX heredoc forms. Pending-heredoc queue handles multiple heredocs on
one command. Delimiter quoted-detection triggers literal-mode body
collection (no expansion). <<- strips leading tabs from body and close
delimiter line. UnterminatedHeredoc error surfaces when end-of-input is
hit with unresolved heredocs."
```

---

## Task 3: Continuation classifier — `Heredoc` reason + joiner

Small. Adds the new reason variant, maps `UnterminatedHeredoc` to `Incomplete(Heredoc)`, sets `joiner_for(Heredoc, _)` to `"\n"`.

**Files:** `src/continuation.rs`.

- [ ] **Step 1: Failing tests**

In `src/continuation.rs::tests`:
```rust
#[test]
fn classify_heredoc_unclosed_is_incomplete() {
    assert_eq!(
        classify("cat <<EOF\nhello"),
        Completeness::Incomplete(ContinuationReason::Heredoc)
    );
}

#[test]
fn classify_heredoc_closed_is_complete() {
    assert_eq!(
        classify("cat <<EOF\nhello\nEOF\n"),
        Completeness::Complete
    );
}

#[test]
fn joiner_for_heredoc_is_newline() {
    assert_eq!(joiner_for(ContinuationReason::Heredoc, ""), "\n");
}
```

Run: `cargo test --bin huck classify_heredoc joiner_for_heredoc` → expect compile failure (no `Heredoc` variant yet).

- [ ] **Step 2: Add the variant + map the error + handle the joiner**

```rust
pub enum ContinuationReason {
    Backslash,
    Operator,
    OpenQuote,
    Compound,
    Heredoc,  // NEW
}

fn is_unterminated_lex(e: &LexError) -> bool {
    matches!(
        e,
        LexError::UnterminatedQuote
            | LexError::UnterminatedBrace
            | LexError::UnterminatedSubstitution
            | LexError::UnterminatedArith
            | LexError::UnterminatedHeredoc  // NEW
    )
}
```

But heredoc needs a DIFFERENT reason than OpenQuote (because the joiner is `"\n"` not `"; "`). So splitting is needed in `classify`:

```rust
pub fn classify(buffer: &str) -> Completeness {
    if ends_with_continuation_backslash(buffer) {
        return Completeness::Incomplete(ContinuationReason::Backslash);
    }
    let tokens = match lexer::tokenize(buffer) {
        Ok(tokens) => tokens,
        Err(LexError::UnterminatedHeredoc) => {
            return Completeness::Incomplete(ContinuationReason::Heredoc);
        }
        Err(e) if is_unterminated_lex(&e) => {
            return Completeness::Incomplete(ContinuationReason::OpenQuote);
        }
        Err(_) => return Completeness::Error,
    };
    // ... existing ...
}
```

(Remove `UnterminatedHeredoc` from `is_unterminated_lex` since it's now handled explicitly with its own reason; OR keep it in the helper and rely on the explicit match arriving first. Implementer's choice — make sure the explicit `Heredoc` arm runs before the generic OpenQuote arm.)

```rust
pub fn joiner_for(reason: ContinuationReason, last_line: &str) -> &'static str {
    match reason {
        ContinuationReason::Backslash => "",
        ContinuationReason::Operator => " ",
        ContinuationReason::OpenQuote => "; ",
        ContinuationReason::Compound => {
            if ends_with_control_keyword(last_line) {
                " "
            } else {
                "; "
            }
        }
        ContinuationReason::Heredoc => "\n",  // NEW
    }
}
```

- [ ] **Step 3: Verify**

```bash
cargo test --bin huck classify_heredoc joiner_for_heredoc 2>&1 | tail -5
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print p, f}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```
Expected: 3 new tests pass, 0 fails, 0 warnings.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "continuation: Heredoc reason + newline joiner

classify maps LexError::UnterminatedHeredoc to Incomplete(Heredoc) so
the REPL keeps collecting body lines. joiner_for returns \\\"\\\\n\\\" so
appended lines preserve heredoc body fidelity."
```

---

## Task 4: Executor — heredoc body → child stdin via pending_input

Wires the runtime side. After this task, `cat <<EOF\nhello\nEOF` actually pipes "hello\n" into cat's stdin.

**Files:** `src/executor.rs`.

- [ ] **Step 1: Failing integration test**

Create `tests/heredoc_integration.rs`:
```rust
//! End-to-end tests for v24 here-documents. Spawn huck with piped stdin
//! so the full lex → classify → parse → execute path runs.

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
fn heredoc_simple_expand_no_vars() {
    let (out, _) = run("cat <<EOF\nhello\nEOF\nexit\n");
    assert!(out.contains("hello"), "got: {out}");
}
```

Run: `cargo test --test heredoc_integration heredoc_simple_expand_no_vars` → expect failure (executor doesn't handle Heredoc redirect yet).

- [ ] **Step 2: Wire heredoc handling into `open_stage_files`**

Find `open_stage_files` in `src/executor.rs`. Its stdin handling currently looks like:
```rust
let stdin = match &cmd.stdin {
    Some(redirect) => Some(open_resolved(...)?),  // or similar
    None => None,
};
```

Widen to handle `Redirect::Heredoc`:
```rust
let stdin_input: StdinInput = match &cmd.stdin {
    None => StdinInput::None,
    Some(Redirect::Read(path_word)) => {
        let path = expand_single(path_word, shell);
        StdinInput::File(File::open(&path)?)
    }
    Some(Redirect::Heredoc { body, .. }) => {
        // expand_assignment gives no-split, no-glob semantics — perfect
        // for heredoc body where the parts are already lex-time-classified
        // as quoted (no splitting wanted).
        let bytes = expand_assignment(body, shell).into_bytes();
        StdinInput::Bytes(bytes)
    }
    Some(Redirect::Truncate(_) | Redirect::Append(_)) => {
        unreachable!("parser produces only Read/Heredoc for stdin");
    }
};
```

Define a small local enum `StdinInput { None, File(File), Bytes(Vec<u8>) }` (or extend `StageFiles` similarly).

- [ ] **Step 3: Route `StdinInput::Bytes` through the existing `pending_input` plumbing in `run_multi_stage`**

Find `pending_input: Option<Vec<u8>>` in `run_multi_stage`. When the stage's stdin is `StdinInput::Bytes(b)`, set `pending_input = Some(b)`. The existing code at ~line 1051 (`if let Some(bytes) = pending_input && let Some(mut child_stdin) = child.stdin.take()`) handles the write.

Make sure `process.stdin(Stdio::piped())` is set when bytes are pending. The existing logic does this for the `Carry::Buffer` case; extend to do it for any `pending_input.is_some()`.

- [ ] **Step 4: Add the same `pending_input` plumbing to `run_exec_single`**

`run_exec_single` doesn't currently use `pending_input` (it's a single-stage path, no upstream pipeline). For heredocs, it needs the same write-bytes-to-piped-stdin treatment. Mirror the `run_multi_stage` pattern: if stdin is heredoc bytes, configure piped stdin and write before waiting.

- [ ] **Step 5: Add the integration tests from the spec**

Add the remaining 11 integration tests from the spec's Integration test table to `tests/heredoc_integration.rs`. Cover:
- `heredoc_literal_no_expand` — `FOO=secret cat <<'EOF'\n$FOO\nEOF` → `$FOO`
- `heredoc_expand_var` — `FOO=hi cat <<EOF\n$FOO\nEOF` → `hi`
- `heredoc_expand_cmd_sub` — `cat <<EOF\n$(echo via-sub)\nEOF` → `via-sub`
- `heredoc_strip_tabs` — `cat <<-EOF\n\t\thello\n\tEOF` → `hello`
- `heredoc_in_pipeline` — `cat <<EOF | grep marker\nmarker\nother\nEOF` → `marker`
- `heredoc_multiple_per_command_last_wins` — `cat <<A <<B\nfirst\nA\nsecond\nB` → `second`
- `heredoc_empty_body` — `cat <<EOF\nEOF` → (empty)
- `heredoc_with_inline_assignment_expand` — `FOO=hi cat <<EOF\nval=$FOO\nEOF` → `val=hi`
- `heredoc_escape_dollar` — `cat <<EOF\n\\$NOT_EXPANDED\nEOF` → `$NOT_EXPANDED`
- `heredoc_multi_line_body` — `cat <<EOF\nline1\nline2\nline3\nEOF` → `line1\nline2\nline3`
- `heredoc_backgrounded_command_sees_body` — uses temp file like the v23 background test

- [ ] **Step 6: Verify**

```bash
cargo test --test heredoc_integration 2>&1 | tail -20
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print p, f}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```
Expected: all heredoc integration tests pass, full suite 0 fails, 0 warnings.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "exec: pipe heredoc body bytes into child stdin

open_stage_files now expands Redirect::Heredoc bodies via expand_assignment
(no-split, no-glob semantics for the body Word). Resulting bytes route
through the existing pending_input pipeline that previously only fed
buffered upstream pipeline output into the next stage. run_exec_single
gains the same pending_input plumbing for the single-stage path."
```

---

## Task 5: History — escape encoding for `\\` + `\n`

Adds lossless persistence of multi-line history entries (per spec option B).

**Files:** `src/history.rs`.

- [ ] **Step 1: Failing tests**

In `src/history.rs::tests`:
```rust
#[test]
fn history_round_trips_embedded_newline() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hf");
    {
        let mut h = History::new(Some(path.clone()), 1000);
        h.add("cat <<EOF\nhello\nworld\nEOF".to_string());
        h.save();
    }
    let mut h2 = History::new(Some(path.clone()), 1000);
    h2.load();
    let entries: Vec<String> = h2.entries().map(|(_, s)| s.clone()).collect();
    assert_eq!(entries, vec!["cat <<EOF\nhello\nworld\nEOF".to_string()]);
}

#[test]
fn history_round_trips_literal_backslash() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hf");
    {
        let mut h = History::new(Some(path.clone()), 1000);
        h.add(r"echo a\b".to_string());
        h.save();
    }
    let mut h2 = History::new(Some(path.clone()), 1000);
    h2.load();
    let entries: Vec<String> = h2.entries().map(|(_, s)| s.clone()).collect();
    assert_eq!(entries, vec![r"echo a\b".to_string()]);
}

#[test]
fn history_loads_pre_v24_format_without_escapes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hf");
    std::fs::write(&path, "echo hi\nls -la\n").unwrap();
    let mut h = History::new(Some(path.clone()), 1000);
    h.load();
    let entries: Vec<String> = h.entries().map(|(_, s)| s.clone()).collect();
    assert_eq!(entries, vec!["echo hi".to_string(), "ls -la".to_string()]);
}
```

(Adapt `History::new` signature to match the actual constructor; inspect `src/history.rs` first.)

Run: `cargo test --bin huck history_round_trips history_loads_pre_v24` → expect failures (no escape encoding yet).

- [ ] **Step 2: Implement escape encoding**

In `History::save`:
```rust
fn escape_for_save(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str(r"\\"),
            '\n' => out.push_str(r"\n"),
            other => out.push(other),
        }
    }
    out
}
```

In `History::load`:
```rust
fn unescape_for_load(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.peek() {
                Some('\\') => { chars.next(); out.push('\\'); }
                Some('n') => { chars.next(); out.push('\n'); }
                _ => out.push('\\'),  // \X where X isn't n or \ → keep literal \
            }
        } else {
            out.push(c);
        }
    }
    out
}
```

Then call `escape_for_save` on each entry before writing, and `unescape_for_load` after reading each line.

- [ ] **Step 3: Verify**

```bash
cargo test --bin huck history_ 2>&1 | tail -10
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print p, f}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```
Expected: 3+ history tests pass, full suite 0 fails, 0 warnings.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "history: escape \\\\ and \\\\n for multi-line entries

Round-trips heredoc bodies and other newline-containing entries through
the on-disk history file losslessly. Backward-compat for pre-v24 files:
unescape only recognises \\\\\\\\ and \\\\n; any other \\X stays literal so
existing entries with embedded backslashes load unchanged."
```

---

## Task 6: PTY tests + docs

Wraps up.

**Files:**
- Modify: `tests/pty_interactive.rs` — add 3 PTY tests.
- Modify: `docs/bash-divergences.md` — M-12 → fixed; change log.
- Modify: `README.md` — v24 status row.

- [ ] **Step 1: Add PTY tests**

In `tests/pty_interactive.rs`, append (using the existing test patterns from v23 / earlier):

```rust
#[test]
fn pty_heredoc_simple() {
    let dir = tempfile::tempdir().unwrap();
    let env = histfile_env(dir.path());
    let Some(mut session) = try_spawn(dir.path(), &env_refs(&env)) else {
        return;
    };
    expect(&mut session, "huck> ");
    send(&mut session, "cat <<EOF");
    send(&mut session, ENTER);
    expect(&mut session, "> ");
    send(&mut session, "PTY_HEREDOC_MARKER");
    send(&mut session, ENTER);
    expect(&mut session, "> ");
    send(&mut session, "EOF");
    send(&mut session, ENTER);
    expect(&mut session, "PTY_HEREDOC_MARKER");
    send(&mut session, "exit");
    send(&mut session, ENTER);
}

#[test]
fn pty_heredoc_continuation_prompt_appears() {
    let dir = tempfile::tempdir().unwrap();
    let env = histfile_env(dir.path());
    let Some(mut session) = try_spawn(dir.path(), &env_refs(&env)) else {
        return;
    };
    expect(&mut session, "huck> ");
    send(&mut session, "cat <<EOF");
    send(&mut session, ENTER);
    expect(&mut session, "> ");
    send(&mut session, CTRL_C);
    expect(&mut session, "huck> ");
    send(&mut session, "exit");
    send(&mut session, ENTER);
}

#[test]
fn pty_heredoc_ctrl_c_aborts_body_collection() {
    let dir = tempfile::tempdir().unwrap();
    let env = histfile_env(dir.path());
    let Some(mut session) = try_spawn(dir.path(), &env_refs(&env)) else {
        return;
    };
    expect(&mut session, "huck> ");
    send(&mut session, "cat <<EOF");
    send(&mut session, ENTER);
    expect(&mut session, "> ");
    send(&mut session, "partial body");
    send(&mut session, ENTER);
    expect(&mut session, "> ");
    send(&mut session, CTRL_C);
    expect(&mut session, "huck> ");
    // Buffer was discarded — confirm by running a fresh command.
    send(&mut session, "pwd");
    send(&mut session, ENTER);
    let marker = dir.path().file_name().unwrap().to_str().unwrap();
    expect(&mut session, marker);
    send(&mut session, "exit");
    send(&mut session, ENTER);
}
```

- [ ] **Step 2: Update `docs/bash-divergences.md`**

Find the M-12 line under "Compound commands" and replace its bullet with:
```markdown
- **M-12: Here-documents `<<EOF`** — `[fixed (2026-05-24)]` high. Now supported: `<<DELIM` (expanding), `<<'DELIM'` (literal), `<<-DELIM` (tab-strip), composable; multiple here-docs per command; per-stage in pipelines; full POSIX expansion ($var, ${var}, $(cmd), backticks, \\$, \\\\, \\`).
```

Add change-log entry:
```markdown
- **2026-05-24**: M-12 (here-documents) shipped as v24. Also reshapes ExecCommand.stdin from Option<Word> to Option<Redirect> so <file, <<EOF, and future <<<word share a uniform shape.
```

- [ ] **Step 3: Update `README.md`**

Add the v24 row to the status table:
```
| v24       | Here-documents (`<<EOF`, `<<'EOF'`, `<<-EOF`)            |
```

- [ ] **Step 4: Verify**

```bash
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print p, f}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```
Expected: 0 fails, 0 warnings. Test count should be ~985 (944 baseline + ~41 new tests across all six tasks).

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "v24: here-docs — PTY tests + docs

3 PTY tests verify interactive collection (prompt, body display, Ctrl-C
abort). docs/bash-divergences.md M-12 marked fixed with full description.
README.md status table gets v24 row."
```

---

## Final verification (no separate task)

```bash
cargo build 2>&1 | tail -3
cargo test 2>&1 | grep -E "^test result:" | awk '{p+=$4; if($6>0) f+=$6} END {print "Pass: " p ", Fail: " (f+0)}'
cargo clippy --bin huck --all-targets 2>&1 | grep -c "^warning:"
```

Acceptance: 0 failures, 0 warnings, clean build. Then dispatch the **final cross-cutting reviewer** (per the v23 pattern — use opus) over the whole `v24-here-docs` branch diff. This reviewer specifically looks for things that surface only when looking at the whole changeset: AST shape consistency across modules, edge-case interactions between heredoc + inline assignments + pipelines + backgrounded commands, etc.

After review approval, merge `v24-here-docs` to `main`:
```bash
git -C /home/john/projects/shuck checkout main
git -C /home/john/projects/shuck merge --ff-only v24-here-docs
git -C /home/john/projects/shuck branch -d v24-here-docs
```

(Use `--ff-only` because the branch is strictly ahead of main with no main commits since branch creation. If main has moved during work, switch to `--no-ff` and resolve any conflicts.)

---

## Self-review checklist

1. **Spec coverage**: every section in the spec has a corresponding task.
   - Lexer (spec §3) → Task 2.
   - AST (spec §4) → Task 1.
   - Classifier (spec §5) → Task 3.
   - Executor (spec §6) → Task 4.
   - History (spec §7) → Task 5.
   - Edge cases (spec §8) → tested across Tasks 2/4 integration tests.
   - Tests (spec §9) → distributed across Tasks 2/3/4/5/6.
   - Out-of-scope → not in any task, as expected.

2. **Placeholders**: every step shows concrete code/commands. The exact signatures for helpers (`PendingHeredoc`, `parse_heredoc_delim`, `collect_one_heredoc_body`, `scan_expanding_body_line`) are spelled out.

3. **Type consistency**: `Token::Heredoc { body: Word, expand: bool, strip_tabs: bool }` matches `Redirect::Heredoc { body: Word, expand: bool, strip_tabs: bool }` exactly — parser threads them through.

4. **Order dependencies**: AST refactor (Task 1) must precede everything that touches `cmd.stdin`. Lexer (Task 2) must precede classifier (Task 3) because the classifier maps the new error variant. Executor (Task 4) and history (Task 5) are independent of each other but both depend on Tasks 1-3. Integration tests (Task 6) depend on Tasks 1-5.

5. **Backward-compat callouts**: the AST shape change in Task 1 is explicitly called out. The history format change in Task 5 specifies how pre-v24 files load.
