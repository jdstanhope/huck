# v242 — Parser-Driven Flat Command-List Parser Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a flat command-list parser in `crates/huck-syntax/src/parser.rs` that consumes the production `Command`-mode tokens and produces the same `Sequence` AST `command.rs` does, dormant and differentially-tested against `command.rs`.

**Architecture:** New `parse_sequence`/`parse_and_or`/`parse_pipeline`/`parse_command`/`parse_simple`/`parse_redirects` in `parser.rs`, reusing the existing AST (`Sequence`/`Pipeline`/`Command`/`SimpleCommand`/`ExecCommand`/`Assignment`/`Redirection`). It mirrors `command.rs`'s parser functions for the flat subset and returns the NEW `ParseError::UnsupportedCommand` for any deferred construct. No lexer change; `command.rs` untouched.

**Tech Stack:** Rust, `crates/huck-syntax/src/{parser.rs, command.rs, errors.rs}`.

## Global Constraints

- **Dormant / byte-identical:** `parser.rs`'s command parser is reached ONLY by tests; `command.rs`'s parser, `Command` mode, and the engine are untouched. `cargo test --workspace` green, 0 warnings; the `*_diff_check.sh` release sweep byte-identical.
- **No lexer change** — consume the production `Command`-mode token stream via the existing pull API (`peek_kind`/`peek2_kind`/`next_kind`/`peek_span`/`next`). No new `Mode`/`TokenKind`.
- **`command.rs` is the ORACLE.** When a differential case disagrees, fix `parser.rs` to match `command.rs` — never weaken the comparison, never edit `command.rs`.
- Reuse the existing AST verbatim — NO AST change (engine untouched). Add only `ParseError::UnsupportedCommand`.
- All new parsing code in `crates/huck-syntax/src/parser.rs`; the only other edits are the `ParseError` variant (`command.rs`) + its message arm (`errors.rs`).
- Commit trailer on every commit: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

## File Structure

- `crates/huck-syntax/src/command.rs` — add ONE `ParseError::UnsupportedCommand` variant (enum at ~line 731).
- `crates/huck-syntax/src/errors.rs` — add its message arm.
- `crates/huck-syntax/src/parser.rs` — all the new parser fns + the differential test harness/corpus.

## Reference (the oracle to mirror — read these in `command.rs`)

`parse` (793), `parse_command_then_pipeline` (819), `parse_sequence_opts` (872), `parse_command_inner` (1013), `parse_simple_stage` (2134), `parse_next_stage` (2296), `parse_pipeline` (2414), `parse_trailing_redirects` (2055), `next_is_redirect` (2041), `try_split_assignment` (96), `keyword_of` (57), `finalize_stage` (208). The new `parser.rs` fns reimplement the FLAT subset of these; deferred constructs short-circuit to `UnsupportedCommand` where `command.rs` branches into a compound.

---

### Task 1: Scaffolding — `UnsupportedCommand` + `parse_sequence` skeleton + differential harness

**Files:**
- Modify: `crates/huck-syntax/src/command.rs` (`enum ParseError` ~731)
- Modify: `crates/huck-syntax/src/errors.rs` (message arm)
- Modify: `crates/huck-syntax/src/parser.rs` (add fns + test harness)

**Interfaces produced (Tasks 2–6 depend on these):**
- `command::ParseError::UnsupportedCommand`
- `pub(crate) fn parser::parse_sequence(iter: &mut Lexer) -> Result<Option<Sequence>, ParseError>` (skeleton: `unimplemented!()` for now)
- test helpers `old_seq`/`new_seq`/`diff_cmd`/`diff_unsupported`

- [ ] **Step 1: Write the failing test** — in `parser.rs`'s `#[cfg(test)] mod tests`:
```rust
#[test]
fn v242_scaffolding_exists() {
    let _ = crate::command::ParseError::UnsupportedCommand;
    // harness compiles + the entry is callable
    let _ = old_seq("echo a");
}
```

- [ ] **Step 2: Run to verify it fails** — `cargo test -p huck-syntax --lib v242_scaffolding_exists 2>&1 | tail`. Expected: compile error (unknown variant / `old_seq` undefined).

- [ ] **Step 3: Implement.**
(a) `command.rs` `enum ParseError` — add:
```rust
    /// A command-level construct the parser-driven flat command parser does not
    /// model yet (subshell, arith command, compound command, heredoc, …). v242 boundary.
    UnsupportedCommand,
```
(b) `errors.rs` — add the matching message arm (mirror a neighboring arm), e.g. `ParseError::UnsupportedCommand => "unsupported command".to_string(),`.
(c) `parser.rs` — add imports + the entry skeleton + the harness:
```rust
use crate::command::{Command, Sequence, Pipeline, SimpleCommand, ExecCommand, Assignment, Connector, ParseError};
use crate::lexer::{Lexer, Operator, TokenKind, Word};

pub(crate) fn parse_sequence(iter: &mut Lexer) -> Result<Option<Sequence>, ParseError> {
    let _ = iter;
    unimplemented!("parse_sequence: Task 2")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::{tokenize_with_opts, LexerOptions, Lexer};

    fn old_seq(s: &str) -> Result<Option<Sequence>, ParseError> {
        let toks = tokenize_with_opts(s, LexerOptions::default()).expect("lex");
        crate::command::parse(&mut Lexer::from_tokens(toks))
    }
    fn new_seq(s: &str) -> Result<Option<Sequence>, ParseError> {
        let toks = tokenize_with_opts(s, LexerOptions::default()).expect("lex");
        parse_sequence(&mut Lexer::from_tokens(toks))
    }
    /// In-scope: the new parser must produce the SAME AST as command.rs (the oracle).
    fn diff_cmd(s: &str) {
        assert_eq!(new_seq(s).unwrap(), old_seq(s).unwrap(), "command AST mismatch for {s:?}");
    }
    /// Deferred: the new parser must return UnsupportedCommand.
    fn diff_unsupported(s: &str) {
        assert!(matches!(new_seq(s), Err(ParseError::UnsupportedCommand)),
                "expected UnsupportedCommand for {s:?}, got {:?}", new_seq(s));
    }
    // tests added in later tasks
}
```
NOTE: `parser.rs` already begins with `#![allow(dead_code, unused_imports)]` (from v241) — keep it; the new dormant fns are dead until Stage 2 wires them in.

- [ ] **Step 4: Run to verify it passes** — `cargo test -p huck-syntax --lib v242_scaffolding_exists 2>&1 | grep "test result"` PASS; `cargo build --workspace 2>&1 | grep -E "error|warning" || echo clean` (clean — additive variant + dead fns).

- [ ] **Step 5: Commit**
```bash
git add crates/huck-syntax/src/command.rs crates/huck-syntax/src/errors.rs crates/huck-syntax/src/parser.rs
git commit -m "v242 T1: ParseError::UnsupportedCommand + parse_sequence skeleton + differential harness

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: The full chain for a single simple command (program + args) + the deferred boundary

**Files:** Modify `crates/huck-syntax/src/parser.rs`.

**Interfaces produced:** `parse_and_or`, `parse_pipeline`, `parse_command`, `parse_simple` (program + args only); `parse_sequence` now works for a single simple command.

The chain mirrors `command.rs`: `parse_sequence` (≈ `parse`, returns `None` on empty/newlines-only) → `parse_and_or` (≈ `parse_sequence_opts`, **Task 6** adds connectors; Task 2 returns a 1-command Sequence) → `parse_command` (dispatch) → `parse_simple` (build `ExecCommand`).

`parse_command` DISPATCH (mirror `parse_command_inner` 1013 + `keyword_of` 57) — for v242, every non-simple branch returns `UnsupportedCommand`:
- `peek_kind` is `None` → `Err(MissingCommand)` (match command.rs).
- `Op(LParen)` → `UnsupportedCommand` (subshell).
- `TokenKind::ArithBlock(..)` → `UnsupportedCommand` (arith command).
- `TokenKind::Heredoc{..}` / `Op(HereString)` → `UnsupportedCommand`.
- a Word that `keyword_of` maps to a reserved word (`if/then/elif/else/fi/while/until/do/done/for/in/case/esac/select/function/{/}/[[/]]/time`) → `UnsupportedCommand`.
- a Word `name` with `peek2_kind == Op(LParen)` (function def) → `UnsupportedCommand`.
- otherwise → `parse_simple`.

`parse_simple` (mirror `parse_simple_stage` 2134, FLAT subset — no assignments/redirects yet in Task 2): capture the first-token line via `peek_span` (`ExecCommand.line`); collect `Word`s as program (first) then args, stopping (without consuming) at a stage/list terminator (`Op(Pipe|Semi|And|Or|Background|RParen|DoubleSemi|SemiAmp|DoubleSemiAmp)`, `Newline`, EOF). On a redirect op / `RedirFd` / `Heredoc` mid-command → leave for Task 4 (Task 2: treat a redirect op as `UnsupportedCommand` for now, then Task 4 implements it). Build `Command::Simple(SimpleCommand::Exec(ExecCommand { inline_assignments: vec![], program, args, redirects: vec![], line }))`.

Confirm from `command.rs` whether a single simple command is wrapped as `Command::Simple` (it is) and how `parse_sequence`/`parse` return it inside `Sequence{first, rest:[], background:false}` — reproduce exactly (the differential pins it).

- [ ] **Step 1: Write failing tests:**
```rust
#[test]
fn cmd_single_simple() {
    diff_cmd("echo");
    diff_cmd("echo a");
    diff_cmd("echo a b c");
    diff_cmd("echo \"$x\" 'y' z");
    assert_eq!(new_seq("").unwrap(), None);          // empty input
    assert_eq!(new_seq("\n\n").unwrap(), None);       // only newlines
}
#[test]
fn cmd_deferred_boundary() {
    for s in ["( a )", "(( 1+2 ))", "if true; then x; fi", "while x; do y; done",
              "for i in a; do x; done", "case x in y) z;; esac", "{ a; }",
              "[[ -n x ]]", "f() { x; }", "coproc x"] {
        diff_unsupported(s);
    }
}
```

- [ ] **Step 2: Run to verify they fail** — `cargo test -p huck-syntax --lib cmd_ 2>&1 | tail -20` (panics at `unimplemented!`).

- [ ] **Step 3: Implement** `parse_sequence`/`parse_and_or` (single-command form)/`parse_command`/`parse_simple` per the dispatch above, mirroring the cited `command.rs` fns. Words are consumed via `next_kind`; the program/args `Word`s are taken from `TokenKind::Word(w)`.

- [ ] **Step 4: Run to verify they pass** — `cargo test -p huck-syntax --lib cmd_ 2>&1 | grep "test result"` PASS. If a `diff_cmd` mismatches, fix `parse_simple`/`parse_command` to match `old_seq` (the oracle). Full lexer suite still green: `cargo test -p huck-syntax 2>&1 | grep "test result" | tail -2`.

- [ ] **Step 5: Commit**
```bash
git add crates/huck-syntax/src/parser.rs
git commit -m "v242 T2: single simple command (program+args) + deferred-command boundary

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Assignments (inline prefix + bare `Assign`)

**Files:** Modify `crates/huck-syntax/src/parser.rs`.

**Interfaces produced:** `parse_simple` now handles leading assignments and a bare-assignment-only line.

Mirror `command.rs`'s `try_split_assignment` (96) + `finalize_stage` (208): leading words that are assignments (`WordPart::AssignPrefix{target,append}` OR a `Literal` `NAME=value`) become `inline_assignments`; the first NON-assignment word is the program. A line of ONLY assignments (no program) → `Command::Simple(SimpleCommand::Assign(assignments, line))`. Use `crate::command::try_split_assignment_ref` (173) if it is `pub` — it returns `Option<Assignment>` for a `&Word` without consuming, ideal for the leading-prefix loop; otherwise reproduce its logic.

- [ ] **Step 1: Write failing tests:**
```rust
#[test]
fn cmd_assignments() {
    diff_cmd("A=1 cmd");
    diff_cmd("A=1 B=2 cmd x y");
    diff_cmd("A=1");                 // bare assign -> SimpleCommand::Assign
    diff_cmd("A=1 B=2");             // bare multi-assign
    diff_cmd("A=$x cmd");
    diff_cmd("A+=v cmd");            // append
    diff_cmd("arr[0]=v cmd");        // subscripted (AssignPrefix)
    diff_cmd("PATH=/x:/y cmd");
}
```

- [ ] **Step 2: Run to verify they fail** — `cargo test -p huck-syntax --lib cmd_assignments 2>&1 | tail` (mismatch — assignments treated as program/args).
- [ ] **Step 3: Implement** the leading-assignment loop + bare-`Assign` case in `parse_simple`, mirroring `try_split_assignment`.
- [ ] **Step 4: Run to verify they pass** — PASS; fix to match `old_seq` on any mismatch. Lexer suite green.
- [ ] **Step 5: Commit**
```bash
git add crates/huck-syntax/src/parser.rs
git commit -m "v242 T3: inline + bare assignments

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Redirects (file/fd operators + `RedirFd` prefixes, source order)

**Files:** Modify `crates/huck-syntax/src/parser.rs`.

**Interfaces produced:** `parse_redirects` (or inline in `parse_simple`) builds `Vec<Redirection>` in source order; interleaves with words.

Mirror `command.rs`'s `next_is_redirect` (2041) + `parse_trailing_redirects` (2055): a redirect is a `RedirFd` token (glued fd prefix) followed by a redirect `Op`, OR a redirect `Op` directly; then a `Word` target (or another fd for dups like `2>&1`). Build `Redirection` exactly as `command.rs` does. Redirects may appear before, between, or after words and must be collected in SOURCE ORDER into `ExecCommand.redirects` while words go to program/args. Replace Task 2's "redirect op → UnsupportedCommand" stub with real handling. **Heredocs / here-strings stay `UnsupportedCommand`** (deferred): a `TokenKind::Heredoc` or `Op(HereString)` → `UnsupportedCommand`.

- [ ] **Step 1: Write failing tests:**
```rust
#[test]
fn cmd_redirects() {
    diff_cmd("cmd >out");
    diff_cmd("cmd >>out");
    diff_cmd("cmd <in");
    diff_cmd("cmd 2>err");
    diff_cmd("cmd >out 2>&1");
    diff_cmd("cmd 2>&1 >out");        // order matters
    diff_cmd(">out cmd");             // leading redirect
    diff_cmd("cmd a >o b <i c");      // interleaved
    diff_cmd("3>f cmd");              // RedirFd prefix
    diff_cmd("cmd >|f");              // clobber
    diff_cmd("cmd <>f");              // read-write
    diff_cmd("cmd <&3");              // dup-in
    diff_cmd("cmd &>f");              // and-redirect
}
#[test]
fn cmd_heredoc_deferred() {
    diff_unsupported("cat <<<word");
    // (heredoc body cases need a newline; keep to here-string for the dispatch test)
}
```

- [ ] **Step 2: Run to verify they fail** — mismatch / UnsupportedCommand from the Task-2 stub.
- [ ] **Step 3: Implement** redirect parsing mirroring `parse_trailing_redirects`; keep heredoc/here-string deferred.
- [ ] **Step 4: Run to verify they pass** — PASS; oracle-fix on mismatch. Lexer suite green.
- [ ] **Step 5: Commit**
```bash
git add crates/huck-syntax/src/parser.rs
git commit -m "v242 T4: redirects (file/fd ops + RedirFd, source order); heredocs deferred

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Pipelines (`!` negate + `|` stages)

**Files:** Modify `crates/huck-syntax/src/parser.rs`.

**Interfaces produced:** `parse_pipeline` builds `Command::Pipeline{negate, commands}` for multi-stage, `Command::Simple` for a single stage; `parse_and_or` calls it.

Mirror `command.rs`'s `parse_pipeline` (2414) + `parse_command_then_pipeline` (819) + `parse_next_stage` (2296): an optional leading `!` (toggle `negate`); stages are simple commands joined by `Op(Pipe)` (skip newlines after `|`). A single stage stays `Command::Simple`; multiple → `Command::Pipeline{negate, commands: vec![Command::Simple,…]}`. Reproduce command.rs's exact single-vs-pipeline shape and `!` handling.

- [ ] **Step 1: Write failing tests:**
```rust
#[test]
fn cmd_pipelines() {
    diff_cmd("a | b");
    diff_cmd("a | b | c");
    diff_cmd("! a");
    diff_cmd("! a | b");
    diff_cmd("echo x | grep y | wc -l");
    diff_cmd("A=1 cmd | other");
    diff_cmd("cmd >o | other");      // redirect on a pipeline stage
}
```

- [ ] **Step 2: Run to verify they fail** — single-stage-only parser doesn't handle `|`/`!`.
- [ ] **Step 3: Implement** `parse_pipeline` (`!` + `|` stages) and route `parse_and_or`'s element through it (mirror `parse_command_then_pipeline`).
- [ ] **Step 4: Run to verify they pass** — PASS; oracle-fix on mismatch. Lexer suite green.
- [ ] **Step 5: Commit**
```bash
git add crates/huck-syntax/src/parser.rs
git commit -m "v242 T5: pipelines (! negate + | stages)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: And-or list — connectors, background, newlines (the full `Sequence`) + final proof

**Files:** Modify `crates/huck-syntax/src/parser.rs`.

**Interfaces produced:** `parse_and_or` builds the full `Sequence{first, rest, background}`.

Mirror `command.rs`'s `parse_sequence_opts` (872): after the first pipeline, loop consuming `Op(Semi)`/`Newline`/`Op(And)`/`Op(Or)`/`Op(Background)`, pushing `(Connector, Command)` pairs; a trailing `&` (nothing meaningful follows) sets `background`; a `&` between groups is `Connector::Amp`. Reproduce the exact newline/`&`/terminator handling (incl. `UnexpectedBackground` for `& &`). Stop (without consuming) at EOF / `Op(RParen)` / case terminators — but `RParen`/case-terminators only arise inside compounds, which are deferred, so at the flat top level the list ends at EOF. Use the `parse` (non-unit) contract: a top-level `Newline` is a Semi-like continue connector (NOT a unit terminator) — match `parse` (793 → `parse_cursor`), not `parse_one_unit`.

- [ ] **Step 1: Write failing tests:**
```rust
#[test]
fn cmd_and_or_lists() {
    diff_cmd("a; b");
    diff_cmd("a; b; c");
    diff_cmd("x && y");
    diff_cmd("x || y");
    diff_cmd("x && y || z");
    diff_cmd("a | b && c | d");
    diff_cmd("p &");                 // trailing background
    diff_cmd("p & q");               // & as separator (Connector::Amp)
    diff_cmd("a\nb");                // newline as connector (parse contract)
    diff_cmd("a; b &");
    diff_cmd("! a | b && c");
}
#[test]
fn cmd_invalid_double_background() {
    // `cmd & &` -> command.rs returns UnexpectedBackground; match it exactly.
    assert_eq!(new_seq("cmd & &"), old_seq("cmd & &"));
}
```

- [ ] **Step 2: Run to verify they fail** — single-pipeline parser ignores connectors.
- [ ] **Step 3: Implement** the connector/background/newline loop in `parse_and_or`, mirroring `parse_sequence_opts`. Confirm whether `parse_sequence` should use the `parse` (newline=connector) or `parse_one_unit` (newline=terminator) contract — match `command::parse` (the oracle the harness uses).
- [ ] **Step 4: Run the full proof:**
  - `cargo test -p huck-syntax --lib cmd_ 2>&1 | grep "test result"` — all PASS.
  - `cargo test --workspace 2>&1 | grep -E "test result|FAILED|warning:" | tail -3` — green, 0 warnings.
  - Release harness sweep (production unaffected — confirm):
    ```bash
    cargo build --release 2>&1 | tail -1
    H="$(pwd)/target/release/huck"; n=0; f=0
    for s in tests/scripts/*_diff_check.sh; do n=$((n+1)); HUCK_BIN="$H" timeout 60 bash "$s" >/dev/null 2>&1 || { echo "FAIL $(basename $s)"; f=$((f+1)); }; done
    echo "harness $n scripts $f fail"
    ```
    Expected: 0 fail (the known `kill_signals` 30s-flake passes at 90s — re-run alone if it trips).
- [ ] **Step 5: Commit**
```bash
git add crates/huck-syntax/src/parser.rs
git commit -m "v242 T6: and-or list (connectors, background, newlines) — full flat command parser

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Notes for the implementer

- The **production `command.rs` parser is the oracle**: when a `diff_cmd` case mismatches, change `parser.rs` to match `old_seq` — never weaken the assertion, never edit `command.rs`.
- Mirror the cited `command.rs` functions for exact behavior (single-vs-`Pipeline` shape, `line` numbers, assignment split, `!`/`&` rules, `RedirFd`/dup `Redirection` construction). The differential corpus enforces every one.
- Do NOT touch `Command` mode, `command.rs`'s parser logic, or any engine crate. If an in-scope case seems to require it, the oracle says otherwise — re-check `command.rs`.
- Keep heredocs/here-strings and every compound/subshell/arith-command deferred to `UnsupportedCommand`; the deferred corpus asserts the boundary.
- Line numbers are approximate (`command.rs` is ~6000 lines) — locate by symbol name.
