# v252 — Array literals (`name=(…)`) on the atom-command path — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Port compound array-literal assignment (`name=(…)`) onto huck's DORMANT
atom-command parser, byte-identical to the `command.rs` oracle, via a real
`Mode::ArrayLiteral`.

**Architecture:** The lexer detects `name=(` / `name+=(` / `name[sub]=(` in
assignment-value position and emits a zero-width `ArrayOpen` signal; the parser
pushes `Mode::ArrayLiteral`, whose scanner emits *element atoms* (value-word
content + whitespace/newline separators + subscript brackets + `ArrayClose`), and
the parser (`parser.rs`) assembles `WordPart::ArrayLiteral(Vec<ArrayLiteralElement>)`
+ owns the `)` matching. THE RULE: the lexer emits small atoms and NEVER scans
ahead for the matching `)`; the parser owns delimiter-matching + assembly.

**Tech Stack:** Rust; `crates/huck-syntax/src/lexer.rs` (atoms + mode scanner),
`crates/huck-syntax/src/parser.rs` (assembly + differential tests). No
`command.rs` change.

## Global Constraints

- **Dormant + differential.** `command_atoms` stays `false` at both definition
  sites in `lexer.rs`. Every in-scope input must parse to the SAME AST / same
  error on the atom path (`new_seq`) as the oracle (`old_seq`); `diff_cmd(s)`
  asserts equality. A well-formed in-scope divergence is a BUG to fix, not to pin.
- **Production untouched.** Do NOT modify `scan_array_literal`,
  `scan_array_element_word`, `scan_subscript`, `skip_array_literal_separators`,
  the production `=`/`+=`/`[sub]=` Word-scanner arms (`lexer.rs:2616`/`2635`/`2660`),
  `command.rs`, or `process_line`. `git diff main -- crates/huck-syntax/src/command.rs`
  MUST stay empty. Engine-facing `WordPart::ArrayLiteral` / `ArrayLiteralElement`
  AST unchanged.
- **THE RULE.** The lexer emits small atoms and never scans ahead for the matching
  `)`. The parser owns the close and recursion.
- **Test runner (box is 1 core / 1.9 GiB — `--workspace`/parallel OOM-kills the
  session).** ALWAYS: `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1`
  (narrow with a name filter while iterating). Warnings check:
  `cargo build -p huck-syntax 2>&1 | grep -c warning` → must print `0`.
- **Branch** `v252-array-literals` (NOT main). Every commit ends with
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- **rust-analyzer phantom diagnostics**: the IDE intermittently shows false
  compile errors after edits. Trust `cargo`, not the IDE.

---

## Background the implementer needs

**The oracle (`scan_array_literal`, `lexer.rs:6099`) — the grammar to match:**
loop → `skip_array_literal_separators` (whitespace / newline / `\<NL>` line
continuation / `#`-comment); `)` → done; EOF → `LexError::UnterminatedArrayLiteral`;
optional `[expr]=` prefix (a `]` not followed by `=` →
`LexError::ArrayLiteralMissingEquals`); a value via `scan_array_element_word`
(`lexer.rs:6194`) which stops ONLY at whitespace / `)` (so `| ; & < >` are LITERAL
inside a value). A **subscripted** element keeps its single value; a **bare**
element is `brace_expand_parts(value.0)?` → N positional elements.

**How `name=(…)` becomes ONE Word in the oracle:** the production `=` arm pushes
`Literal("name=")` then `ArrayLiteral(elems)` onto `self.parts` — so the Word is
`[Literal("name="), ArrayLiteral(...)]`. For `+=` it is `[AssignPrefix{Bare(name),
append:true}, ArrayLiteral(...)]`; for `[sub]=` it is `[AssignPrefix{Indexed{…}},
ArrayLiteral(...)]`.

**Atom-path prefix emission today (`try_scan_assign_prefix`, `lexer.rs:3521`):** at
word start it emits `Lit{ text:"name=" }` (for `=`), or `AssignPrefix{ Bare, append:
true }` (for `+=`), or `AssignPrefix{ Indexed{name,subscript}, append }` (for
`[sub]=`/`[sub]+=`), and sets `in_assignment_value`. It does NOT look for a
following `(`; the `(` currently falls to the operator arm as `Op(LParen)`, and the
parser defers (`name=` is assignment-shaped + a bare `LParen` → the v251
`UnsupportedCommand` catch-all in `parse_simple_with_leading_word`).

**Reusable machinery:**
- `Token::new(kind, Span::new(off, l, c))` pushed onto `self.history`; a single
  `scan_step` MAY push more than one token (multi-token emit).
- `skip_line_continuations(&mut self.cursor)` (`lexer.rs:6082`).
- `scan_step_dquote` (`lexer.rs:~3663`) is the closest template for a new
  `body_started`-gated mode scanner: it consumes its opener when `!body_started`,
  flips the flag, and scans the first inner atom in the same call; it emits `Lit`
  literal runs + the shared expansion openers (`ParamOpen`/`ArithOpen`/`CmdSubOpen`/
  `BeginBacktick`/`DollarName`) + quote handling.
- `parse_command_sub` (`parser.rs:781`) is the template for a mode-push/parse/pop
  word-part builder (incl. the empty-body and pop-on-error discipline).
- `parse_word_command(iter, quoted)` (`parser.rs:118`) assembles a `Word` from
  atoms and BREAKS on `None | Blank | Newline | Op(_)` and other non-word atoms —
  reuse it to assemble each element value (it will stop at a `Blank`/`Newline`
  separator or at `ArrayClose`). Its `CmdSubOpen`/`ArithOpen`/`BeginBacktick`/
  `ParamOpen` arms recurse into nested expansions.
- The `${a[i]}` subscript reader in `parse_param_expansion` (`parser.rs:422-444`):
  on `LBracket`, `next_kind()`, `push_mode(Mode::ParamSubscriptOperand{in_dquote:
  false})`, assemble the subscript `Word`, consume `RBracket`, `pop_mode()`.
- `brace_expand_parts(parts: Vec<WordPart>) -> Result<Vec<Vec<WordPart>>, LexError>`
  (`lexer.rs:4329`, currently private) — the parser calls it on each bare value.

---

## Task 1: Positional array literals end-to-end (core)

Delivers `name=(…)` / `name+=(…)` with positional, brace-expanded, literal-meta
values parsing byte-identically. (A new mode is not `diff_cmd`-testable until the
parser pushes it, so lexer + parser land together here; richer values, separators,
and subscripts follow in Tasks 2-3.)

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` — `TokenKind` (add `ArrayOpen`,
  `ArrayClose`); `Mode::ArrayLiteral` (add `{ body_started: bool }`); the
  `scan_step` dispatch match (`lexer.rs:925`); `try_scan_assign_prefix`'s `=` and
  `+=` branches; new `scan_step_array_literal`; make `brace_expand_parts`
  `pub(crate)`.
- Modify: `crates/huck-syntax/src/parser.rs` — new `parse_array_literal`; new
  `ArrayOpen` arm in `parse_word_command`; tests in `mod tests`.

**Interfaces:**
- Produces: `TokenKind::ArrayOpen` (zero-width, cursor left on `(`),
  `TokenKind::ArrayClose`; `Mode::ArrayLiteral { body_started: bool }`;
  `fn scan_step_array_literal(&mut self, body_started: bool) -> Result<Step, LexError>`;
  `fn parse_array_literal(iter: &mut Lexer) -> Result<WordPart, ParseError>`.
- Consumes: `try_scan_assign_prefix` (emits the prefix), `skip_line_continuations`,
  `parse_word_command`, `brace_expand_parts`, `push_lit`/`flush_lit`.

- [ ] **Step 1: Add the atoms.** In `TokenKind` (near `CmdSubOpen`/`ProcSubOpen`,
  `lexer.rs:420`) add:

```rust
    ArrayOpen,   // v252: zero-width signal that a compound array RHS `(…)` follows an assignment prefix; cursor left on `(`. Parser pushes Mode::ArrayLiteral.
    ArrayClose,  // v252: the `)` closing an array literal, emitted by Mode::ArrayLiteral.
```

- [ ] **Step 2: Make the mode carry `body_started`.** Change `lexer.rs:635` from
  `ArrayLiteral,` to `ArrayLiteral { body_started: bool },` and add a dispatch arm
  in the `scan_step` match (before the `other =>` catch-all at `lexer.rs:939`):

```rust
            Mode::ArrayLiteral { body_started } => self.scan_step_array_literal(body_started),
```

- [ ] **Step 3: Emit `ArrayOpen` after the `=` and `+=` prefixes.** In
  `try_scan_assign_prefix`, in the `Some('=') =>` branch, immediately BEFORE its
  `Ok(Some(Step::Produced))`, and in the `Some('+') =>` branch likewise, insert
  (using a fresh position for the signal's span — the cursor now sits after the
  prefix, before `(`):

```rust
                // v252: compound array RHS `name=(...)` / `name+=(...)`. A `\<NL>`
                // may sit between the prefix and `(` (bash deletes it). Mirror the
                // production `=` arm's inline `(` probe: emit a zero-width ArrayOpen
                // so the parser pushes Mode::ArrayLiteral. Cursor is LEFT on `(`.
                skip_line_continuations(&mut self.cursor);
                if self.cursor.peek() == Some(&'(') {
                    let (ao, al, ac) = (self.cursor.offset(), self.cursor.line(), self.cursor.column());
                    self.history.push(Token::new(TokenKind::ArrayOpen, Span::new(ao, al, ac)));
                }
```

- [ ] **Step 4: Write `scan_step_array_literal` (positional values + separators +
  close + Unterminated).** Add next to `scan_step_dquote`. On entry with
  `!body_started`, the cursor is on `(`: consume it, flip the frame, fall through.
  Then emit ONE atom per call, mirroring `scan_array_literal`'s grammar but for
  POSITIONAL values only (leave `[` as an ordinary literal char here — subscripts
  arrive in Task 3):

```rust
    fn scan_step_array_literal(&mut self, body_started: bool) -> Result<Step, LexError> {
        if !body_started {
            debug_assert_eq!(self.cursor.peek(), Some(&'('), "array-literal entry: expected '('");
            self.cursor.next(); // consume opening '('
            if let Some(Mode::ArrayLiteral { body_started }) = self.modes.last_mut() {
                *body_started = true;
            }
            // fall through to scan the first atom
        }
        let (off, l, c) = (self.cursor.offset(), self.cursor.line(), self.cursor.column());
        match self.cursor.peek().copied() {
            None => Err(LexError::UnterminatedArrayLiteral),
            Some(')') => {
                self.cursor.next();
                self.history.push(Token::new(TokenKind::ArrayClose, Span::new(off, l, c)));
                Ok(Step::Produced)
            }
            // Inter-element separators: whitespace / newline / `\<NL>` / `#`-comment.
            // Coalesce a maximal run into ONE Blank atom (a comment consumes to EOL,
            // its body — incl. any `)` — never read as elements; matches
            // skip_array_literal_separators). Never emit content for a separator.
            Some(ch) if ch.is_whitespace() || ch == '#'
                || (ch == '\\' && { let mut p = self.cursor.clone(); p.next(); p.peek() == Some(&'\n') }) => {
                loop {
                    match self.cursor.peek().copied() {
                        Some(w) if w.is_whitespace() => { self.cursor.next(); }
                        Some('#') => { while let Some(&x) = self.cursor.peek() { if x == '\n' { break; } self.cursor.next(); } }
                        Some('\\') => {
                            let mut p = self.cursor.clone(); p.next();
                            if p.peek() == Some(&'\n') { self.cursor.next(); self.cursor.next(); } else { break; }
                        }
                        _ => break,
                    }
                }
                self.history.push(Token::new(TokenKind::Blank, Span::new(off, l, c)));
                Ok(Step::Produced)
            }
            // Value content — a literal run stopping ONLY at whitespace / `)` (so
            // `|;&<>` stay literal). Quotes/`$`/backtick delegate to the shared
            // openers (Task 2 widens this); for Task 1, emit a plain literal run and
            // let Task 2 add the expansion arms.
            _ => {
                let mut text = String::new();
                while let Some(&ch) = self.cursor.peek() {
                    if ch.is_whitespace() || ch == ')' { break; }
                    // Task 2 will break here on quote/`$`/backtick to emit openers.
                    text.push(ch);
                    self.cursor.next();
                }
                self.history.push(Token::new(TokenKind::Lit { text, quoted: false }, Span::new(off, l, c)));
                Ok(Step::Produced)
            }
        }
    }
```

- [ ] **Step 5: Make `brace_expand_parts` callable from the parser.** Change
  `fn brace_expand_parts` (`lexer.rs:4329`) to `pub(crate) fn brace_expand_parts`.

- [ ] **Step 6: Write `parse_array_literal`** in `parser.rs` (near
  `parse_command_sub`). Pop the mode on EVERY exit path:

```rust
/// v252: assemble `WordPart::ArrayLiteral` from atoms under `Mode::ArrayLiteral`.
/// The caller (parse_word_command's ArrayOpen arm) has already discarded the
/// zero-width ArrayOpen signal; the cursor is on `(`.
pub(crate) fn parse_array_literal(iter: &mut Lexer) -> Result<WordPart, ParseError> {
    iter.push_mode(Mode::ArrayLiteral { body_started: false });
    let mut elements: Vec<ArrayLiteralElement> = Vec::new();
    loop {
        match iter.peek_kind() {
            Ok(Some(TokenKind::Blank)) | Ok(Some(TokenKind::Newline)) => { iter.next_kind()?; }
            Ok(Some(TokenKind::ArrayClose)) => { iter.next_kind()?; break; }
            Ok(Some(_)) => {
                // A positional value: parse_word_command stops at the next
                // Blank/Newline/ArrayClose. Then brace-expand (bare elements).
                let value = match parse_word_command(iter, false) {
                    Ok(v) => v,
                    Err(e) => { iter.pop_mode(); return Err(e); }
                };
                match brace_expand_parts(value.0) {
                    Ok(expansions) => {
                        for p in expansions {
                            elements.push(ArrayLiteralElement { subscript: None, value: Word(p) });
                        }
                    }
                    Err(e) => { iter.pop_mode(); return Err(ParseError::Lex(e)); }
                }
            }
            Ok(None) => { iter.pop_mode(); return Err(ParseError::UnexpectedEof); }
            Err(e) => { iter.pop_mode(); return Err(e); }
        }
    }
    iter.pop_mode();
    Ok(WordPart::ArrayLiteral(elements))
}
```

  Notes for the implementer: confirm the exact `ParseError` variant names by
  grepping (`ParseError::Lex`, `ParseError::UnexpectedEof` — use whatever the crate
  actually defines; `parse_command_sub` and the lex-pull sites show the real
  names). `parse_word_command` making progress is guaranteed here because it is
  only called when `peek_kind` is a real value atom (never a separator/close), so
  no empty-word infinite loop (the v247 OOM hazard).

- [ ] **Step 7: Add the `ArrayOpen` arm to `parse_word_command`.** Right after the
  `CmdSubOpen` arm (`parser.rs:~161`). ArrayOpen only ever follows a prefix part
  (`Lit "name="` or `AssignPrefix`), so it GLUES (like `CmdSubOpen`), and needs NO
  entry in the `parse_simple_with_leading_word` word-start set:

```rust
            // v252: compound array RHS. The prefix part (Literal "name=" or
            // AssignPrefix) is already accumulated; glue the ArrayLiteral after it.
            Some(TokenKind::ArrayOpen) => {
                iter.next_kind()?;            // discard the signal (cursor on `(`)
                flush_lit(&mut acc, &mut parts);
                parts.push(parse_array_literal(iter)?);
            }
```

  Add `ArrayLiteralElement` and `Word` to the `use crate::lexer::{…}` imports if
  not already present.

- [ ] **Step 8: Write the failing test, run it (expect FAIL), then verify PASS
  after Steps 1-7.** Add to `parser.rs` `mod tests`:

```rust
    #[test]
    fn atoms_array_literal_positional() {
        diff_cmd("a=(1 2 3)");
        diff_cmd("a=()");                 // empty
        diff_cmd("a=(x)");                // single
        diff_cmd("a=(  1   2  )");        // extra spaces
        diff_cmd("arr+=(4 5)");           // append form
        diff_cmd("a=(a|b c;d e<f)");      // |;&<> literal inside values
        diff_cmd("a=({1..3})");           // brace-expanded bare element -> 1 2 3
        diff_cmd("a=(x{a,b}y)");          // brace expansion with prefix/suffix
        diff_cmd("pre a=(1 2) post");     // assignment mid-command still one word
    }
```

  Run: `cargo test -p huck-syntax --jobs 1 --lib atoms_array_literal_positional -- --test-threads 1`
  Expected: FAIL before Steps 1-7 (parser defers → `UnsupportedCommand` unwrap
  panic in `new_seq`), PASS after. If any case diverges, print `new_seq` vs
  `old_seq` and reconcile to the oracle (fix the atom path, never the oracle).

- [ ] **Step 9: Full-suite + warnings gate, then commit.**
  Run: `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` (expect all
  green, count ≥ prior 966) and `cargo build -p huck-syntax 2>&1 | grep -c warning`
  (expect `0`).

```bash
git add crates/huck-syntax/src/lexer.rs crates/huck-syntax/src/parser.rs
git commit -m "v252 T1: positional array literals (name=(…)/+=) via Mode::ArrayLiteral

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Rich values + separators

Delivers quoting / expansions / tilde / globs inside element values, plus
newline and comment separators — everything `scan_array_element_word` accepts.

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` — the value-content arm of
  `scan_step_array_literal` (add the shared expansion/quote openers).
- Modify: `crates/huck-syntax/src/parser.rs` — tests only (the value atoms already
  route through `parse_word_command`'s existing arms).

**Interfaces:**
- Consumes: the shared opener emission already used by `scan_command_word_atom` /
  `scan_step_dquote` (`ParamOpen`/`ArithOpen`/`CmdSubOpen`/`BeginBacktick`/
  `DollarName`, quote runs, `Tilde`).

- [ ] **Step 1: Widen the value-content arm of `scan_step_array_literal`.** Replace
  the Task-1 plain-literal `_ =>` value arm so it BREAKS the literal run at a quote
  / `$` / backtick and emits the SAME opener atoms `scan_step_dquote` (and the
  command-word scanner) emit there. Study `scan_step_dquote`'s `$`/backtick/quote
  arms (`lexer.rs:~3663+`) and `scan_command_word_atom`'s `$` arm (`lexer.rs:3339`)
  and mirror them: `${`→`ParamOpen{quoted:false}`, `$((`→`ArithOpen`, `$(`→
  `CmdSubOpen`, `$name`/`$1`/`$@`…→`DollarName{quoted:false}`, `` ` ``→
  `BeginBacktick`, `'`/`"`→`QuoteRun`/`BeginDquote` exactly as command position
  does, tilde at value start→`Tilde`. The literal run still stops at whitespace /
  `)` for plain chars (so `|;&<>` remain literal). Keep it a faithful mirror — the
  goal is that `parse_word_command` sees the identical atom stream it would for the
  same text in command position, EXCEPT the stop-set is whitespace/`)` not the
  command operators.

- [ ] **Step 2: Write the failing test, verify FAIL→PASS.** Add:

```rust
    #[test]
    fn atoms_array_literal_rich_values() {
        diff_cmd("a=(\"x y\" 'z' bare)");            // double/single/bare
        diff_cmd("a=($x ${y} ${z:-d})");             // param expansions
        diff_cmd("a=($(echo hi) `echo bye`)");       // command subs
        diff_cmd("a=($((1 + 2)) end)");              // arith
        diff_cmd("a=(~ ~/x a=~)");                   // tilde eligibility
        diff_cmd("a=(*.txt foo?bar [ab]c)");         // globs (patterns kept literal in AST)
        diff_cmd("a=(pre$xpost \"$mix\"tail)");      // adjacency/glue within a value
        diff_cmd("a=(\n  one\n  two\n)");            // newline separators
        diff_cmd("a=(one # comment\n two)");         // comment separator
        diff_cmd("arr=\\\n(1 2)");                   // `\<NL>` between prefix and `(`
    }
```

  Run: `cargo test -p huck-syntax --jobs 1 --lib atoms_array_literal_rich_values -- --test-threads 1`
  Expected: FAIL before Step 1 (values lex as one flat literal → AST mismatch),
  PASS after. Reconcile any divergence to `old_seq`.

- [ ] **Step 3: Full-suite + warnings gate, then commit.**

```bash
git add crates/huck-syntax/src/lexer.rs crates/huck-syntax/src/parser.rs
git commit -m "v252 T2: rich element values (quotes/expansions/tilde/globs) + separators

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Subscripted elements `[i]=value` + `name[sub]=(…)` prefix

Delivers explicit-subscript elements inside the literal and the subscripted
lvalue prefix form, with `ArrayLiteralMissingEquals` parity.

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` — `scan_step_array_literal` (recognize
  `[` at element start → `LBracket`; consume the required `=` after the subscript's
  `]`, else `ArrayLiteralMissingEquals`); `try_scan_assign_prefix`'s `[sub]=` branch
  (emit `ArrayOpen` after `(`).
- Modify: `crates/huck-syntax/src/parser.rs` — `parse_array_literal` (the
  subscript branch); tests.

**Interfaces:**
- Consumes: `LBracket`/`RBracket` + `Mode::ParamSubscriptOperand` and the subscript
  `Word` reader from `parse_param_expansion` (`parser.rs:422-444`).
- Produces: `ArrayLiteralElement { subscript: Some(Word), value }` (single value, NO
  brace expansion — matches `scan_array_literal`).

- [ ] **Step 1: Emit `ArrayOpen` after the `[sub]=` prefix.** In
  `try_scan_assign_prefix`'s `Some(append) =>` arm (the confirmed indexed
  assignment, `lexer.rs:~3603`), immediately before its `Ok(Some(Step::Produced))`,
  insert the SAME `skip_line_continuations` + `(`-probe + `ArrayOpen`-emit block
  used in Task 1 Step 3.

- [ ] **Step 2: Recognize a subscript at element start in
  `scan_step_array_literal`.** Add lexer state to the mode to track "just opened a
  subscript" — extend the variant to
  `Mode::ArrayLiteral { body_started: bool, expect_subscript_eq: bool }` (update the
  enum, the dispatch arm's destructuring, and the `push_mode` call sites). In the
  value dispatch, BEFORE the value-content arm, when at ELEMENT START (i.e. the
  previous atom was a separator/open, not mid-value) and `self.cursor.peek() ==
  Some(&'[')`:

```rust
            Some('[') => {
                self.cursor.next(); // consume '['
                if let Some(Mode::ArrayLiteral { expect_subscript_eq, .. }) = self.modes.last_mut() {
                    *expect_subscript_eq = true;
                }
                self.history.push(Token::new(TokenKind::LBracket, Span::new(off, l, c)));
                Ok(Step::Produced)
            }
```

  Then, when `expect_subscript_eq` is set (control has returned from the parser's
  subscript scan, cursor is just past `]`): require `=` — consume it, clear the
  flag, and FALL THROUGH to scan the value's first atom in the same call; if the
  next char is not `=`, return `Err(LexError::ArrayLiteralMissingEquals)`. Structure
  this as the first check at the top of the post-`body_started` body:

```rust
        if let Some(Mode::ArrayLiteral { expect_subscript_eq: e @ true, .. }) = self.modes.last_mut() {
            *e = false;
            if self.cursor.peek() == Some(&'=') {
                self.cursor.next(); // consume '='; fall through to scan the value
            } else {
                return Err(LexError::ArrayLiteralMissingEquals);
            }
        }
```

  Guidance: `[` is a subscript ONLY at element start. A `[` encountered MID-value
  (inside the value-content literal run) stays literal (`a=(x[0]y)` → element
  `x[0]y`); ensure the element-start `[` arm is reached only when no value literal
  is in progress in the current `scan_step` (it naturally is, since each element
  begins with a fresh `scan_step` after a separator/open). Verify against the
  oracle with the tests below and reconcile.

- [ ] **Step 3: Add the subscript branch to `parse_array_literal`.** Before the
  positional `Ok(Some(_)) =>` value arm, add an `LBracket` arm mirroring
  `parse_param_expansion:422-444`:

```rust
            Ok(Some(TokenKind::LBracket)) => {
                iter.next_kind()?; // consume LBracket
                iter.push_mode(Mode::ParamSubscriptOperand { in_dquote: false });
                let sub_word = match parse_word(iter) {           // assembles until RBracket
                    Ok(w) => w,
                    Err(e) => { iter.pop_mode(); iter.pop_mode(); return Err(e); }
                };
                match iter.next_kind() {
                    Ok(Some(TokenKind::RBracket)) => {}
                    Ok(_) | Ok(None) => { iter.pop_mode(); iter.pop_mode(); return Err(ParseError::UnexpectedToken); }
                    Err(e) => { iter.pop_mode(); iter.pop_mode(); return Err(e); }
                }
                iter.pop_mode(); // ParamSubscriptOperand
                // The lexer consumed the required `=` (or errored ArrayLiteralMissingEquals).
                let value = match parse_word_command(iter, false) {
                    Ok(v) => v,
                    Err(e) => { iter.pop_mode(); return Err(e); }
                };
                elements.push(ArrayLiteralElement { subscript: Some(sub_word), value });
            }
```

  Replace `parse_word` with the ACTUAL subscript-word reader used at
  `parser.rs:422-444` (grep the function name — it may be `parse_word`,
  `parse_subscript_word`, or inline). Match its exact signature and the exact
  `RBracket`-handling those lines use. Note the DOUBLE `pop_mode` on the inner error
  paths (both `ParamSubscriptOperand` and the outer `ArrayLiteral`).

- [ ] **Step 4: Write the failing test, verify FAIL→PASS.** Add:

```rust
    #[test]
    fn atoms_array_literal_subscripts() {
        diff_cmd("a=([0]=x [1]=y)");             // explicit subscripts
        diff_cmd("a=([2]=two 1 [0]=zero)");      // mixed positional + subscripted
        diff_cmd("a=([i+1]=v)");                 // arithmetic subscript expr
        diff_cmd("a=([k]={a,b})");               // subscripted: brace stays LITERAL (no expansion)
        diff_cmd("m[k]=(1 2)");                  // name[sub]=(…) prefix form
        diff_cmd("m[k]+=(3)");                   // name[sub]+=(…) prefix form
    }

    #[test]
    fn atoms_array_literal_error_parity() {
        // `[i]` without `=` → ArrayLiteralMissingEquals on BOTH paths (lexer-level).
        assert!(new_seq("a=([0])").is_err());
        assert!(old_seq_result("a=([0])").is_err()); // use the crate's fallible oracle helper
        // EOF before `)` → UnterminatedArrayLiteral on both.
        assert!(new_seq("a=(1 2").is_err());
        assert!(old_seq_result("a=(1 2").is_err());
        assert!(new_seq("a=(").is_err());
        assert!(old_seq_result("a=(").is_err());
    }
```

  For the error test, use whatever fallible oracle accessor the existing
  error-parity tests use (grep for other `*_missing_*`/`Unterminated` tests in
  `parser.rs`/`lexer.rs` to copy the exact `is_err`/variant-match idiom).
  Run: `cargo test -p huck-syntax --jobs 1 --lib atoms_array_literal_sub -- --test-threads 1`
  and `... atoms_array_literal_error_parity ...`. Expected FAIL before, PASS after.

- [ ] **Step 5: Full-suite + warnings gate, then commit.**

```bash
git add crates/huck-syntax/src/lexer.rs crates/huck-syntax/src/parser.rs
git commit -m "v252 T3: subscripted [i]=value elements + name[sub]=(…) prefix; MissingEquals parity

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: `declare`/`local` observation, adversarial corpus, error parity, gate

Delivers the full differential corpus, the `declare`-routing decision, and the
final green gate.

**Files:**
- Modify: `crates/huck-syntax/src/parser.rs` — tests (and any small reconciliation
  fixes the corpus surfaces, in `lexer.rs`/`parser.rs`).

- [ ] **Step 1: Determine the `declare`/`local` routing by OBSERVATION.** Add
  probe tests and run them to see whether `declare -a x=(1 2)` / `local a=(1 2)` /
  `export e=(1)` / `readonly r=(1)` parse through the SAME command-word path (in
  which case they already `diff_cmd`-match and are supported for free) or through a
  different `DeclArg` pre-parse (in which case the atom path defers). Grep for
  `DeclArg` and how `declare`/`local` args are parsed. Record the finding as a test:

```rust
    #[test]
    fn atoms_array_literal_declare_routing() {
        // If declare/local args route through the command-word path, these are
        // diff_cmd. If they route through DeclArg (different path), replace each
        // with the observed-actual behavior and leave a NOTE comment documenting
        // the deferral (per spec: declare is deferred ONLY if it routes differently).
        diff_cmd("declare -a x=(1 2)");
        diff_cmd("local a=(1 2)");
        diff_cmd("export e=(1)");
        diff_cmd("readonly r=(1)");
    }
```

  If any of these do NOT `diff_cmd`-match because of `DeclArg` routing (not an
  array-literal atom bug), convert that case to an error-parity/observed assertion
  and add a one-line NOTE comment citing the deferral; report it as a concern for
  the whole-branch review (candidate follow-on `[deferred]` divergence). If they
  match, keep them as `diff_cmd`.

- [ ] **Step 2: Add the adversarial corpus.** Cover nesting, adjacency, and edge
  shapes; reconcile every divergence to the oracle (fix the atom path):

```rust
    #[test]
    fn atoms_array_literal_corpus() {
        diff_cmd("a=($(cat <<X\nhi\nX\n))");        // NOTE: heredoc-in-cmdsub is a known v250 gap; if it diverges, drop this line and rely on the cmdsub-body carry-forward — do NOT pin new
        diff_cmd("a=(${arr[@]} ${arr[*]})");        // array expansions as values
        diff_cmd("a=(x=y z=w)");                     // `=`-containing values (NOT subscripts)
        diff_cmd("a=(=leading)");                    // value starting with `=`
        diff_cmd("a=(one)(two)");                    // `)(` — second `(` is NOT an array open
        diff_cmd("a=(a)b");                          // text glued after the close paren
        diff_cmd("cmd a=(1 2) b=(3 4)");             // two array assignments in one command
        diff_cmd("a=(   )");                         // whitespace-only body == empty
        diff_cmd("a=(\n)");                          // newline-only body == empty
        diff_cmd("nots a =(1 2)");                   // space before `=` → NOT an assignment (a, =(1, 2) words)
    }
```

  For any line that legitimately differs due to an UNPORTED family (e.g. a `[[ ]]`
  or `$[ ]` inside a value), convert to the established deferral posture
  (`UnsupportedExpansion` assertion) exactly as v251 did for `[[ ]]` in procsub
  bodies — do NOT invent a new pin unless the divergence is genuinely array-literal
  specific and new. Verify each `a=(one)(two)` / `a=(a)b` shape against `old_seq`
  and drop/adjust any whose oracle behavior you cannot reproduce, documenting why.

- [ ] **Step 3: Final gate.**
  Run: `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` (all green),
  `cargo test -p huck-syntax --jobs 1 --doc -- --test-threads 1` (green),
  `cargo build -p huck-syntax 2>&1 | grep -c warning` (`0`), and confirm
  `git diff main -- crates/huck-syntax/src/command.rs` is empty and both
  `command_atoms` sites are still `false`.

- [ ] **Step 4: Commit.**

```bash
git add crates/huck-syntax/src/parser.rs crates/huck-syntax/src/lexer.rs
git commit -m "v252 T4: declare-routing observation + adversarial array-literal corpus + gate

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Notes carried from prior iterations (read before starting)

- **Byte-identical is the bar.** The differential corpus is the real spec; when a
  `diff_cmd` fails, print `new_seq(s)` and `old_seq(s)` and fix the ATOM path to
  match the oracle — never the oracle. (v251 caught a wrong glue assumption this
  way; expect the corpus to catch spec imperfections here too — e.g. exact
  separator/brace-expansion behavior.)
- **Progress guarantee (v247 OOM hazard).** Never call `parse_word_command` (or any
  sub-parser) when the current atom is a separator/close — only on a genuine value
  atom — so no zero-progress loop can spin. The box OOM-kills the whole session on
  a runaway loop; if a test hangs, it is almost certainly a non-progress bug, not
  the runner.
- **mark/rewind hazard (v248).** This port needs NO speculative mark/rewind
  (detection is a bounded 1-char peek after the prefix; the subscript is
  parser-driven). Do not add one.
- **Pop discipline.** Every `push_mode` must be balanced by a `pop_mode` on ALL
  paths including errors (see `parse_command_sub` and the double-pop in Task 3
  Step 3).
```
