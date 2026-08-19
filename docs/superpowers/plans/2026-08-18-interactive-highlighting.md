# Interactive Syntax Highlighting Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Colour the interactive command line as it is typed — invalid commands, quotes, expansions, globs, escapes, brackets — without changing what parses or runs.

**Architecture:** A parse-driven recorder. `parse_sequence` drives the lexer; behind a `LexerOptions` flag the lexer appends `(span, role)` records at ONE hook, `scan_step_guarded`, plus `(open, close)` records when a pair frame pops. The CLI crate maps roles to SGR. Anything the highlighter would otherwise have to re-derive is instead PRODUCED BY the lexer — no second copy of any rule.

**Tech Stack:** Rust, rustyline 18 (`Highlighter` trait), expectrl (pty tests).

Spec: [`2026-08-18-interactive-highlighting-design.md`](../specs/2026-08-18-interactive-highlighting-design.md) · Issue: [#666](https://github.com/jdstanhope/huck/issues/666)

## Global Constraints

- **Inertness is the master gate.** With `record_highlight: false` (the default) behaviour must be bit-identical. EVERY task runs: `tools/parse_sweep.sh` (3103 scripts, identical to baseline), `tools/param_corpus.sh` (282 rows, identical), both `--lib` suites, all `-p huck` integration binaries, and `tests/scripts/run_diff_checks.sh` (309+, both binaries).
- **Doctests run in CI but not under `cargo test -p X --lib`.** Any indented block in a doc comment is compiled as Rust — fence prose examples as ```text. Run `cargo test --workspace --doc` before pushing.
- **Clippy is pinned**: `cargo +1.97.1 clippy --workspace --all-targets --locked -- -D warnings`. Lint with that version, not the default stable.
- **No parallel structures** (#641). The recorder holds ONE record type; roles are derived where the information already lives, never recomputed downstream.
- **Aliases are passed EMPTY to the highlight parse.** Read-time alias expansion splices tokens whose spans point into the alias body.
- **`Highlighter::highlight` must preserve DISPLAY WIDTH** — SGR only.
- Commit trailer: `Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>`.
- `cargo fmt --all` before every commit.

## File Structure

| file | responsibility |
| --- | --- |
| `crates/huck-syntax/src/highlight.rs` (new) | `Role`, `Mark`, `HighlightRecord` — the vocabulary and the container. No colour. |
| `crates/huck-syntax/src/lexer.rs` | the `record_highlight` flag, the recorder hook in `scan_step_guarded`, sub-token marks, pair records |
| `crates/huck-syntax/src/lib.rs` | `pub mod highlight;` |
| `crates/huck-cli/src/paint.rs` (new) | role -> SGR table, span painting, `NO_COLOR`/tty gating |
| `crates/huck-cli/src/completion_helper.rs` | `impl Highlighter for HuckHelper` |
| `crates/huck-engine/src/cmd_validity.rs` (new) | positive+negative command-validity cache with the slow-fs guard |
| `tests/highlight_render_pty.rs` (new) | the rendered-output harness |
| `crates/huck-syntax/tests/highlight_spans.rs` (new) | pure `(text) -> Vec<Mark>` unit tests |

---

### Task 1: The record vocabulary and an inert recorder

**Files:**
- Create: `crates/huck-syntax/src/highlight.rs`
- Modify: `crates/huck-syntax/src/lib.rs`, `crates/huck-syntax/src/lexer.rs`
- Test: `crates/huck-syntax/tests/highlight_spans.rs`

**Interfaces:**
- Produces: `huck_syntax::highlight::{Role, Mark, HighlightRecord}`; `LexerOptions::record_highlight`; `Lexer::take_highlight_record() -> HighlightRecord`.

- [ ] **Step 1: Write the failing test**

```rust
// crates/huck-syntax/tests/highlight_spans.rs
use huck_syntax::highlight::Role;
use huck_syntax::lexer::{Lexer, LexerOptions};

fn marks(src: &str) -> Vec<(usize, Role)> {
    let empty = std::collections::HashMap::new();
    let opts = LexerOptions { record_highlight: true, ..Default::default() };
    let mut lx = Lexer::new(src, &empty, opts);
    let _ = huck_syntax::parser::parse_sequence(&mut lx);
    lx.take_highlight_record().marks.into_iter().map(|m| (m.start, m.role)).collect()
}

#[test]
fn records_quote_styles_distinctly() {
    let m = marks("echo 'sq' \"dq\"");
    assert!(m.iter().any(|(s, r)| *s == 5 && *r == Role::QuotedSingle));
    assert!(m.iter().any(|(s, r)| *s == 10 && *r == Role::QuotedDouble));
}

#[test]
fn records_expansions() {
    let m = marks("echo $HOME ${x:-d} $(date)");
    assert!(m.iter().any(|(_, r)| *r == Role::VarName));
    assert!(m.iter().any(|(_, r)| *r == Role::Expansion));
}

#[test]
fn off_by_default_records_nothing() {
    let empty = std::collections::HashMap::new();
    let mut lx = Lexer::new("echo 'x' $HOME", &empty, LexerOptions::default());
    let _ = huck_syntax::parser::parse_sequence(&mut lx);
    assert!(lx.take_highlight_record().marks.is_empty());
}
```

- [ ] **Step 2: Run it and watch it fail** — `cargo test -p huck-syntax --test highlight_spans`. Expected: does not compile (`highlight` module missing).

- [ ] **Step 3: Add the vocabulary**

```rust
// crates/huck-syntax/src/highlight.rs
//! What the highlighter needs to know about a line, produced BY the lexer.
//!
//! Roles are semantic, never colours — `huck-syntax` must not learn about SGR.
//! Nothing here is derived a second time downstream: if the highlighter would
//! have to re-answer a question the lexer already answered, the answer is
//! recorded here instead (#666).

/// What a source region IS. Ordered roughly outermost-to-innermost; a later
/// mark at the same offset refines an earlier one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The command word of a simple command — the thing whose existence is
    /// checked. Recorded for a word at command position, INCLUDING inside a
    /// substitution body, so `$(nosuchcmd)` marks its own command word.
    CommandWord,
    /// A reserved word (`if`, `for`, `do`, …) at command position.
    Keyword,
    QuotedSingle,
    QuotedDouble,
    /// The `$`/`${`/`$((`/`` ` ``/`<(` region as a whole.
    Expansion,
    /// The NAME inside an expansion — bolded.
    VarName,
    Operator,
    Redirect,
    Comment,
    /// A glob metacharacter run in an UNQUOTED literal (`*`, `?`, `[a-z]`).
    Glob,
    /// A backslash escape that the scanner consumed (`\$`, `\"` in a dquote).
    Escape,
    Tilde,
}

/// One recorded region. `end` is exclusive. The lexer sets both ends: `Span`
/// carries only a start, so an extent derived downstream from consecutive
/// starts would be a second source of truth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mark {
    pub start: usize,
    pub end: usize,
    pub role: Role,
}

/// A matched pair, recorded when its frame pops. `close` is the offset of the
/// closing character; both ends come from the frame, so nothing re-scans.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PairSpan {
    pub open: usize,
    pub close: usize,
}

/// Everything one highlight parse produced. Two lists of DIFFERENT things —
/// not index-parallel, so this is not the structure #641 forbids.
#[derive(Debug, Default, Clone)]
pub struct HighlightRecord {
    pub marks: Vec<Mark>,
    pub pairs: Vec<PairSpan>,
    /// The still-open pair at end of input, if any — the dangling opener
    /// (Task 6). Comes from the same walk v362 built.
    pub unterminated: Option<usize>,
}
```

- [ ] **Step 4: Add the flag and the ONE hook**

`LexerOptions` gains `pub record_highlight: bool` (defaults false). `Lexer` gains `hl: HighlightRecord`.

The hook goes in `scan_step_guarded`, which is the single wrapper every token
passes through — 121 `history.push` sites, one place they are all observed:

```rust
// in scan_step_guarded, after `let step = match self.scan_step() { ... }`
if self.opts.record_highlight {
    // Tokens appended by THIS step; `before_len` captured before the call.
    let end = self.cursor.offset();
    for tok in &self.history[before_len..] {
        if let Some(role) = highlight_role(&tok.kind) {
            self.hl.marks.push(crate::highlight::Mark {
                start: tok.span.offset,
                end,
                role,
            });
        }
    }
}
```

with a free function mapping kind -> role (`QuoteRun{Single}` -> `QuotedSingle`,
`BeginDquote` -> `QuotedDouble`, `DollarName` -> `VarName`, `ParamName` ->
`VarName`, `ParamOpen`/`CmdSubOpen`/`ArithOpen`/`BeginBacktick`/`ProcSubOpen`
-> `Expansion`, `Op` -> `Operator`, `RedirFd` -> `Redirect`, `Tilde` -> `Tilde`,
everything else `None`).

Add `pub fn take_highlight_record(&mut self) -> HighlightRecord { std::mem::take(&mut self.hl) }`.

- [ ] **Step 5: Run the tests — expect PASS.**

- [ ] **Step 6: INERTNESS GATES.** Parse sweep identical, corpus identical, both `--lib` suites, integration binaries, full sweep, pinned clippy, `cargo test --workspace --doc`. The flag is off everywhere in production, so all must be unchanged.

- [ ] **Step 7: Commit** — `feat(#666): record highlight spans behind a lexer flag`

---

### Task 2: The pty harness and first pixels

Nothing here has a bash to diff against, so the harness comes before the rest of the features — not after.

**Files:**
- Create: `crates/huck-cli/src/paint.rs`, `tests/highlight_render_pty.rs`
- Modify: `crates/huck-cli/src/completion_helper.rs`, `crates/huck-cli/src/lib.rs`

**Interfaces:**
- Consumes: `HighlightRecord` (Task 1).
- Produces: `paint::render(line: &str, rec: &HighlightRecord, enabled: bool) -> String`.

- [ ] **Step 1: Write the failing unit test for the painter**

```rust
// in paint.rs's #[cfg(test)] module
#[test]
fn paints_only_marked_regions_and_preserves_text() {
    let rec = HighlightRecord {
        marks: vec![Mark { start: 5, end: 9, role: Role::QuotedSingle }],
        ..Default::default()
    };
    let out = render("echo 'sq'", &rec, true);
    // Same visible text, colour only.
    assert_eq!(strip_sgr(&out), "echo 'sq'");
    assert!(out.contains("\x1b["));
}

#[test]
fn disabled_returns_the_line_untouched() {
    let rec = HighlightRecord { marks: vec![Mark { start: 0, end: 4, role: Role::CommandWord }], ..Default::default() };
    assert_eq!(render("echo hi", &rec, false), "echo hi");
}
```

- [ ] **Step 2: Run red.**

- [ ] **Step 3: Implement `paint::render`** — sort marks by `start`, later marks refine earlier ones, emit `ESC[<sgr>m` … `ESC[0m` around each region, copy unmarked bytes verbatim. Overlapping/zero-length marks are dropped rather than emitted (they would corrupt width).

- [ ] **Step 4: Wire the `Highlighter` impl** in `completion_helper.rs`:

```rust
impl Highlighter for HuckHelper {
    fn highlight<'l>(&self, line: &'l str, _pos: usize) -> Cow<'l, str> {
        if !self.colour_enabled { return Borrowed(line); }
        let empty = std::collections::HashMap::new();   // aliases DELIBERATELY empty
        let opts = LexerOptions { record_highlight: true, ..Default::default() };
        let mut lx = Lexer::new(line, &empty, opts);
        let _ = huck_syntax::parser::parse_sequence(&mut lx);  // errors are normal while typing
        Owned(paint::render(line, &lx.take_highlight_record(), true))
    }
    fn highlight_char(&self, _line: &str, _pos: usize, _kind: CmdKind) -> bool { true }
}
```

⚠️ `highlight_char` defaults to FALSE — without this override nothing ever re-renders.

- [ ] **Step 5: Write the pty harness**

```rust
// tests/highlight_render_pty.rs — asserts RENDERED bytes, the only way to test this
use expectrl::spawn;
#[test]
fn quoted_run_is_coloured_as_typed() {
    let mut p = spawn(&format!("{} -i", huck_binary().display())).unwrap();
    p.send_line("").unwrap();                 // settle the prompt
    p.send("echo 'sq'").unwrap();             // NO newline — we inspect the edit line
    let seen = read_until_quiet(&mut p);
    assert!(seen.contains("\x1b["), "expected SGR in the rendered line: {seen:?}");
    assert!(strip_sgr(&seen).contains("echo 'sq'"));
}
```

- [ ] **Step 6: Run both; then the inertness gates.** A non-tty run must emit no SGR at all — assert that explicitly, because the sweep pipes huck and would otherwise go red everywhere.

- [ ] **Step 7: Commit** — `feat(#666): paint recorded spans, with a pty rendering harness`

---

### Task 3: Command position, keywords, and a comment token

The lexer gains what it needs rather than the highlighter guessing.

**Files:** `crates/huck-syntax/src/lexer.rs`

- [ ] **Step 1: Failing tests** — `marks("ls -la")` contains `(0, Role::CommandWord)`; `marks("if true; then :; fi")` marks `if`/`then`/`fi` as `Keyword`; `marks("echo hi # note")` contains a `Comment` mark covering `# note`; `marks("echo $(nosuch)")` marks `nosuch` as `CommandWord` too.

- [ ] **Step 2: Run red.**

- [ ] **Step 3: Command position.** The recorder hook already sits where `self.cmd_pos` is live, so no public exposure is needed:

```rust
// inside the recorder, for a Lit/Word token
let role = if was_at_word_start && !cmd_pos_was_assignment {
    if is_shell_keyword(text) { Role::Keyword } else { Role::CommandWord }
} else { /* … */ };
```

Capture `let was_at_word_start = self.cmd_pos.at_word_start();` BEFORE `scan_step` runs, since the step advances it.

- [ ] **Step 4: The comment token.** `skip_line_comment` currently consumes to EOL and emits nothing. Give it the offsets and, when recording, push a `Comment` mark. Do NOT emit a real token — that would change parsing; the mark is recorded directly.

- [ ] **Step 5: Tests pass; inertness gates.** The comment change touches a hot path — the parse sweep is the gate that matters.

- [ ] **Step 6: Commit** — `feat(#666): mark command words, keywords and comments`

---

### Task 4: Invalid commands

**Files:** Create `crates/huck-engine/src/cmd_validity.rs`; modify `completion_helper.rs`, `repl.rs`

- [ ] **Step 1: Failing tests** — a cache that answers `Valid`/`Invalid`/`Unknown`; a miss populates it; `PATH` change clears it; a lookup exceeding the guard returns `Unknown` (never blocks).

- [ ] **Step 2: Run red.**

- [ ] **Step 3: Implement**

```rust
/// Positive AND negative command-validity cache for highlighting (#666).
///
/// #655's command hash table is NOT enough: it knows commands that have been
/// RUN, while the highlighter sees names that have only been TYPED — of `g`,
/// `gi`, `git`, two are misses, and a miss stats every PATH segment (measured:
/// 90-160 us, and ~940 us for a 6-stage pipeline of unknown words).
pub struct ValidityCache { seen: HashMap<String, bool> }
```

Resolution order mirrors the shell's: alias, function, builtin, keyword, the
command hash table, then a PATH search. Cleared on `PATH` change (reuse #655's
`invalidate_command_hash_if_path` chokepoint) and at each prompt, which bounds
staleness to one line so a freshly installed program is picked up next.

**Slow-filesystem guard:** time the PATH search; beyond a threshold return
`Unknown` and stop validity-colouring that word. Highlighting degrades, the
editor never stalls.

- [ ] **Step 4: pty test** — `nosuchcmd_xyz` renders red; `echo` renders plain.

- [ ] **Step 5: Gates. Commit** — `feat(#666): red for a command that does not exist`

---

### Task 5: Globs and escapes (sub-token marks)

A literal run is coalesced into ONE `Lit{text}` and the dquote scanner DROPS the backslash of `\$`, so neither is visible downstream. The scanners that see them record them.

**Files:** `crates/huck-syntax/src/lexer.rs`

- [ ] **Step 1: Failing tests** — `marks("ls *.rs")` has a `Glob` mark on `*` only; `marks("ls 'a*b'")` has NONE (quoted); `marks("echo \"a\\$b\"")` has an `Escape` mark on `\$`.

- [ ] **Step 2: Run red.**

- [ ] **Step 3: Implement** — in the unquoted literal-run scanner, when recording, push a `Glob` mark per metacharacter run (`*`, `?`, and a balanced `[…]`). In the double-quote escape arm, push an `Escape` mark spanning both characters. Both are inside `if self.opts.record_highlight` so the hot path is untouched when off.

- [ ] **Step 4: Gates. Commit** — `feat(#666): mark glob metacharacters and quoted escapes`

---

### Task 6: Bracket matching and the dangling opener

**Files:** `crates/huck-syntax/src/lexer.rs`, `crates/huck-cli/src/paint.rs`, `completion_helper.rs`

- [ ] **Step 1: Failing tests** — `pairs("echo $(date)")` yields one pair whose `open` is the `$(` offset and `close` the `)`; `unterminated("echo \"abc")` is `Some(5)`.

- [ ] **Step 2: Run red.**

- [ ] **Step 3: Implement.** Frames already carry `open_off` (v361) and pop at their closer, so both ends are in hand at the pop site — record a `PairSpan` there. For the dangling opener reuse v362's `pairs::reported_pair`, which already answers "which pair is still open, and where did it start".

- [ ] **Step 4: Cursor-aware painting.** `highlight(line, pos)` receives the cursor; if `pos` touches either end of a `PairSpan`, emphasise both. `highlight_char` already returns true for `CmdKind::MoveCursor`, so moving the cursor re-renders.

- [ ] **Step 5: pty test** with cursor movement — send arrow keys, assert the emphasis moves.

- [ ] **Step 6: Gates. Commit** — `feat(#666): match brackets under the cursor and mark a dangling opener`

---

### Task 7: Gating, docs, hand-off

**Files:** `paint.rs`, `repl.rs`, `docs/architecture.md`, `site/content/blog/`

- [ ] **Step 1:** `NO_COLOR` (any value disables), not-a-tty disables, and a
  `shopt` option to turn it off — `shopt -u syntax_highlight` / `shopt -s
  syntax_highlight`, on by default, which is the control users will reach for
  and the one that composes with an rc file. Tests for all three; the
  not-a-tty case is what keeps the 309-harness sweep green.
  (Configurable COLOURS are deliberately out of scope — #667.)
- [ ] **Step 2:** `docs/architecture.md` — a cross-cutting section: highlighting runs a parse, the lexer PRODUCES roles rather than the highlighter deriving them, and where the palette lives.
- [ ] **Step 3:** Blog entry with real before/after (a terminal capture, since this is visual).
- [ ] **Step 4:** Full verification from the branch point, then `gh pr create` with `Closes #666`. **Do NOT merge** — a `vNN` iteration PR is handed to the user.

---

## Self-Review

**Spec coverage.** Invalid command -> Task 4. Quote colours -> Tasks 1-2. Variable regions + bold name -> Task 1 (`VarName`). Operators/redirections -> Task 1. Keywords + comments -> Task 3. Substitution regions and the command inside them -> Tasks 1 and 3. Globs + escapes -> Task 5. Bracket matching + dangling opener -> Task 6. Validity cache and slow-fs guard -> Task 4. pty harness -> Task 2 (first, per the spec). `NO_COLOR`/tty/disable -> Task 7. Every spec section maps to a task.

**Placeholder scan.** None. Task 4's threshold value is deliberately unfixed — it is measured against the box during that task, not guessed here.

**Type consistency.** `Role`, `Mark`, `PairSpan`, `HighlightRecord` are defined in Task 1 and used with those names in Tasks 2-6. `render(line, rec, enabled)` is introduced in Task 2 and reused in Task 6. `take_highlight_record` is the only accessor.

**Ordering.** Task 1 is inert and provable. Task 2 makes it visible and builds the harness everything later needs. Tasks 3-6 each add one category with its own gates. Task 7 is the gating and hand-off. Each task ends green, so the branch is landable at any point.
