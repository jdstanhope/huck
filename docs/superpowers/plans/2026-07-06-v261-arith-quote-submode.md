# v261 Arith Quote Sub-mode (CF6+CF7) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a quote/backslash sub-mode to the shared `scan_step_arith` body loop so the dormant atom-command path matches the `command.rs` oracle for quoted/backslashed arithmetic bodies (CF6 quote-removal for `$((`/`((`/`for ((`, CF7 quote/backslash protection of `]` for `$[`) plus the bare-`$` part-split.

**Architecture:** The change is confined to `crates/huck-syntax/src/lexer.rs` (`Mode::Arith` gains an `in_squote` flag; `scan_step_arith` gains quote/backslash arms + a bare-`$` flush) and the differential corpus in `crates/huck-syntax/src/parser.rs`. `command.rs` is UNTOUCHED. `command_atoms` stays `false` — this is dormant/differential work verified via `new_seq` (atom) vs `old_seq` (oracle) full-AST equality.

**Tech Stack:** Rust, single crate `huck-syntax`.

## Global Constraints

- `command.rs` diff-vs-`main` = EMPTY. No changes to that file.
- `command_atoms` stays `false` at both constructor sites (lexer.rs:819, ~4261).
- Box is 1 core / 1.9 GB. The ONLY test command is
  `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1`
  (append a test name before `--` to run one). NEVER `--workspace`, NEVER
  multi-threaded — it OOM-kills the session.
- `cargo build -p huck-syntax` → 0 warnings. All `match` arms stay exhaustive
  (no `_ =>` added to `WordPart`/`Command`/quote logic).
- Every arith body part stays `quoted: true`; the whole `WordPart::Arith` stays
  `quoted: false`. The oracle hardcodes `quoted: true` for arith bodies — do not
  thread the outer `quoted` context into part flags.
- Commit trailer VERBATIM on every commit:
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- rust-analyzer PHANTOM diagnostics — trust `cargo`, not the editor.

---

### Task 1: Quote sub-mode machinery in `scan_step_arith`

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` — `Mode::Arith` enum variant (~650),
  the dispatch (~964), all `Mode::Arith { … }` full constructions, and
  `scan_step_arith` (~1990).
- Modify: `crates/huck-syntax/src/parser.rs` — the four `Mode::Arith { … }` push
  sites (1326, 1357, 1405, 3180) get `in_squote: false`; the two that seed
  `in_dquote: quoted` (1326, 1357) change to `in_dquote: false`.

**Interfaces:**
- Consumes: nothing new.
- Produces: `Mode::Arith { paren_depth, in_squote, in_dquote, body_started, for_header, delim }`
  (new `in_squote` field). `scan_step_arith(&mut self, paren_depth: u32,
  in_squote: bool, in_dquote: bool, body_started: bool, for_header: bool,
  delim: ArithDelim)` (was `_in_dquote`; now both quote flags are live).

**Design notes (read before editing):**
- `in_dquote` was DEAD in `scan_step_arith` (the `_in_dquote` param). It is
  repurposed as the *internal* double-quote-span state and must start `false`
  at every push site (a fresh arith body is outside any span). The outer
  `quoted` context is irrelevant to the body — parts are always `quoted: true`.
- To avoid adding a sync to every `return` site, the `'`/`"` toggles write the
  frame field IMMEDIATELY (like `body_started` is set), keeping the frame
  authoritative. `paren_depth` continues to sync via the existing `sync_depth!`
  (its invariant — text is always non-empty when depth changed — still holds).
- THE ASYMMETRY: Paren `(`/`)`/`;` fire regardless of quote state; Bracket
  `[`/`]` fire only when `!in_squote && !in_dquote`. Single-quote is handled
  first and suppresses everything (including `$`); double-quote keeps the shared
  `$`/backtick expansion and applies the `\`-escape table.

- [ ] **Step 1: Add `in_squote` to the `Mode::Arith` variant**

In `crates/huck-syntax/src/lexer.rs` at ~650, change:
```rust
    Arith { paren_depth: u32, in_dquote: bool, body_started: bool, for_header: bool, delim: ArithDelim }, // $(( … )) / (( … )) / for (( … )) / $[ … ]
```
to:
```rust
    Arith { paren_depth: u32, in_squote: bool, in_dquote: bool, body_started: bool, for_header: bool, delim: ArithDelim }, // $(( … )) / (( … )) / for (( … )) / $[ … ]
```

- [ ] **Step 2: Update the dispatch (~964)**

Change:
```rust
            Mode::Arith { paren_depth, in_dquote, body_started, for_header, delim } =>
                self.scan_step_arith(paren_depth, in_dquote, body_started, for_header, delim),
```
to:
```rust
            Mode::Arith { paren_depth, in_squote, in_dquote, body_started, for_header, delim } =>
                self.scan_step_arith(paren_depth, in_squote, in_dquote, body_started, for_header, delim),
```

- [ ] **Step 3: Add `in_squote: false` to every FULL `Mode::Arith` construction**

These are the sites that write all fields (patterns using `{ .., }` are
unaffected). For each, insert `in_squote: false,` right before `in_dquote`,
AND at the two parser.rs push sites that read `in_dquote: quoted`, change that
to `in_dquote: false`:

- `lexer.rs` test/construction sites: 7879, 7880, 7884, 7894, 7914, 7934,
  13114, 13119, 13120 — add `in_squote: false,`.
- `parser.rs:1326` — `push_mode(Mode::Arith { paren_depth: 0, in_squote: false, in_dquote: false, body_started: false, for_header: false, delim: ArithDelim::Paren })` (was `in_dquote: quoted`).
- `parser.rs:1357` — same shape with `delim: ArithDelim::Bracket` (was `in_dquote: quoted`).
- `parser.rs:1405` — add `in_squote: false,` (keep `in_dquote: false`).
- `parser.rs:3180` — add `in_squote: false,` (keep `in_dquote: false`, `for_header: true`).

Also update the doc-comment at `parser.rs:1241` and `lexer.rs:1986` to list the
new field (mechanical; keep them accurate).

VERIFY `quoted` is still used later in the functions at parser.rs:1326/1357
(the `WordPart::Arith { quoted }` result) so removing it from the field seed
does not create an unused-variable warning. It is used — do not remove the
binding.

- [ ] **Step 4: Update `scan_step_arith` signature (~1990)**

Change:
```rust
    fn scan_step_arith(&mut self, paren_depth: u32, _in_dquote: bool, body_started: bool, for_header: bool, delim: ArithDelim) -> Result<Step, LexError> {
```
to:
```rust
    fn scan_step_arith(&mut self, paren_depth: u32, in_squote: bool, in_dquote: bool, body_started: bool, for_header: bool, delim: ArithDelim) -> Result<Step, LexError> {
```

- [ ] **Step 5: Seed quote locals + a frame-write macro (in the body section)**

After the `let mut depth = paren_depth;` line and its `sync_depth!` macro
definition (the body-accumulation preamble, ~2020), add:
```rust
        let mut squote = in_squote;
        let mut dquote = in_dquote;
        // Write the current quote-span state back to the top Arith frame. Called
        // on every `'`/`"` toggle so the flag survives a `$`/backtick sub-parse
        // round-trip WITHOUT adding a sync to every `return` site (mirrors how
        // `body_started` is set directly on the frame).
        macro_rules! sync_quotes { () => {
            if let Some(Mode::Arith { in_squote, in_dquote, .. }) = self.modes.last_mut() {
                *in_squote = squote; *in_dquote = dquote;
            }
        }; }
```

- [ ] **Step 6: Rewrite the `None` (EOF) arm to error inside a quote**

Change the existing `None =>` arm to:
```rust
                None => {
                    if squote || dquote {
                        // Unterminated quote span inside the arith body. The oracle
                        // also errors here (scan_arith_body → UnterminatedArith /
                        // scan_legacy_arith_body/push_quoted_span → UnterminatedLegacyArith).
                        // Both paths error, so the input is not byte-comparable
                        // (`old_seq` panics on lex errors) — same non-diff pattern as
                        // prior iterations' unterminated cases.
                        return Err(LexError::UnterminatedArith);
                    }
                    if !text.is_empty() {
                        sync_depth!();
                        self.history.push(Token::new(TokenKind::Lit { text, quoted: true }, Span::new(off, l, c)));
                        return Ok(Step::Produced);
                    }
                    return Err(LexError::UnterminatedArith);
                }
```

- [ ] **Step 7: Add the single-quote-state arms FIRST (right after `None`)**

Insert immediately after the `None =>` arm, before the `open_char` arm:
```rust
                // Inside a single-quoted span: only `'` closes it; everything else
                // (incl. `$`, `` ` ``, `\`, delimiters) is literal, no expansion.
                Some('\'') if squote => {
                    self.cursor.next();
                    squote = false;
                    sync_quotes!();
                }
                Some(ch) if squote => {
                    self.cursor.next();
                    text.push(ch);
                }
                // Quote openers/closers (not in single-quote here). A `'` inside a
                // double-quote is literal; otherwise it OPENS a single-quote (drop
                // the quote char — bash quote-removal). `"` toggles the double-quote
                // span (drop the quote char). Both are DROPPED, never pushed.
                Some('\'') => {
                    self.cursor.next();
                    if dquote {
                        text.push('\'');
                    } else {
                        squote = true;
                        sync_quotes!();
                    }
                }
                Some('"') => {
                    self.cursor.next();
                    dquote = !dquote;
                    sync_quotes!();
                }
```

- [ ] **Step 8: Guard the delimiter arms with the quote asymmetry**

The three `close_char`/`open_char` arms and the `;` for-header arm currently
match unconditionally. Add `(matches!(delim, ArithDelim::Paren) || !dquote)` to
the three bracket/paren delimiter arms so a Bracket `[`/`]` inside a
double-quote is NOT a delimiter event (single-quote already handled above; Paren
stays quote-blind). Leave the `;` for-header arm as `for_header && depth == 0`
(quote-blind; `for_header` implies Paren).

Change:
```rust
                Some(oc) if oc == open_char => {
```
to:
```rust
                Some(oc) if oc == open_char && (matches!(delim, ArithDelim::Paren) || !dquote) => {
```
Change:
```rust
                Some(cc) if cc == close_char && depth > 0 => {
```
to:
```rust
                Some(cc) if cc == close_char && depth > 0 && (matches!(delim, ArithDelim::Paren) || !dquote) => {
```
Change:
```rust
                Some(cc) if cc == close_char => {
```
to:
```rust
                Some(cc) if cc == close_char && (matches!(delim, ArithDelim::Paren) || !dquote) => {
```

- [ ] **Step 9: Add the backslash arm (before the `;`/catch-all)**

Insert a `\` arm after the `$` block's closing (after the `Some('$') => { … }`
arm, before the `Some(';')` for-header arm). It branches on double-quote first,
then delim:
```rust
                Some('\\') => {
                    if dquote {
                        // Double-quote `\`-escape table (matches arith_string_to_word):
                        // `\` before `" \ $ ` `` drops the backslash and keeps the
                        // metachar; otherwise the `\` is literal and the next char is
                        // reprocessed normally.
                        self.cursor.next(); // consume `\`
                        match self.cursor.peek().copied() {
                            Some(n @ ('"' | '\\' | '$' | '`')) => { self.cursor.next(); text.push(n); }
                            _ => { text.push('\\'); }
                        }
                    } else {
                        match delim {
                            // `$[`: `\` protects the NEXT char (consume it raw, incl. a
                            // `]`/`[`) so it can't close the bracket — matches
                            // scan_legacy_arith_body. Both chars are retained literally.
                            ArithDelim::Bracket => {
                                self.cursor.next(); // `\`
                                text.push('\\');
                                if let Some(n) = self.cursor.peek().copied() {
                                    self.cursor.next();
                                    text.push(n);
                                }
                            }
                            // `$((`: `\` is a plain literal (scan_arith_body is
                            // quote/escape-blind; arith_string_to_word keeps it).
                            ArithDelim::Paren => {
                                self.cursor.next();
                                text.push('\\');
                            }
                        }
                    }
                }
```

- [ ] **Step 10: Change the bare-`$` fallthrough to a standalone `$` literal**

In the `$` classifier block, replace the final `_ =>` arm:
```rust
                        _ => {
                            // Bare `$` (e.g. before an operator) — literal.
                            self.cursor.next();
                            text.push('$');
                        }
```
with:
```rust
                        _ => {
                            // Bare `$` (not `${`/`$(`/`$((`/`$[`/`$name`/special/digit):
                            // the oracle (arith_string_to_word) flushes the pending
                            // literal and pushes `$` as its OWN Literal part. Match that
                            // structure so `$(( 1 $ 2 ))`/`$(( $'x' ))` are byte-identical.
                            if !text.is_empty() {
                                sync_depth!();
                                self.history.push(Token::new(TokenKind::Lit { text, quoted: true }, Span::new(off, l, c)));
                                return Ok(Step::Produced);
                            }
                            let so = self.cursor.offset(); let sl = self.cursor.line(); let sc = self.cursor.column();
                            self.cursor.next(); // `$`
                            self.history.push(Token::new(TokenKind::Lit { text: "$".into(), quoted: true }, Span::new(so, sl, sc)));
                            return Ok(Step::Produced);
                        }
```

- [ ] **Step 11: Build and run the full arith suite (non-regression gate)**

Run: `cargo build -p huck-syntax` → expect 0 warnings.
Run: `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1`

Expected: the vast majority pass. Any FAILING test is one that asserted the OLD
retained-quote arith body (that assertion WAS the CF6 divergence). For each such
failure, update the expected body to the STRIPPED form (drop the quote chars;
keep expansion parts). Do NOT weaken a test to pass — only correct expected
literals that encoded the pre-fix divergence. Re-run until green.

- [ ] **Step 12: Commit**

```bash
git add crates/huck-syntax/src/lexer.rs crates/huck-syntax/src/parser.rs
git commit -m "v261 T1: arith quote sub-mode in scan_step_arith (CF6+CF7 + bare-\$)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```
(Only the field-seed edits touch parser.rs here; the corpus is Task 2.)

---

### Task 2: Differential corpus + pin flip + edge probes

**Files:**
- Modify: `crates/huck-syntax/src/parser.rs` — add corpus tests in `mod tests`;
  rewrite the v258 pin `atoms_legacy_arith_quote_backslash_carryforward`
  (~5716).

**Interfaces:**
- Consumes: `diff_cmd(s)` (asserts `new_seq(s).unwrap() == old_seq(s).unwrap()`),
  `diff_err(s)` (both error). `old_seq` uses `.expect("lex")`, so a lex-error
  input PANICS — never pass one to `diff_cmd`/`old_seq`; guard exploratory
  probes with `catch_unwind`.
- Produces: nothing consumed downstream.

- [ ] **Step 1: Add the CF6 Paren quote-removal corpus (write, expect PASS after T1)**

Add to `crates/huck-syntax/src/parser.rs mod tests`:
```rust
    #[test]
    fn atoms_arith_paren_quote_removal() {
        // CF6: bash quote-removal in $((/((/for (( arith bodies — quotes are
        // dropped, single-quote suppresses `$`, double-quote keeps expansion.
        diff_cmd("echo $(( \"x\" ))");
        diff_cmd("echo $(( x=\"5\" ))");
        diff_cmd("echo $(( 1\"2\"3 ))");
        diff_cmd("echo $(( '$x' ))");        // single-quote → literal $x, no expand
        diff_cmd("echo $(( \"$x\" ))");      // double-quote → expands, quotes gone
        diff_cmd("echo $(( \"a\\\"b\" ))");  // dquote \-escape → a"b
        diff_cmd("echo $(( \"`echo 1`\" ))"); // backtick inside dquote
        diff_cmd("echo $(( \"${x:-]}\" ))"); // ${…} inside dquote
        diff_cmd("echo $(( \"a$(( 1 ))b\" ))"); // nested $(( )) inside dquote
        diff_cmd("echo $(( \"\" ))");        // empty dquote dropped
        diff_cmd("echo $(( \"a\"'b' ))");    // adjacent quotes concatenate
        diff_cmd("(( \"x\" ))");             // standalone (( )) command
    }
```

- [ ] **Step 2: Add the bare-`$` split corpus**

```rust
    #[test]
    fn atoms_arith_bare_dollar_split() {
        // Bare `$` (not an expansion start) is its own literal part in the oracle.
        diff_cmd("echo $(( 1 $ 2 ))");
        diff_cmd("echo $(( 1 $+ 2 ))");
        diff_cmd("echo $(( $'x' ))");   // no ANSI-C in arith: `$` literal + 'x' removed
    }
```

- [ ] **Step 3: Add the CF7 Bracket protection corpus**

```rust
    #[test]
    fn atoms_legacy_arith_quote_protection() {
        // CF7: `$[ … ]` — quotes and `\` protect the `]` AND are removed
        // (backslash retained literally).
        diff_cmd("echo $[ \"]\" ]");    // was UnterminatedQuote → Arith " ] "
        diff_cmd("echo $[']']");         // was UnterminatedQuote → Arith "]"
        diff_cmd("echo $[ \\] ]");      // was 2 args → Arith " \\] "
        diff_cmd("echo $[ \"$x\" ]");   // dquote expands, protects, removed
        diff_cmd("echo $[ ${x:-]} ]");  // ${…} already protects (regression)
        diff_cmd("echo $[ $(echo ]) ]"); // $(…) already protects (regression)
    }
```

- [ ] **Step 4: Rewrite the v258 pin to `diff_cmd`**

Replace the ENTIRE body of `atoms_legacy_arith_quote_backslash_carryforward`
(~5716) — the three `assert!`/`assert_eq!` cases — with a resolved-divergence
`diff_cmd` guard, keeping the test name:
```rust
    #[test]
    fn atoms_legacy_arith_quote_backslash_carryforward() {
        // v258 pinned a KNOWN gap: the atom Mode::Arith{Bracket} treated quotes
        // and `\` as literal, so a `]` inside `'…'`/`"…"` or after `\` closed
        // `$[ … ]` early. v261 RESOLVED it (the arith quote sub-mode protects
        // those spans and removes the quotes, matching scan_legacy_arith_body +
        // arith_string_to_word). Now a resolved-divergence regression guard.
        diff_cmd("echo $[ \"]\" ]");
        diff_cmd("echo $[ \\] ]");
        diff_cmd("echo $[']']");
    }
```

- [ ] **Step 5: Add the bail regression guards (must NOT change)**

```rust
    #[test]
    fn atoms_arith_quote_blind_bail_unchanged() {
        // Paren delimiters are quote-BLIND: a `)`/`(` inside a quote still drives
        // the depth/bail logic (scan_arith_body). These bail to a
        // cmdsub-of-subshell on BOTH paths — quote-removal must not protect them.
        diff_cmd("echo $(( \")\" ))");
        diff_cmd("echo $(( ')' ))");
        diff_cmd("echo $(( \"(\" ))");
    }
```

- [ ] **Step 6: Probe the for-header/quote interaction (pin only if it diverges)**

Add a throwaway probe test (guarded with `std::panic::catch_unwind` around both
`old_seq`/`new_seq`) for `for (( \"a;b\" ; ; )); do :; done` and
`for (( \"1\" ; \"2\" ; )); do :; done`. Run it.
- If both are byte-identical, add them to a `diff_cmd` test
  `atoms_arith_for_header_quote` and DELETE the throwaway.
- If one genuinely diverges (both `Ok`, ASTs differ), PIN it with an `assert_ne!`
  test + a comment noting it is the same class as the v256 for-header
  `;`-in-backtick/`${…}` carry-forward, and record it for the memory update.
  DELETE the throwaway probe either way (leave the tree clean).

- [ ] **Step 7: Run the full suite + gates**

Run: `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` → all green.
Run: `cargo build -p huck-syntax` → 0 warnings.
Run: `git diff --stat main -- crates/huck-syntax/src/command.rs` → EMPTY.

- [ ] **Step 8: Commit**

```bash
git add crates/huck-syntax/src/parser.rs
git commit -m "v261 T2: arith quote/bare-\$ differential corpus + flip v258 pin

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review

- **Spec coverage:** CF6 (Paren quote-removal, sq-suppress-`$`, dq-escape/expand)
  → T1 Steps 7-9 + T2 Step 1. CF7 (Bracket protection) → T1 Steps 8-9 + T2
  Step 3. Bare-`$` → T1 Step 10 + T2 Step 2. Pin flip → T2 Step 4. Bail
  regression guards → T2 Step 5. For-header edge → T2 Step 6. Asymmetry →
  T1 Step 8. ✓
- **Type consistency:** `Mode::Arith` field order
  `{ paren_depth, in_squote, in_dquote, body_started, for_header, delim }` is used
  identically in the enum def (Step 1), dispatch (Step 2), constructions (Step 3),
  and signature (Step 4). `scan_step_arith` arg order matches. ✓
- **Placeholder scan:** none. All code is verbatim. The only discovery step is
  T1 Step 11 (which existing arith tests asserted retained quotes) and T2 Step 6
  (for-header probe) — both have explicit instructions. ✓
