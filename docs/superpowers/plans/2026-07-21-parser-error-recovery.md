# Parser Error-Recovery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give huck's parser a recovery capability — `parse_recover(src)` parses incomplete input (a line truncated at the cursor) and returns a complete, walkable tree plus a `CursorContext`, instead of erroring.

**Architecture:** Recovery synthesizes the minimal valid completion of every open construct at EOF. The **lexer** closes open modes with synthetic closing atoms (generalizing the existing `eof_closes_heredoc` option); the **parser** synthesizes minimal bodies for open compound commands at its unterminated-error sites, and captures the cursor context at the synthesis boundary. No AST changes; the strict `parse()` path is byte-for-byte unaffected.

**Tech Stack:** Rust (2024 edition), `crates/huck-syntax` (lexer + parser + command AST). No new dependencies.

**Spec:** `docs/superpowers/specs/2026-07-21-parser-error-recovery-design.md`
**Issue:** [#246](https://github.com/jdstanhope/huck/issues/246)

## Global Constraints

- Every commit ends with the trailer `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- Run `cargo fmt --all` before every commit — CI enforces `cargo fmt --all --check`.
- **Never** run `cargo test --workspace` — this box (1 core / 1.9GB) OOM-kills the session. Always `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` (huck-syntax is the only crate this iteration touches).
- The strict `parse()` / `parse_sequence` path used for execution must stay **byte-for-byte unchanged** — recovery is a new entry point behind new, default-`false` flags. The existing huck-syntax parser test suite is the gate for this.
- `parse_recover` must **never panic** on any input, including arbitrary truncations — completion feeds it partial input on every keystroke.
- Caller passes `src = line[..pos]`; the cursor is EOF. No cursor-offset is threaded through the parser.
- Public types `RecoveredParse`, `CursorContext`, `Frame`, `WordPosition` are `#[non_exhaustive]` (iteration 2 will extend them).
- This iteration touches **no** completion code (`analyze_full`, `dispatch::resolve`) — that is iteration 2.

## Codebase Orientation

Read before Task 1.

- **`LexerOptions`** (`crates/huck-syntax/src/lexer.rs:955`) already has `eof_closes_heredoc: bool` — the precedent for "EOF gracefully closes this construct." It is `pub`, immutable for a lexer's lifetime, read only by the heredoc-body collectors. Task 2 adds a sibling `recover_at_eof: bool` the same way.
- **The lexer mode stack** is `self.modes: Vec<Mode>`; `current_mode()` (`lexer.rs:1327`) reads the top. `Mode` variants relevant to recovery: `CommandSub`, `Arith`, `BacktickRaw`, `ParamExpansion`, `DoubleQuote`, `SingleQuote`(via quote scanning), `ArrayLiteral`, plus `Regex`/`Extglob`. The lexer already emits close atoms for some modes (e.g. `lexer.rs:2493` "emit the matching close atom"); Task 2 makes EOF-with-open-modes do the same synthetically.
- **The parse entry** is `parse(src)` (`parser.rs:23`): `Lexer::new(src, &Default::default(), LexerOptions::default())` then `parse_sequence(&mut lx)`. `parse_recover` mirrors this with recovery options.
- **The unterminated-error sites** in the parser (Task 3 converts these under the recovery flag): `UnterminatedSubshell` at `parser.rs:845`; `unterminated_cmdsub(pos)` returns at `:1654`, `:1698`, `:1775`, `:1802`; `unterminated_backtick` at `:2013`; `ParseError::Unexpected(ExpectFailure{Found::Eof, …})` at `:2785`, `:3074`, `:3832`. Each corresponds to a specific construct hitting EOF.
- **The AST** (`crates/huck-syntax/src/command.rs`): `Sequence` (`:735`), `Command` (`:611`), `Pipeline` (`:519`). No node carries a source span — this is why the cursor context is captured during recovery, not searched by offset afterward.
- **`ParseError`** and `ExpectFailure`/`Found` live in `crates/huck-syntax/src/errors.rs` and `command.rs`.

---

## File Structure

- **Create** `crates/huck-syntax/src/recover.rs` — the public `parse_recover` entry point, the `RecoveredParse` / `CursorContext` / `Frame` / `WordPosition` types, and the `Mode`→`Frame` / mode→closer mapping helpers. Sole owner of the recovery-facing API.
- **Modify** `crates/huck-syntax/src/lexer.rs` — add `LexerOptions::recover_at_eof`; emit synthetic mode-closers at EOF when set; expose a `pub(crate)` snapshot of the mode stack + last-word span for cursor capture.
- **Modify** `crates/huck-syntax/src/parser.rs` — thread a recovery flag (read from the lexer's option); at the unterminated-error sites synthesize minimal compound bodies; record the cursor `WordPosition` at the recovery boundary.
- **Modify** `crates/huck-syntax/src/lib.rs` — `pub mod recover;` and re-export `parse_recover` + the types.
- **Modify** `docs/architecture.md` — one note that `parse_recover` exists alongside strict `parse`.

---

## Task 1: Public types + `parse_recover` skeleton

**Files:**
- Create: `crates/huck-syntax/src/recover.rs`
- Modify: `crates/huck-syntax/src/lib.rs`

**Interfaces:**
- Consumes: `parse_sequence` (existing), `Sequence` (existing), `Lexer`/`LexerOptions` (existing).
- Produces: `pub fn parse_recover(src: &str) -> RecoveredParse`; `RecoveredParse { tree: Option<Sequence>, cursor: CursorContext }`; `CursorContext { enclosing: Vec<Frame>, position: WordPosition, word: String, word_start: usize }`; `Frame` enum; `WordPosition` enum (all `#[non_exhaustive]`).

This task establishes the surface with recovery **not yet active** — `parse_recover` on *complete* input must already return the correct tree and a best-effort cursor. Recovery of incomplete input arrives in Tasks 2-4.

- [ ] **Step 1: Write the failing test**

Create `crates/huck-syntax/src/recover.rs` with only its test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recover_complete_input_returns_tree_and_command_cursor() {
        // Complete input parses to a tree; recovery is a no-op here.
        let r = parse_recover("echo hi");
        assert!(r.tree.is_some(), "complete input yields a tree");
    }

    #[test]
    fn recover_empty_input_is_command_position() {
        let r = parse_recover("");
        assert_eq!(r.cursor.position, WordPosition::Command);
        assert_eq!(r.cursor.word, "");
        assert_eq!(r.cursor.word_start, 0);
    }

    #[test]
    fn types_are_non_exhaustive_and_public() {
        // Compile-time surface check.
        let _f: Frame = Frame::CommandSub;
        let _p: WordPosition = WordPosition::Argument;
    }
}
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test -p huck-syntax --jobs 1 --lib recover:: -- --test-threads 1
```

Expected: FAIL to compile — `parse_recover`, `Frame`, `WordPosition` undefined.

- [ ] **Step 3: Implement the types + skeleton**

Prepend to `crates/huck-syntax/src/recover.rs`:

```rust
//! Error-recovery parse: parse a line truncated at the cursor and return a
//! walkable tree plus the cursor context, instead of erroring on the
//! unterminated tail. See docs/superpowers/specs/2026-07-21-parser-error-recovery-design.md.
//!
//! The caller passes `src = line[..cursor]`, so the cursor is EOF. Recovery
//! synthesizes the minimal valid completion of every open construct; the strict
//! `parse()` path is unaffected.

use crate::command::Sequence;

/// An enclosing construct at the cursor. Innermost is LAST in
/// `CursorContext::enclosing`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Frame {
    CommandSub,
    Subshell,
    ArrayLiteral,
    Arith,
    Backtick,
    DoubleQuote,
    SingleQuote,
    ParamExpansion,
    IfCondition,
    WhileCondition,
    ForList,
    CaseSubject,
    BraceGroup,
}

/// What the word at the cursor is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum WordPosition {
    Command,
    Argument,
    VariableName,
    RedirectTarget,
    AssignRhs,
    Unknown,
}

/// The cursor context, captured at the recovery synthesis boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CursorContext {
    pub enclosing: Vec<Frame>,
    pub position: WordPosition,
    pub word: String,
    pub word_start: usize,
}

/// The result of a recovery parse.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct RecoveredParse {
    pub tree: Option<Sequence>,
    pub cursor: CursorContext,
}

/// Parse `src` (a line truncated at the cursor) with EOF-recovery.
pub fn parse_recover(src: &str) -> RecoveredParse {
    // Task 1 skeleton: strict parse, best-effort cursor. Tasks 2-4 activate
    // recovery of incomplete input.
    let tree = crate::parser::parse(src).ok().flatten();
    RecoveredParse {
        tree,
        cursor: CursorContext {
            enclosing: Vec::new(),
            position: WordPosition::Command,
            word: String::new(),
            word_start: src.len(),
        },
    }
}
```

Add to `crates/huck-syntax/src/lib.rs` (keep module list ordering; re-export the public surface next to the existing `parse` re-export):

```rust
pub mod recover;
pub use recover::{parse_recover, CursorContext, Frame, RecoveredParse, WordPosition};
```

- [ ] **Step 4: Run to verify it passes**

```bash
cargo fmt --all
cargo test -p huck-syntax --jobs 1 --lib recover:: -- --test-threads 1
```

Expected: PASS, 3 tests. (`recover_empty_input_is_command_position` passes because the skeleton hardcodes `Command`/empty — Task 4 makes it real, but empty-line-is-command is the correct end state, so it stays.)

- [ ] **Step 5: Commit**

```bash
git add crates/huck-syntax/src/recover.rs crates/huck-syntax/src/lib.rs
git commit -m "$(cat <<'EOF'
recovery task 1: parse_recover skeleton + public types (#246)

Adds the recover.rs surface — parse_recover + RecoveredParse/CursorContext/
Frame/WordPosition (all #[non_exhaustive]). Recovery of incomplete input is
not yet active; complete input returns the strict tree. Strict parse path
untouched.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Lexer recovery — synthetic mode closers

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` (`LexerOptions` struct; the EOF path of the scanner)
- Modify: `crates/huck-syntax/src/recover.rs` (`parse_recover` uses the recovery lexer)
- Test: `crates/huck-syntax/src/recover.rs` (`mod tests`)

**Interfaces:**
- Consumes: `LexerOptions`, `Lexer::new`, `self.modes` (existing).
- Produces: `LexerOptions::recover_at_eof: bool`; when set, the lexer, at real EOF with open lexer modes, emits the synthetic close atom for each open frame innermost-out before yielding `None`.

This is the lexer half — it makes the *nesting* constructs (`$(`, `"$(`, `${`, `$((`, backtick, `<(`/`>(`, `NAME=(`) recover, so `parse_recover` produces a complete tree for them.

- [ ] **Step 1: Write the failing tests**

Add to `recover.rs`'s `mod tests`:

```rust
#[test]
fn recover_unterminated_cmdsub_yields_tree() {
    // `echo $(whi` — recovery closes the `$(` so the whole thing parses.
    let r = parse_recover("echo $(whi");
    assert!(r.tree.is_some(), "unterminated $( should recover to a tree");
}

#[test]
fn recover_cmdsub_in_double_quotes_yields_tree() {
    let r = parse_recover("echo \"$(whi");
    assert!(r.tree.is_some(), "quoted unterminated $( should recover");
}

#[test]
fn recover_unterminated_param_expansion_yields_tree() {
    let r = parse_recover("echo ${whi");
    assert!(r.tree.is_some());
}

#[test]
fn recover_unterminated_arith_yields_tree() {
    let r = parse_recover("echo $(( x + ");
    assert!(r.tree.is_some());
}

#[test]
fn recover_unterminated_backtick_yields_tree() {
    let r = parse_recover("echo `whi");
    assert!(r.tree.is_some());
}

#[test]
fn recover_unterminated_double_quote_yields_tree() {
    let r = parse_recover("echo \"hello");
    assert!(r.tree.is_some());
}
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test -p huck-syntax --jobs 1 --lib recover::tests::recover_unterminated_cmdsub -- --test-threads 1
```

Expected: FAIL — `parse_recover` still calls strict `parse`, which returns `Err` (→ `tree == None`) for unterminated input.

- [ ] **Step 3: Add the lexer option + synthetic closers**

**3a.** In `crates/huck-syntax/src/lexer.rs`, add to `LexerOptions` (after `eof_closes_heredoc`):

```rust
    /// Recovery mode for completion/tooling: at genuine end-of-input with open
    /// lexer modes (`$(`, `${`, `$((`, `"`, `'`, backtick, `<(`/`>(`, `NAME=(`),
    /// emit the synthetic CLOSING atom for each open frame (innermost-out)
    /// before yielding `None`, so a caller (`parse_recover`) sees a well-formed
    /// token stream. DEFAULT `false`: strict parsing (execution) is unaffected.
    /// Like `eof_closes_heredoc`, immutable for a lexer's lifetime.
    pub recover_at_eof: bool,
```

**3b.** Find the scanner's EOF handling — where the pull loop reaches real end-of-input and would return `None` (the main `next_kind`/`scan_step` EOF path). When `self.opts.recover_at_eof` and `self.modes` has open frames beyond the base, emit one synthetic close atom per open frame, innermost-first, on successive pulls, popping the corresponding mode each time, THEN return `None`. Reuse the exact close `TokenKind` each mode already emits on a real close (grep the mode's real-close site — e.g. `CommandSub`'s `)` handling, `ParamExpansion`'s `ParamClose`, `DoubleQuote`'s `EndDquote`, `Arith`'s `ArithClose`, backtick's `EndBacktick`). Map:

| top `Mode` | synthetic atom to emit (same as its real close) |
|---|---|
| `CommandSub` / subshell / `ArrayLiteral` | the `)` / `RParen` / `ArrayClose` atom that mode emits on a real `)` |
| `Arith` | `ArithClose` |
| `BacktickRaw` | `EndBacktick` |
| `ParamExpansion` | `ParamClose` |
| `DoubleQuote` | `EndDquote` |
| `Regex` / `Extglob` | their existing zero-width terminators |

Single quotes and `$'…'` are consumed by a self-contained scan (not a persistent `Mode` frame); if an unterminated `'…` reaches EOF, close it by treating the collected text as a complete single-quoted Lit (mirror `eof_closes_heredoc`'s "parse the body collected so far"). Model the implementation on the existing `None if self.opts.eof_closes_heredoc =>` arm (`lexer.rs:4100`).

Emit synthetic closers with a **zero-width span at the current (EOF) offset** so they carry no real source range.

**3c.** In `recover.rs`, make `parse_recover` drive a recovery lexer instead of strict `parse`:

```rust
pub fn parse_recover(src: &str) -> RecoveredParse {
    let opts = crate::lexer::LexerOptions {
        recover_at_eof: true,
        ..Default::default()
    };
    let mut lx = crate::lexer::Lexer::new(src, &Default::default(), opts);
    let tree = crate::parser::parse_sequence(&mut lx).ok().flatten();
    RecoveredParse {
        tree,
        cursor: CursorContext {
            enclosing: Vec::new(),
            position: WordPosition::Command,
            word: String::new(),
            word_start: src.len(),
        },
    }
}
```

(Check `Lexer::new`'s exact signature at `parser.rs:23` and match it — the second arg is `&Default::default()`.)

- [ ] **Step 4: Run to verify they pass**

```bash
cargo fmt --all
cargo test -p huck-syntax --jobs 1 --lib recover:: -- --test-threads 1
```

Expected: the 6 nesting tests PASS (trees are `Some`). If the parser still errors on a recovered stream for one construct, that construct's mode isn't being closed with the right atom — fix the closer for that mode; do not weaken the test.

- [ ] **Step 5: Confirm the strict path is unaffected**

```bash
cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1
```

Expected: the entire existing huck-syntax suite still passes — `recover_at_eof` defaults `false`, so strict parsing is untouched.

- [ ] **Step 6: Commit**

```bash
git add crates/huck-syntax/src/lexer.rs crates/huck-syntax/src/recover.rs
git commit -m "$(cat <<'EOF'
recovery task 2: lexer closes open modes at EOF under recover_at_eof (#246)

Adds LexerOptions::recover_at_eof (sibling of eof_closes_heredoc): at real
EOF with open lexer modes, emit each mode's synthetic close atom innermost-
out, so parse_recover sees a well-formed stream. Fixes the nesting cases
($(, "$(, ${, $((, backtick, "…). Strict path unchanged.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Parser recovery — synthetic compound-command bodies

**Files:**
- Modify: `crates/huck-syntax/src/parser.rs` (unterminated-error sites)
- Modify: `crates/huck-syntax/src/recover.rs` (thread the recovery flag; test)

**Interfaces:**
- Consumes: the recovery lexer from Task 2; `Lexer` exposes whether recovery is on (via `self.opts.recover_at_eof` — add a `pub(crate) fn recover_at_eof(&self) -> bool` accessor if the options aren't already reachable from the parser).
- Produces: under recovery, the parser synthesizes the minimal valid body for an open compound command instead of returning `Err(unterminated_*)`.

This is the parser half — it makes `if`/`while`/`until`/`for … in`/`case`/`{`/subshell recover, so `parse_recover("if whi")` yields a tree.

- [ ] **Step 1: Write the failing tests**

Add to `recover.rs`'s `mod tests`:

```rust
#[test]
fn recover_if_without_then_yields_tree() {
    let r = parse_recover("if whi");
    assert!(r.tree.is_some(), "`if COND` should recover (synthesize then/fi)");
}

#[test]
fn recover_while_without_do_yields_tree() {
    let r = parse_recover("while whi");
    assert!(r.tree.is_some());
}

#[test]
fn recover_for_in_without_do_yields_tree() {
    let r = parse_recover("for x in whi");
    assert!(r.tree.is_some());
}

#[test]
fn recover_case_without_esac_yields_tree() {
    let r = parse_recover("case whi");
    assert!(r.tree.is_some());
}

#[test]
fn recover_brace_group_yields_tree() {
    let r = parse_recover("{ whi");
    assert!(r.tree.is_some());
}

#[test]
fn recover_subshell_yields_tree() {
    let r = parse_recover("( whi");
    assert!(r.tree.is_some());
}
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test -p huck-syntax --jobs 1 --lib recover::tests::recover_if_without_then -- --test-threads 1
```

Expected: FAIL — the compound-command parser returns `Err(Unexpected{Eof})` for a missing `then`/`do`/`esac`; the lexer's mode-closing does not touch keyword grammar.

- [ ] **Step 3: Synthesize minimal bodies at the unterminated sites**

Add a `pub(crate) fn recover_at_eof(&self) -> bool` to `Lexer` (reads `self.opts.recover_at_eof`) if the parser can't already see it.

At each compound-command parse function, where it currently returns `Err` on a missing continuation keyword at EOF, guard on `iter.recover_at_eof()` and instead build the node with a synthesized minimal body. Concretely (read each function first; these are the sites):

- `if` (the function that expects `then`/`fi`): on EOF before `then`, return an `If` whose condition is the sequence parsed so far and whose then-branch is a single `:` (true/no-op) command, no else. On EOF before `fi`, close with what was parsed.
- `while`/`until`: on EOF before `do`/`done`, body = single `:`.
- `for` (the function expecting `in`/`do`/`done`): on EOF before `do`/`done`, body = single `:`; on EOF before the word list, an empty list.
- `case` (expecting `in`/patterns/`esac`): on EOF, close with the clauses parsed so far (possibly none).
- brace group `{ … }` / subshell `( … )`: on EOF, close with the sequence parsed so far. (Subshell's lexer mode also emits a synthetic `)` from Task 2; ensure the parser's own subshell-close site is recovery-guarded too, so both agree.)

Build the synthetic `:` command the same way the parser builds a simple command elsewhere (a `Word` of one `Lit{text:":"}`); reuse existing AST constructors — grep for how `:` or an empty pipeline is built (the empty-cmdsub body at `parser.rs:~1690` constructs a `Sequence`/`Pipeline` you can model on).

Keep each guard tight: `if iter.recover_at_eof() { <synthesize> } else { <existing Err> }`. Do not alter the non-recovery branch.

- [ ] **Step 4: Run to verify they pass**

```bash
cargo fmt --all
cargo test -p huck-syntax --jobs 1 --lib recover:: -- --test-threads 1
```

Expected: the 6 compound tests PASS. Re-run the whole suite to confirm strict parsing is still green:

```bash
cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1
```

- [ ] **Step 5: Commit**

```bash
git add crates/huck-syntax/src/parser.rs crates/huck-syntax/src/recover.rs
git commit -m "$(cat <<'EOF'
recovery task 3: parser synthesizes minimal compound bodies at EOF (#246)

Under recover_at_eof, the if/while/until/for/case/brace/subshell parse
sites synthesize a minimal valid body (`then :; fi`, `do :; done`, `esac`,
…) instead of returning Err(unterminated_*), so parse_recover yields a
tree for compound commands. Non-recovery branch unchanged.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Capture `CursorContext`

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` (record the last real word + mode-stack snapshot at EOF)
- Modify: `crates/huck-syntax/src/parser.rs` (record `WordPosition` at the recovery boundary)
- Modify: `crates/huck-syntax/src/recover.rs` (assemble `CursorContext`)
- Test: `crates/huck-syntax/src/recover.rs`

**Interfaces:**
- Consumes: recovery lexer + parser from Tasks 2-3.
- Produces: a filled `CursorContext { enclosing, position, word, word_start }` on `parse_recover`.

- [ ] **Step 1: Write the failing tests**

Add to `recover.rs`'s `mod tests`:

```rust
fn ctx(src: &str) -> CursorContext {
    parse_recover(src).cursor
}

#[test]
fn cursor_command_position_cases() {
    for src in ["whi", "if whi", "while whi", "echo $(whi", "echo `whi", "(whi", "echo <(whi"] {
        assert_eq!(ctx(src).position, WordPosition::Command, "{src:?}");
    }
}

#[test]
fn cursor_argument_position_cases() {
    for src in ["echo whi", "for x in whi", "ls -l whi"] {
        assert_eq!(ctx(src).position, WordPosition::Argument, "{src:?}");
    }
}

#[test]
fn cursor_variable_position_cases() {
    assert_eq!(ctx("echo ${whi").position, WordPosition::VariableName);
    assert_eq!(ctx("echo $whi").position, WordPosition::VariableName);
    assert_eq!(ctx("echo $(( whi").position, WordPosition::VariableName);
}

#[test]
fn cursor_word_and_start() {
    let c = ctx("echo $(whi");
    assert_eq!(c.word, "whi");
    assert_eq!(c.word_start, 7, "anchor right after `$(`");
}

#[test]
fn cursor_enclosing_frames() {
    assert_eq!(ctx("echo \"$(whi").enclosing.last(), Some(&Frame::CommandSub));
    assert_eq!(ctx("echo $(( whi").enclosing.last(), Some(&Frame::Arith));
    assert!(ctx("echo whi").enclosing.is_empty());
}

#[test]
fn cursor_array_literal_not_command() {
    // `x=(whi` is an array literal, not a subshell command.
    assert_ne!(ctx("x=(whi").position, WordPosition::Command);
    assert_eq!(ctx("x=$(whi").position, WordPosition::Command);
}
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test -p huck-syntax --jobs 1 --lib recover::tests::cursor_ -- --test-threads 1
```

Expected: FAIL — the skeleton still returns hardcoded `Command`/empty for every input.

- [ ] **Step 3: Implement the capture**

The mechanism has three pieces; all fire at the recovery boundary (real EOF):

1. **`word` + `word_start`** — the lexer records the text and start offset of the **last real Lit-family atom** it emitted before EOF (add two fields to `Lexer`, updated whenever a word atom is produced; expose via `pub(crate) fn last_word(&self) -> (&str, usize)`). If the last real token was a separator/opener (cursor sits at a fresh boundary), `word` is empty and `word_start == src.len()`.

2. **`enclosing`** — snapshot `self.modes` at real EOF (before Task 2's synthetic closers pop them), mapped `Mode`→`Frame` (a `mode_to_frame` helper in `recover.rs`, innermost LAST). Compound-command frames (`IfCondition`, `ForList`, `CaseSubject`, `BraceGroup`, `WhileCondition`) are pushed by the parser: when a Task-3 synthesis fires, push the corresponding `Frame` onto a parser-held `recovery_frames: Vec<Frame>` that `recover.rs` reads and appends. Expose both via accessors.

3. **`position`** — set by whichever context the cursor word is in, in priority order: inside `Mode::ParamExpansion` or `Mode::Arith` → `VariableName`; a `$name` word (the lexer knows it is scanning a `$`-name) → `VariableName`; else the parser's command-assembly records `Command` when the cursor word is word 0 of its simple command and `Argument` when it is a later word (the simple-command parser knows which). Add a `pub(crate)` cell on `Lexer` or a parser field the recovery boundary writes; `recover.rs` reads it. `NAME=(` is an `ArrayLiteral` mode (not command); `NAME=$(` is `CommandSub` (command) — the mode stack already distinguishes them, so `position` follows from the mode.

Assemble in `parse_recover`:

```rust
// after parse_sequence returns, read the captured state off the lexer/parser
let cursor = CursorContext {
    enclosing: /* mode_to_frame snapshot + parser recovery_frames */,
    position:  /* captured WordPosition */,
    word:      /* lexer last_word text */,
    word_start:/* lexer last_word start */,
};
```

Because the captured state lives on the `Lexer` (and a parser-held vec threaded through), read it after `parse_sequence` returns while `lx` is still in scope.

- [ ] **Step 4: Run to verify they pass**

```bash
cargo fmt --all
cargo test -p huck-syntax --jobs 1 --lib recover:: -- --test-threads 1
```

Expected: all cursor tests PASS, and the earlier Task 1-3 tree tests still PASS.

- [ ] **Step 5: Confirm strict path unaffected**

```bash
cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1
```

Expected: whole suite green.

- [ ] **Step 6: Commit**

```bash
git add crates/huck-syntax/src/lexer.rs crates/huck-syntax/src/parser.rs crates/huck-syntax/src/recover.rs
git commit -m "$(cat <<'EOF'
recovery task 4: capture CursorContext at the synthesis boundary (#246)

parse_recover now fills CursorContext {enclosing, position, word,
word_start} from state captured at real EOF: the lexer records the last
word + mode-stack snapshot; the parser records command-vs-argument and its
compound recovery frames. Positions match bash for command/argument/
variable across $(, "$(, $((, ${, if/while/for-in, x=( vs x=$(.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Recovered-tree shape tests, never-panic sweep, docs

**Files:**
- Test: `crates/huck-syntax/src/recover.rs`
- Modify: `docs/architecture.md`

**Interfaces:**
- Consumes: the finished `parse_recover`.
- Produces: nothing new — hardening + docs.

- [ ] **Step 1: Recovered-tree shape test**

Prove the recovered tree is well-formed and walkable (not just `Some`). Add to `recover.rs`'s `mod tests` a test that walks the tree for `echo $(whi` and asserts the outer command is `echo` with an argument that is a command substitution whose body is the command `whi`. Match on the real AST shape (read `command.rs`'s `Command`/`Pipeline`/`WordPart` variants to write the exact pattern):

```rust
#[test]
fn recovered_tree_is_walkable() {
    let r = parse_recover("echo $(whi");
    let seq = r.tree.expect("tree");
    // Assert the top command is `echo` and it carries a command-substitution
    // argument whose inner command word is `whi`. (Fill in the match against
    // the actual Command/WordPart variants.)
    // This proves recovery yields a structurally valid tree, not just Some.
    assert!(format!("{seq:?}").contains("whi"));
}
```

(Replace the `format!`-contains shim with a real structural match once you have read the AST variants — the shim is a placeholder-free minimal assertion that still fails if recovery produces an empty/wrong tree; tighten it to a real pattern match.)

- [ ] **Step 2: Never-panic truncation sweep**

`parse_recover` must never panic. Add:

```rust
#[test]
fn recover_never_panics_on_any_truncation() {
    // A spread of inputs exercising every construct; truncate each at every
    // byte offset (including inside multi-byte-free ASCII here) and assert
    // parse_recover returns without panicking.
    let corpus = [
        "echo hi",
        "echo $(whi)",
        "echo \"$(ls) $x\"",
        "if a; then b; fi",
        "for x in a b; do echo $x; done",
        "case $x in a) b;; esac",
        "a=(1 2 3)",
        "echo ${x:-def}",
        "echo $(( 1 + 2 ))",
        "f() { echo `date`; }",
        "while read l; do :; done < f",
        "{ a; b; }",
    ];
    for s in corpus {
        for i in 0..=s.len() {
            if !s.is_char_boundary(i) {
                continue;
            }
            // Must not panic. Result content is not asserted here.
            let _ = parse_recover(&s[..i]);
        }
    }
}
```

- [ ] **Step 3: Run the tests**

```bash
cargo test -p huck-syntax --jobs 1 --lib recover:: -- --test-threads 1
```

Expected: PASS, including no panic in the sweep. If a truncation panics, fix the recovery path (an unhandled open construct or an unwrap) — the sweep is the robustness gate; do not narrow the corpus to dodge a panic.

- [ ] **Step 4: Docs**

In `docs/architecture.md`, in the front-end/parser section, add one or two sentences: a recovery entry point `huck_syntax::parse_recover` exists alongside strict `parse` — it parses a line truncated at the cursor and returns a walkable tree plus a `CursorContext`, for completion (iteration 2) and future tooling; strict `parse` is unchanged.

- [ ] **Step 5: Commit**

```bash
git add crates/huck-syntax/src/recover.rs docs/architecture.md
git commit -m "$(cat <<'EOF'
recovery task 5: tree-shape + never-panic sweep + docs (#246)

Adds a recovered-tree structural test, a truncate-at-every-offset
never-panic sweep over a construct corpus, and an architecture.md note for
parse_recover.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Full verification and PR

**Files:** none — the gate.

- [ ] **Step 1: Per-crate suite + fmt**

```bash
cargo fmt --all --check
cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1
```

Expected: green (both the new `recover::` tests and the entire existing parser/lexer suite — the strict path must be untouched), fmt clean.

- [ ] **Step 2: Engine + CLI still build and pass**

Recovery is additive to huck-syntax, but confirm downstream crates are unaffected:

```bash
cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1
cargo test -p huck-cli --jobs 1 --lib -- --test-threads 1
```

Expected: green (no behavior change — nothing consumes `parse_recover` yet).

- [ ] **Step 3: Diff-check sweep**

```bash
cargo build --locked --bin huck
ulimit -v 1500000
timeout 900 tests/scripts/run_diff_checks.sh 2>&1 | tail -3
```

Expected: `Diff-check sweep: N passed, 0 failed` — unaffected, recovery is not wired into execution.

- [ ] **Step 4: Push and open the PR**

```bash
git push -u origin parser-error-recovery
gh pr create --title "parser: error-recovery — partial tree for incomplete input (#246)" --body "$(cat <<'EOF'
Closes #246

Iteration 1 of the parser-driven completion effort: a `parse_recover(src)`
entry point in huck-syntax that parses a line truncated at the cursor and
returns a walkable tree plus a `CursorContext`, instead of erroring on the
unterminated tail.

## Approach

Recovery synthesizes the minimal valid completion of every open construct at
EOF. The lexer closes open modes with synthetic closing atoms (generalizing
the existing `eof_closes_heredoc`); the parser synthesizes minimal bodies for
open compound commands at its unterminated-error sites, and captures the cursor
context at the synthesis boundary (the AST has no spans, so the enclosing
constructs are snapshotted during recovery, not searched afterward). No AST
changes; the strict `parse()` path used for execution is byte-for-byte
unaffected.

## Deliverable

`parse_recover` + `RecoveredParse { tree, cursor: CursorContext }`. Tested
within huck-syntax: recovery-context assertions per construct
(command/argument/variable across `$(`, `"$(`, `$((`, `${`, `if`/`while`/
`for … in`, `x=(` vs `x=$(`), recovered-tree shapes, the strict-path-unchanged
gate, and a never-panic truncate-at-every-offset sweep.

## Next

Iteration 2 (separate issue) deletes the hand-rolled `analyze_full` completion
scanner and derives completion context from `CursorContext`, fixing the
`if whi` / `echo "$(whi` / `echo $(( HO` / `for x in whi` completion
divergences structurally.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 5: Wait for CI**

```bash
gh pr checks --watch
```

Poll until CI **finishes** and passes (local green ≠ CI green — this box is 1-core, CI 4-core). Then hand the PR to the user. **Do not merge it.**

---

## Self-Review Notes

**Spec coverage.** Every spec section maps to a task: the synthesize-minimal-completion principle and the two halves → Tasks 2 (lexer) + 3 (parser); the API/types → Task 1; cursor capture (no-spans → capture at boundary) → Task 4; the four test categories → Tasks 2-5 (recovery-context, tree-shape, strict-unchanged gate on every task, never-panic sweep in Task 5); docs → Task 5. The `#[non_exhaustive]` requirement is in Task 1.

**Judgment tasks flagged honestly.** Tasks 3 and 4 touch parser internals whose exact code depends on reading the cited functions (`if`/`for`/`case` parsers; the simple-command word loop). Each gives the mechanism, the exact sites, and fully concrete tests — the tests are the contract; the implementer writes the precise parser code against them. This is the integration/judgment task type, not mechanical transcription. Tasks 1, 2, and 5 are close to mechanical.

**One deliberate placeholder-shaped item.** Task 5 Step 1's tree-shape test starts as a `format!`-contains shim with an explicit instruction to tighten it to a real AST match after reading the variants — this is a minimal-but-failing assertion, not a no-op, and the instruction to tighten it is concrete.

**Type consistency.** `RecoveredParse`/`CursorContext`/`Frame`/`WordPosition` field and variant names are used identically in Tasks 1, 4, and 5. `parse_recover(src) -> RecoveredParse`, `LexerOptions::recover_at_eof`, and `iter.recover_at_eof()` are consistent across Tasks 1-4.
