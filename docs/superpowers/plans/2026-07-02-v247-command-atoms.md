# v247 Command-mode-emits-atoms (pure mechanical inversion, dormant) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give `Command` mode an atom-emitting variant (`scan_step_command_atoms`) and make `parser.rs`'s command parser assemble command-position Words from those atoms, producing ASTs byte-identical to the production `command.rs` path — dormant, gated by an old-==-new differential harness, with NO new grammar.

**Architecture:** A construction-time `command_atoms` flag selects `scan_step_command_atoms` (emits word-atoms + a `Blank` word-boundary token + the same structural tokens) over today's Word-emitting `scan_step_command`. The atom scanner is the operand scanner's word/expansion-atom emission (`scan_step_param_operand`) + `scan_step_command`'s structural tokens + `Blank`. `parser.rs` assembles command words via a command-context `parse_word`. Production stays on Word-mode + `command.rs`.

**Tech Stack:** Rust, `crates/huck-syntax` (`lexer.rs`, `parser.rs`). No new dependencies.

## Global Constraints

- Test ONLY with `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` (narrow with filters). This box (1 core / ~1.9 GiB) is OOM-KILLED by `cargo test --workspace` or any parallel/multi-threaded run — NEVER run those.
- The lexer emits atoms and NEVER scans ahead for a matching delimiter; forward-only, bounded local peek. The PARSER owns word assembly + recursion (mode push/pop + mark/rewind). The atom scanner must be atom-native: at `$(`/`${`/`` ` ``/`$((` it emits the opener SIGNAL and lets the parser push the sub-mode — it must NOT call the fat scanners (`scan_dollar_expansion`, `scan_arith_body`, `scan_backtick_body`, `scan_braced_param_expansion`) to pre-build a sub-part.
- PRODUCTION IS UNTOUCHED: `scan_step_command` (Word emission), `command.rs`, the fat scanners, `process_line`, and every live path are unchanged. `command_atoms` defaults `false`. All existing tests pass by construction.
- Pure mechanical inversion: NO new grammar. Deferred constructs (arith cmd `(( ))`, C-for, `[[ ]]`, heredoc bodies, here-strings, funcdef, coproc, array literals `a=(…)`, command-position alias) return `UnsupportedCommand` on the atom path — same as `parser.rs` does today.
- Differential gate: for every in-scope input, `atoms → parser.rs` AST == `Words → command.rs` AST. A well-formed in-scope divergence is a BUG to fix, not to pin (v247 should have zero legitimate divergences).
- Every commit ends with `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- Branch: `v247-command-atoms`. Do NOT commit to `main`.

## Key existing anchors (read before starting)

- `struct Lexer` fields: `lexer.rs:599-636` (add `command_atoms: bool` after `retokenize_arith_as_cmdsub`).
- `Lexer::new(input, opts, live)` constructor (the struct literal with `modes: vec![Mode::Command], retokenize_arith_as_cmdsub: false`): in `impl<'a> Lexer` starting `lexer.rs:638`. `new_live(input, aliases, opts)` at `lexer.rs:2781` delegates to `new`.
- `scan_step` dispatch: `lexer.rs:729-741` (the `Mode::Command => self.scan_step_command()` arm at :731 becomes flag-aware).
- `scan_step_command` (Word-emitting, ~630 lines): `lexer.rs:1953-2584`. STRUCTURAL-TOKEN reference (operators, redirects, fd-prefix, newlines, comments, heredoc openers, line-continuations).
- `scan_step_param_operand` (the operand ATOM scanner — the word/expansion-atom TEMPLATE): find via `grep -n "fn scan_step_param_operand" lexer.rs`. It already emits `Lit`/`DollarName`/`ParamOpen`/`CmdSubOpen`/`BeginBacktick`/`ArithOpen` with correct `quoted` bookkeeping and the `$`-classification; the command scanner reuses this emission logic MINUS the `}`/operand terminators, PLUS `Blank` splitting and structural tokens.
- `TokenKind` enum: `lexer.rs` ~line 388-430 (add `Blank` near the other atoms).
- Differential harness `old_seq`/`new_seq`/`diff_cmd`/`diff_unsupported`/`diff_err`: `parser.rs:1901-1917` (repoint `new_seq` at a live atom-Lexer).
- Command parser word-consumption sites: `parse_simple` word loop `parser.rs:1023-1027` (`TokenKind::Word(word) => all_words.push(word)`); `parse_for`, `parse_case_item`, redirect targets — find via `grep -n "TokenKind::Word" parser.rs`.
- Command-context `parse_word` boundary today: `parser.rs:28-35`.

## File Structure

- `crates/huck-syntax/src/lexer.rs` — the `command_atoms` flag + constructor plumbing; `TokenKind::Blank`; the `scan_step` selector; the new `scan_step_command_atoms` (built up across T2–T6); atom-stream unit tests.
- `crates/huck-syntax/src/parser.rs` — the command-context `parse_word` stop-set; repointed command-parser word sites; the repointed differential harness + broadened corpus.

---

### Task 1: Scaffolding — flag, `Blank`, selector, atom-scanner skeleton, repointed harness

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` (struct field, constructor, `TokenKind`, `scan_step` selector, skeleton `scan_step_command_atoms`)
- Modify: `crates/huck-syntax/src/parser.rs` (repoint `new_seq`, add a command-context `parse_word` entry, first scaffolding test)

**Interfaces:**
- Produces:
  - `Lexer` field `command_atoms: bool` (default `false`).
  - `pub fn new_live_atoms(input: &'a str, aliases: &HashMap<String,String>, opts: LexerOptions) -> Lexer<'a>` — like `new_live` but sets `command_atoms = true`.
  - `TokenKind::Blank` (unit variant).
  - `fn scan_step_command_atoms(&mut self) -> Result<Step, LexError>` (skeleton — real body lands T2+).
  - `parser.rs`: `new_seq` repointed to `new_live_atoms` + `parse_sequence`. (The command-context assembler `parse_word_command` is added in Task 2, not here — T1 only repoints the harness and proves it wires up on empty input.)

- [ ] **Step 1: Add the `command_atoms` field.** In `struct Lexer` (`lexer.rs:635`, after `retokenize_arith_as_cmdsub: bool,`) add:

```rust
    /// v247: when true, `Mode::Command` scans via `scan_step_command_atoms`
    /// (emits word-atoms + `Blank` + structural tokens) instead of the
    /// Word-emitting `scan_step_command`. Default false (production). Set only by
    /// the dormant atom path (differential harness + the eventual live flip).
    command_atoms: bool,
```

- [ ] **Step 2: Initialize it in the constructor.** In `Lexer::new`'s struct literal (the one with `retokenize_arith_as_cmdsub: false,`), add `command_atoms: false,`. (There is one struct-literal site in `new`; `from_tokens` at `lexer.rs:2637` builds its own — add `command_atoms: false,` there too. Grep `grep -n "retokenize_arith_as_cmdsub: false" lexer.rs` to find every struct-literal site and add the field beside it.)

- [ ] **Step 3: Add the `new_live_atoms` constructor.** After `new_live` (`lexer.rs:2781-2786`) add:

```rust
    /// v247: a live lexer whose `Mode::Command` emits atoms (dormant atom path).
    pub fn new_live_atoms(
        input: &'a str,
        aliases: &std::collections::HashMap<String, String>,
        opts: LexerOptions,
    ) -> Lexer<'a> {
        let mut lx = Lexer::new_live(input, aliases, opts);
        lx.command_atoms = true;
        lx
    }
```

- [ ] **Step 4: Add `TokenKind::Blank`.** In `pub enum TokenKind` (near the v246 arith atoms), add:

```rust
    Blank,   // v247: a run of unquoted inter-word whitespace in the atom-command stream (word boundary)
```

- [ ] **Step 5: Make the dispatch flag-aware.** In `scan_step` (`lexer.rs:731`) change:

```rust
            Mode::Command => self.scan_step_command(),
```

to:

```rust
            Mode::Command if self.command_atoms => self.scan_step_command_atoms(),
            Mode::Command => self.scan_step_command(),
```

- [ ] **Step 6: Add a skeleton `scan_step_command_atoms`.** Place it immediately after `scan_step_command` (after `lexer.rs:2584`). Skeleton delegates nothing yet — it only needs to handle EOF so the harness wires up; real word/structural handling lands T2+:

```rust
    /// v247 atom-emitting Command scanner (dormant). Built up across T2–T6:
    /// word-atoms + `Blank` splitting (T2), command-position expansions (T3),
    /// assignments (T4), redirects/operators/comments (T5), compounds (T6).
    /// Atom-native: at `$(`/`${`/`` ` ``/`$((` it emits the opener SIGNAL and the
    /// parser pushes the sub-mode — it never calls the fat scanners.
    fn scan_step_command_atoms(&mut self) -> Result<Step, LexError> {
        // T1 skeleton: only EOF handled; any real input errors loudly until T2.
        match self.cursor.peek() {
            None => self.finish(),
            Some(_) => Err(LexError::Unsupported),
        }
    }
```

If `LexError::Unsupported` does not exist, use the nearest existing generic `LexError` variant (grep `enum LexError`); the skeleton's error path is replaced in T2, so the exact variant is not load-bearing — pick one that exists and note it.

- [ ] **Step 7: Repoint `new_seq` at the atom path.** In `parser.rs` `mod tests`, change `new_seq` (`parser.rs:1905-1908`) to drive a LIVE atom-lexer (atoms are parser-driven, so we cannot pre-tokenize into `from_tokens`):

```rust
    fn new_seq(s: &str) -> Result<Option<Sequence>, ParseError> {
        let mut lx = Lexer::new_live_atoms(s, &Default::default(), LexerOptions::default());
        parse_sequence(&mut lx)
    }
```

`old_seq` is UNCHANGED (Word-Lexer + `command::parse`). `diff_cmd`/`diff_unsupported`/`diff_err` are unchanged in shape — they now compare the atom path vs the oracle.

- [ ] **Step 8: Add the scaffolding test.** In `parser.rs` `mod tests`:

```rust
    #[test]
    fn atoms_scaffolding_exists() {
        // The atom lexer + repointed harness wire up. Empty input parses to None
        // on both paths (EOF handled by the skeleton).
        assert_eq!(new_seq("").unwrap(), old_seq("").unwrap());
    }
```

- [ ] **Step 9: Run + confirm.** Run: `cargo test -p huck-syntax --jobs 1 --lib atoms_scaffolding_exists -- --test-threads 1` → PASS. Then `cargo build -p huck-syntax 2>&1 | grep -c warning` → `0`. (Do NOT run the full lib suite yet — many existing `diff_cmd` cases now exercise the T1 skeleton and will fail until T2; that is expected. If any pre-existing NON-`diff_cmd` test regresses, that is a real problem — investigate.)

- [ ] **Step 10: Commit.**

```bash
git add crates/huck-syntax/src/lexer.rs crates/huck-syntax/src/parser.rs
git commit -m "v247 T1: command_atoms flag + Blank atom + scan_step_command_atoms skeleton + repointed diff harness

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

**NOTE for T2+:** repointing `new_seq` in T1 means every existing `diff_cmd` corpus case now runs through the skeleton and fails until its construct is implemented. To keep each task's gate meaningful, T2–T6 each RUN their own scoped `diff_cmd` cases (by test-fn filter) and the FULL `diff_cmd` suite is expected green only after T6. Each task's "run" step names the specific tests to check.

---

### Task 2: Ordinary words + `Blank` word-splitting/gluing

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` (`scan_step_command_atoms`: literals, quoting, `Blank`)
- Modify: `crates/huck-syntax/src/parser.rs` (command-context `parse_word` stop-set; `parse_simple` assembles via it; skip `Blank`)

**Interfaces:**
- Consumes: T1 skeleton, flag, `Blank`.
- Produces: `fn parse_word_command(iter: &mut Lexer, quoted: bool) -> Result<Word, ParseError>` (command-context assembler) — or a `quoted`+stop-set param on the shared `parse_word`; later tasks call it for every command-position word.

- [ ] **Step 1: Failing tests — bare/multi-word + gluing/splitting.**

```rust
    #[test]
    fn atoms_plain_words() {
        diff_cmd("echo");
        diff_cmd("echo hi");
        diff_cmd("echo   hi    there");     // multiple blanks collapse
        diff_cmd("  echo hi  ");            // leading/trailing blanks
        diff_cmd("echo 'a b' \"c d\" e");   // quoted runs stay one word
        diff_cmd("echo a'b'c\"d\"");        // glued quotes = one word
        diff_cmd("echo a\\ b");             // escaped space = one word
        diff_cmd("echo $'x\\ty'");          // $'…' ANSI-C
    }
```

- [ ] **Step 2: Run, expect FAIL** (skeleton errors on real input). Run: `cargo test -p huck-syntax --jobs 1 --lib atoms_plain_words -- --test-threads 1`.

- [ ] **Step 3: Implement literal/quoting/`Blank` handling in `scan_step_command_atoms`.** Replace the skeleton body. Model the word-building on `scan_step_param_operand`'s literal + quoting emission (same `Lit { text, quoted }` accumulation, `'…'`/`"…"`/`$'…'`/backslash rules — read it and mirror), but: (a) there is no `}` operand terminator; (b) a run of unquoted spaces/tabs emits a single `TokenKind::Blank` (flush any pending literal first, then emit `Blank`); (c) at a metacharacter/operator/newline/EOF, flush the pending literal and return so the structural token is produced on the next call (structural tokens themselves are T5 — for T2, treat only whitespace + EOF as boundaries and defer operators by leaving them for a later task; scope the T2 corpus to inputs with no operators). Emit one atom per call (flush-first-then-signal pattern, exactly as the operand scanner and `scan_step_arith` do). Provide the literal/blank core:

```rust
    fn scan_step_command_atoms(&mut self) -> Result<Step, LexError> {
        // Skip a run of unquoted blanks → emit one Blank boundary token.
        if matches!(self.cursor.peek(), Some(' ') | Some('\t')) {
            let off = self.cursor.offset(); let l = self.cursor.line(); let c = self.cursor.column();
            while matches!(self.cursor.peek(), Some(' ') | Some('\t')) { self.cursor.next(); }
            self.history.push(Token::new(TokenKind::Blank, Span::new(off, l, c)));
            return Ok(Step::Produced);
        }
        match self.cursor.peek().copied() {
            None => self.finish(),
            // Quoting + literal word text — mirror scan_step_param_operand's
            // Lit accumulation ('…' / "…" / $'…' / backslash), emitting a single
            // Lit { text, quoted } atom for a maximal unquoted+quoted glued run,
            // stopping at a blank / operator / newline / EOF (not consuming it).
            Some(_) => self.scan_command_word_atom(),
        }
    }
```

Add a helper `scan_command_word_atom(&mut self) -> Result<Step, LexError>` that accumulates one word's `Lit` atom across glued quoted/unquoted segments (T3 extends it to break out on `$`/`` ` `` expansion openers; T5 stops it at operators). Keep the quoting rules byte-identical to the operand scanner (that is the source of the `quoted` flags the oracle expects).

- [ ] **Step 4: Add the command-context assembler in `parser.rs`.** Add `parse_word_command` that assembles atoms into one Word, stopping at `Blank`/`Op`/`Newline`/EOF without consuming (reuse the operand `parse_word`'s part-handling by extracting a shared inner loop with a stop predicate, OR write a focused command variant):

```rust
    fn parse_word_command(iter: &mut Lexer, quoted: bool) -> Result<Word, ParseError> {
        let mut parts = Vec::new();
        loop {
            match iter.peek_kind()? {
                None
                | Some(TokenKind::Blank)
                | Some(TokenKind::Newline)
                | Some(TokenKind::Op(_)) => break,
                Some(TokenKind::Lit { .. }) => {
                    if let Some(TokenKind::Lit { text, quoted: q }) = iter.next_kind()? {
                        parts.push(WordPart::Literal { text, quoted: q || quoted });
                    }
                }
                // T3 adds DollarName / ParamOpen / CmdSubOpen / BeginBacktick / ArithOpen arms here.
                _ => break,
            }
        }
        Ok(Word(parts))
    }
```

- [ ] **Step 5: Assemble command words in `parse_simple`.** In `parse_simple` (`parser.rs:984-1027`), before the token loop's `TokenKind::Word` arm, add `Blank`-skipping and word assembly. Replace the `let kind = iter.next_kind()?...; match kind { TokenKind::Word(word) => all_words.push(word), _ => Err }` tail with:

```rust
        // Skip inter-word blanks in the atom stream.
        if matches!(iter.peek_kind()?, Some(TokenKind::Blank)) { iter.next_kind()?; continue; }
        // A command word begins here — assemble it from atoms.
        if matches!(iter.peek_kind()?, Some(TokenKind::Lit { .. })) {
            all_words.push(parse_word_command(iter, false)?);
            continue;
        }
        // Legacy Word token (Word-mode path, still used by non-atom callers): keep.
        let kind = iter.next_kind()?.unwrap();
        match kind {
            TokenKind::Word(word) => all_words.push(word),
            _ => return Err(ParseError::UnsupportedCommand),
        }
```

(Keeping the legacy `TokenKind::Word` arm means `parse_simple` still works when driven by the Word-Lexer — which other non-atom callers and `old_seq` do NOT use for `parse_sequence`, but it keeps the function total. T3+ add the expansion-opener peeks alongside the `Lit` peek.)

- [ ] **Step 6: Run, expect PASS.** Run: `cargo test -p huck-syntax --jobs 1 --lib atoms_plain_words -- --test-threads 1`. Debug any AST mismatch against `old_seq` output (the assembled Word's part list / `quoted` flags must match the fat scanner's).

- [ ] **Step 7: Warnings + commit.** `cargo build -p huck-syntax 2>&1 | grep -c warning` → `0`. Commit:

```bash
git add -A
git commit -m "v247 T2: atom-command ordinary words + Blank splitting/gluing (literals, quoting)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Command-position expansions

**Files:** `lexer.rs` (`scan_command_word_atom`: emit expansion-opener atoms), `parser.rs` (`parse_word_command`: recurse into sub-parsers).

**Interfaces:** Consumes T2's `parse_word_command` + `scan_command_word_atom`. Produces command-position `$x`/`${…}`/`$(…)`/`` `…` ``/`$((…))`/`~` assembly.

- [ ] **Step 1: Failing tests.**

```rust
    #[test]
    fn atoms_expansions() {
        diff_cmd("echo $x");
        diff_cmd("echo ${x:-d}");
        diff_cmd("echo $(echo hi)");
        diff_cmd("echo `echo hi`");
        diff_cmd("echo $((1+2))");
        diff_cmd("echo $x$y \"$a ${b}\" pre$(c)post");
        diff_cmd("echo ~ ~root ~/x");
        diff_cmd("echo $? $@ $1");
    }
```

- [ ] **Step 2: Run, expect FAIL.** `cargo test -p huck-syntax --jobs 1 --lib atoms_expansions -- --test-threads 1`.

- [ ] **Step 3: Emit expansion-opener atoms in `scan_command_word_atom`.** Mirror `scan_step_param_operand`'s `$`-classification and backtick/tilde arms EXACTLY (they emit `DollarName` for `$name`/specials, `ParamOpen` for `${`, `CmdSubOpen` for `$(`, `ArithOpen` for `$((`, `BeginBacktick` for `` ` ``, tilde handling), with the flush-first-then-signal discipline. The ONLY differences from the operand scanner: no `}`/operand terminator; unquoted blank/operator ends the word instead. Reuse the v246 `$((`-vs-`$(` bounded peek. `quoted` on these atoms follows the surrounding quote context (command words are `quoted:false` at top level, `true` inside `"…"`).

- [ ] **Step 4: Recurse in `parse_word_command`.** Add the opener arms (peek-driven for `ParamOpen`; consume-then-dispatch for the zero-width `CmdSubOpen`/`BeginBacktick`/`ArithOpen` signals — exactly as `parse_arith_body`/the operand `parse_word` do), plus `DollarName` → `Var`/`AllArgs`/`LastStatus`, and tilde:

```rust
                Some(TokenKind::ParamOpen { .. }) => parts.push(parse_param_expansion(iter, quoted)?),
                Some(TokenKind::CmdSubOpen)    => { iter.next_kind()?; parts.push(parse_command_sub(iter, quoted)?); }
                Some(TokenKind::BeginBacktick) => { iter.next_kind()?; parts.push(parse_backtick_sub(iter, quoted)?); }
                Some(TokenKind::ArithOpen)     => { iter.next_kind()?; parts.push(parse_arith_expansion(iter, quoted)?); }
                Some(TokenKind::DollarName { .. }) => {
                    if let Some(TokenKind::DollarName { name, quoted: q }) = iter.next_kind()? {
                        let eff = q || quoted;
                        parts.push(match name.as_str() {
                            "@" => WordPart::AllArgs { quoted: eff, joined: false },
                            "*" => WordPart::AllArgs { quoted: eff, joined: true },
                            "?" => WordPart::LastStatus { quoted: eff },
                            _   => WordPart::Var { name, quoted: eff },
                        });
                    }
                }
```

For tilde, mirror whatever atom the operand scanner emits for `~` (grep the operand scanner for `Tilde`); if the operand path does not emit a tilde atom, emit `Tilde(TildeSpec)` directly from the command scanner and push it in the parser — match the oracle's `WordPart::Tilde` shape (verify via `diff_cmd`).

- [ ] **Step 5: Run, expect PASS; debug part-shape/`quoted` against `old_seq`.** `cargo test -p huck-syntax --jobs 1 --lib atoms_expansions -- --test-threads 1`.

- [ ] **Step 6: Warnings + commit.** `grep -c warning` → 0. Commit `v247 T3: command-position expansions ($x/${…}/$(…)/backtick/$((…))/tilde) via atoms`.

---

### Task 4: Scalar assignments

**Files:** `lexer.rs` (assignment-prefix atom emission), `parser.rs` (assembly + `try_split_assignment` unchanged).

**Interfaces:** Consumes T3. Produces `x=v`/`x+=v`/`a[i]=v` byte-identical assembly.

- [ ] **Step 1: Failing tests.**

```rust
    #[test]
    fn atoms_assignments() {
        diff_cmd("x=1");
        diff_cmd("x=1 y=2 cmd");
        diff_cmd("x+=abc");
        diff_cmd("a[0]=v");
        diff_cmd("a[$i]=v");
        diff_cmd("x=$y\"z\"");
        diff_cmd("PATH=/bin:/usr/bin cmd arg");
    }
```

- [ ] **Step 2: Run, expect FAIL.** `cargo test -p huck-syntax --jobs 1 --lib atoms_assignments -- --test-threads 1`.

- [ ] **Step 3: Emit the assignment-prefix atoms.** In the Word-mode path, an assignment word begins with a `Literal { text: "name=" }` (or an `AssignPrefix` WordPart for `name+=` / `name[i]=`). Read how `scan_step_command` / the fat scanner produce these (grep `AssignPrefix` in `lexer.rs`), then reproduce the SAME leading atom(s) in `scan_command_word_atom`: for a bare `name=` produce the leading `Lit { text: "name=", quoted:false }` then the value atoms; for `name+=` / `name[subscript]=` produce the `AssignPrefix` atom the parser's `try_split_assignment` expects. The downstream `try_split_assignment` (in `command.rs`, reused by `parser.rs`) is UNCHANGED — it consumes the assembled Word. So the task is purely: make `parse_word_command` assemble the SAME Word (leading `Lit`/`AssignPrefix` + value parts) the fat scanner builds.

- [ ] **Step 4: Ensure `parse_word_command` passes `AssignPrefix` through.** Add an arm so an `AssignPrefix` atom (if the lexer emits one) is pushed into the Word's parts unchanged (peek/consume as appropriate — match how `parse_word` handles it in operand context if at all; else add the arm). Verify the assembled Word equals the oracle's, then `try_split_assignment` (called later in `parse_simple`) produces identical `Assignment`s.

- [ ] **Step 5: Run, expect PASS; debug against `old_seq`.** `cargo test -p huck-syntax --jobs 1 --lib atoms_assignments -- --test-threads 1`.

- [ ] **Step 6: Warnings + commit.** Commit `v247 T4: scalar assignments (x=/x+=/a[i]=) via atoms`.

---

### Task 5: Redirects, operators, separators, comments, continuations

**Files:** `lexer.rs` (structural tokens in the atom scanner), `parser.rs` (already handles them — verify).

**Interfaces:** Consumes T2–T4. Produces the full non-compound line grammar on the atom path.

- [ ] **Step 1: Failing tests.**

```rust
    #[test]
    fn atoms_structure() {
        diff_cmd("a | b | c");
        diff_cmd("a && b || c");
        diff_cmd("a; b; c");
        diff_cmd("a &");
        diff_cmd("echo hi > out");
        diff_cmd("echo hi >> out 2>&1");
        diff_cmd("cat < in");
        diff_cmd("3< in 4> out cmd");
        diff_cmd("{fd}> out cmd");
        diff_cmd("echo a  # trailing comment");
        diff_cmd("echo a \\\n  b");           // line continuation
        diff_cmd("cmd 2>&1 >file");
    }
```

- [ ] **Step 2: Run, expect FAIL.** `cargo test -p huck-syntax --jobs 1 --lib atoms_structure -- --test-threads 1`.

- [ ] **Step 3: Emit structural tokens in `scan_step_command_atoms`.** For operators, redirect operators, fd-prefixes (`3>`, `{fd}>`), newlines, comments (`#…`), and line-continuations (`\<newline>`), emit the SAME `TokenKind` tokens `scan_step_command` emits today (`Op(Operator::…)`, `Newline`, `RedirFd`, etc.). Reuse `scan_step_command`'s structural-token code paths verbatim where possible (extract shared helpers if the code is identical, OR mirror the specific arms — `lexer.rs:1953-2584`). The word scanner (`scan_command_word_atom`) must STOP (flush pending `Lit`, return) when it reaches a metacharacter so the structural token is produced on the next call. Heredoc OPENERS (`<<DELIM`) are emitted as the opener token but the body stays deferred (the parser returns `UnsupportedCommand` for heredocs, as today) — do NOT collect heredoc bodies in v247.

- [ ] **Step 4: Confirm the parser already handles these.** `parse_simple`'s redirect handling (`next_is_redirect` + `parse_one_redirect`) and `parse_and_or`/`parse_pipeline` already consume `Op`/redirect/`Newline` tokens. Verify no change is needed beyond `Blank`-skipping (add `Blank` skips wherever the command parser loops over tokens between words/stages — grep `parse_pipeline`/`parse_and_or`/`parse_command` for token loops and skip `Blank`). Add `Blank`-skip where a stage/operator boundary is read.

- [ ] **Step 5: Run, expect PASS; debug against `old_seq`** (redirect source-order + fd-prefix Word shapes must match). `cargo test -p huck-syntax --jobs 1 --lib atoms_structure -- --test-threads 1`.

- [ ] **Step 6: Warnings + commit.** Commit `v247 T5: redirects/operators/separators/comments/continuations on the atom path`.

---

### Task 6: In-scope compounds on the atom path

**Files:** `parser.rs` (compound parsers assemble their words via `parse_word_command`; `Blank`-skip), `lexer.rs` (only if a compound needs a structural token not yet emitted).

**Interfaces:** Consumes T2–T5. Produces if/while/until/for-list/case/select/subshell/brace on the atom path.

- [ ] **Step 1: Failing tests.**

```rust
    #[test]
    fn atoms_compounds() {
        diff_cmd("if true; then echo a; fi");
        diff_cmd("if a; then b; elif c; then d; else e; fi");
        diff_cmd("while read x; do echo $x; done");
        diff_cmd("until false; do echo a; done");
        diff_cmd("for i in a b c; do echo $i; done");
        diff_cmd("for i in $list; do :; done");
        diff_cmd("case $x in a) echo a;; b|c) echo bc;; *) echo d;; esac");
        diff_cmd("select x in a b; do echo $x; break; done");
        diff_cmd("( cd /tmp && ls )");
        diff_cmd("{ echo a; echo b; }");
        diff_cmd("if true; then echo a; fi | wc -l");   // compound in a pipeline
    }
```

- [ ] **Step 2: Run, expect FAIL** (compound word sites still read `TokenKind::Word`). `cargo test -p huck-syntax --jobs 1 --lib atoms_compounds -- --test-threads 1`.

- [ ] **Step 3: Repoint compound word sites + keyword recognition.** In `parse_for` (loop var + `in`-list), `parse_case`/`parse_case_item` (subject + pattern words), and any other compound reading a `TokenKind::Word`, assemble via `parse_word_command` and skip `Blank`s. Keyword recognition: where the parser peeks a leading word to decide a compound (`parse_command`/`parse_compound_section`), it must now assemble the leading word (or peek its atoms) and treat it as a keyword ONLY if it is a single unquoted `Literal` matching a reserved word. Use mark/rewind if needed: mark, assemble the leading word, if it is a bare keyword `Literal` dispatch to the compound (re-driving from the mark or passing the assembled word), else treat as a simple-command word. Match `command.rs`'s keyword tables (`keyword_of_tok`) exactly. `[[` and `((` at command position assemble as ordinary/operator tokens and resolve to the current `UnsupportedCommand` deferral (no new grammar).

- [ ] **Step 4: Run, expect PASS; debug against `old_seq`.** `cargo test -p huck-syntax --jobs 1 --lib atoms_compounds -- --test-threads 1`.

- [ ] **Step 5: Warnings + commit.** Commit `v247 T6: in-scope compounds (if/while/until/for/case/select/subshell/brace) on the atom path`.

---

### Task 7: Broaden the differential corpus + deferred-parity + atom-stream shape

**Files:** `parser.rs` (corpus + `diff_unsupported`/`diff_err`), `lexer.rs` (atom-stream unit tests).

**Interfaces:** Consumes T2–T6. Produces the comprehensive gate + the deferred assertions.

- [ ] **Step 1: Adversarial word-splitting/gluing + error-parity + full pre-existing corpus.**

```rust
    #[test]
    fn atoms_adversarial() {
        for s in [
            "a\"b\"$c", "a\\ b", "x=$y\"z\"", "$a$b$c", "'a'\"b\"c$d",
            "  a   b  ", "a>b", "a>>b<c", "echo \"$(echo $x)\"", "echo ${a[$i]}",
        ] { diff_cmd(s); }
    }

    #[test]
    fn atoms_error_parity() {
        // In-scope malformed input: the atom path must return the SAME error as the oracle.
        for s in ["echo $(", "echo ${", "if true", "for", "case x in", "( a"] {
            assert_eq!(
                new_seq(s).map(|_| ()).map_err(|e| format!("{e:?}")),
                old_seq(s).map(|_| ()).map_err(|e| format!("{e:?}")),
                "error parity for {s:?}",
            );
        }
    }
```

- [ ] **Step 2: Deferred-construct parity.** Assert every deferred construct returns `UnsupportedCommand` on the atom path (proving deliberate deferral):

```rust
    #[test]
    fn atoms_deferred_unsupported() {
        for s in [
            "(( 1+2 ))", "for ((i=0;i<3;i++)); do :; done", "[[ a == b ]]",
            "cat <<EOF\nx\nEOF", "cat <<<word", "f() { :; }", "coproc x { :; }",
            "a=(1 2 3)",
        ] {
            assert!(matches!(new_seq(s), Err(ParseError::UnsupportedCommand)),
                "expected UnsupportedCommand on atom path for {s:?}, got {:?}", new_seq(s));
        }
    }
```

(If the oracle `old_seq` also errors on some of these with a DIFFERENT error, that is fine — the point is the atom path defers cleanly. Adjust the expected variant only if a construct is actually reachable and produces a different deferral in `parser.rs` today — match `parser.rs`'s existing `diff_unsupported` expectations.)

- [ ] **Step 3: Atom-stream shape unit tests (in `lexer.rs` `mod tests`).** Assert the raw atom sequence for representative inputs and that `Blank` never appears in the Word-mode stream. Add a helper mirroring `operand_atoms` that drives a `command_atoms` lexer:

```rust
    fn command_atoms_of(s: &str) -> Vec<TokenKind> {
        let mut lx = Lexer::new_live_atoms(s, &Default::default(), LexerOptions::default());
        let mut out = Vec::new();
        // Drive the FLAT stream; stop at hand-off signals (they need a parser mode push)
        // exactly like operand_atoms does, so a raw drive cannot spin.
        while let Some(t) = lx.next_token().unwrap() {
            let stop = matches!(t.kind,
                TokenKind::CmdSubOpen | TokenKind::BeginBacktick | TokenKind::ArithOpen | TokenKind::ParamOpen { .. });
            out.push(t.kind);
            if stop { break; }
        }
        out
    }

    #[test]
    fn command_atoms_stream_shape() {
        assert_eq!(
            command_atoms_of("echo hi"),
            vec![
                TokenKind::Lit { text: "echo".into(), quoted: false },
                TokenKind::Blank,
                TokenKind::Lit { text: "hi".into(), quoted: false },
            ],
        );
        // Blank never appears in the Word-mode (production) stream.
        let words = tokenize_with_opts("echo hi", LexerOptions::default()).unwrap();
        assert!(words.iter().all(|t| !matches!(t.kind, TokenKind::Blank)));
    }
```

- [ ] **Step 4: Run the FULL differential suite green.** Now every `diff_cmd` case (pre-existing + new) must pass on the atom path. Run: `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` → expect `0 failed`. Investigate and FIX any `diff_cmd` failure (a well-formed in-scope divergence is a bug in v247, not something to pin). If a genuine unavoidable divergence surfaces, pin it (`*_deferred` test + `docs/bash-divergences.md` `[deferred]` entry) and report it prominently — but the expectation is zero.

- [ ] **Step 5: Doctests + warnings.** `cargo test -p huck-syntax --jobs 1 --doc -- --test-threads 1` → `0 failed`. `cargo build -p huck-syntax 2>&1 | grep -c warning` → `0`.

- [ ] **Step 6: Commit.** Commit `v247 T7: broaden differential corpus + deferred-parity + atom-stream shape tests`.

---

## Self-Review checklist (run before the whole-branch review)

- Every in-scope spec construct has a `diff_cmd` case (plain words, quoting, all expansions, assignments, pipelines/and-or/separators, redirects+fd-prefix, comments/continuations, each compound, adversarial gluing/splitting). Every deferred construct has a `diff_unsupported`/`atoms_deferred_unsupported` case.
- Production untouched: `git diff main -- crates/huck-syntax/src/lexer.rs | grep -E '^\-' | grep -E 'fn scan_step_command\b|fn scan_dollar_expansion|fn scan_arith_body|fn scan_backtick_body'` returns nothing (no deletions in the production scanners); `command.rs` and `process_line` unchanged.
- `command_atoms` defaults `false`; `new_seq` uses `new_live_atoms`; `old_seq` unchanged.
- The atom scanner is atom-native: `grep` shows `scan_step_command_atoms`/`scan_command_word_atom` do NOT call `scan_dollar_expansion`/`scan_arith_body`/`scan_backtick_body`/`scan_braced_param_expansion`.
- Full `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` is `0 failed`; 0 warnings.
- All commits carry the trailer.
