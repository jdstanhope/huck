# v258 `$[ ]` legacy arithmetic expansion — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Port `$[expr]` (legacy arithmetic expansion, == `$((expr))`) onto the
dormant atom-command parser, byte-identical to the `command.rs` oracle.

**Architecture:** Extend `Mode::Arith` with a delimiter field (`Paren|Bracket`) — the
v256 `for_header` pattern. `$[` is the same arith body as `$((`, delimited by a
single `]` with bracket-depth instead of `))` with paren-depth (parens become
literal body chars). A new `LegacyArithOpen` signal (close reuses `ArithClose`) +
`parse_legacy_arith_expansion` (no bail/mark/rewind) + a `LegacyArithOpen` arm
parallel to every `ArithOpen` arm, which closes the accumulated `$[expr]`
carry-forwards.

**Tech Stack:** Rust; `huck-syntax` crate; differential testing (atom `new_seq` vs
oracle `old_seq`).

## Global Constraints

- **Dormant + differential.** `command_atoms` stays `false` at BOTH sites
  (lexer.rs:811/812, 4167/4183). Production uses the oracle; the atom path must be
  byte-identical.
- **`command.rs` EMPTY-diff.** The oracle already handles `$[` (`scan_legacy_arith_body`).
- **`$[expr]` AST target:** `WordPart::Arith { body, quoted }` — IDENTICAL to
  `$((expr))`.
- **THE RULE.** No forward scan for a delimiter; the lexer tracks a running bracket
  depth and emits atoms; the parser assembles. No `mark`/`rewind` (there is no bail
  path for `$[`).
- **`lexer.rs` diff** = the `delim` field + `ArithDelim` + `LegacyArithOpen` +
  `scan_step_arith` parametrization + the two `$[` dispatch arms. The ENTIRE existing
  arith suite (`$((`/`((`/`for ((`) is the regression net for `Paren` byte-unchanged.
- **Test command (box OOM-kills on parallel):**
  `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1`.
- **Build clean:** `cargo build -p huck-syntax` → 0 warnings.
- **Commit trailer:** `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

---

### Task 1: Lexer — `ArithDelim` + `delim` field + `LegacyArithOpen` + `scan_step_arith` parametrization + `$[` dispatch

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` (TokenKind enum ~428; `ArithDelim` +
  `Mode::Arith` ~643; the mode-dispatch ~957; `scan_step_arith` ~1957-2165; the two
  `$[`→`DeferredExpansion` arms at ~3132 and ~3838; and every `Mode::Arith { … }`
  construction/test site listed below)
- Modify: `crates/huck-syntax/src/parser.rs` (the 3 `Mode::Arith { … }` push sites at
  1277, 1330, 2918 — add `delim: ArithDelim::Paren`)
- Test: `crates/huck-syntax/src/lexer.rs` `mod tests` (a focused Bracket-mode scan)

**Interfaces:**
- Produces: `pub(crate) enum ArithDelim { Paren, Bracket }` (derive `Debug, Clone,
  Copy, PartialEq, Eq`), exported from `lexer` (add to the `use crate::lexer::{…}`
  in parser.rs).
- Produces: `Mode::Arith { paren_depth: u32, in_dquote: bool, body_started: bool, for_header: bool, delim: ArithDelim }`.
- Produces: `TokenKind::LegacyArithOpen`.

- [ ] **Step 1: Add the `ArithDelim` enum and `LegacyArithOpen` token**

In `crates/huck-syntax/src/lexer.rs`, near the arith `TokenKind` variants
(`ArithOpen` ~428, `ArithClose` ~429, `ArithSemi` ~431) add:

```rust
    LegacyArithOpen,  // v258: opening `$[` of a legacy `$[ … ]` arith expansion — dual role like ArithOpen (zero-width operand signal + real opener in Arith{delim:Bracket})
```

Then, near the `Mode` enum (the `Mode::Arith` variant is ~643) add the delimiter
enum just above the `Mode` enum:

```rust
/// v258: which bracket delimits an arith body. `Paren` = `$(( … ))` / `(( … ))`
/// (paren-depth, closes on `))`); `Bracket` = `$[ … ]` legacy arith (bracket-depth,
/// closes on a single `]`, parens are literal body chars).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArithDelim { Paren, Bracket }
```

- [ ] **Step 2: Add `delim` to `Mode::Arith`**

Change the `Mode::Arith` variant (~643) to:

```rust
    Arith { paren_depth: u32, in_dquote: bool, body_started: bool, for_header: bool, delim: ArithDelim }, // $(( … )) / (( … )) / for (( … )) / $[ … ]
```

- [ ] **Step 3: Thread `delim` through every `Mode::Arith` construction/read site**

Update EACH of these to include `delim` (all existing ones → `ArithDelim::Paren`;
byte-unchanged behavior). This is mechanical — mirror the v256 `for_header` fan-out.

In `crates/huck-syntax/src/lexer.rs`:
- The mode-dispatch (~957):
  ```rust
              Mode::Arith { paren_depth, in_dquote, body_started, for_header, delim } =>
                  self.scan_step_arith(paren_depth, in_dquote, body_started, for_header, delim),
  ```
- Test sites at ~7771, ~7772, ~7776, ~7786, ~7806, ~12986, ~12991, ~12992: add
  `, delim: ArithDelim::Paren` to each `Mode::Arith { … }` literal. (The `..`
  partial-pattern reads inside `scan_step_arith` at ~1969/1986 do NOT need changes.)

In `crates/huck-syntax/src/parser.rs`:
- 1277 (`parse_arith_expansion`), 1330 (`parse_arith_command`), 2918
  (`parse_arith_for_clause`): add `, delim: ArithDelim::Paren` to each
  `Mode::Arith { … }` literal, and add `ArithDelim` to the `use crate::lexer::{…}`
  import list.

Build after this step to confirm all sites are updated:
Run: `cargo build -p huck-syntax 2>&1 | tail -5`
Expected: compiles (0 errors). rust-analyzer may show phantom errors — trust cargo.

- [ ] **Step 4: Change the `scan_step_arith` signature + the `!body_started` opener**

Change the signature (~1960) to accept `delim`:

```rust
    fn scan_step_arith(&mut self, paren_depth: u32, _in_dquote: bool, body_started: bool, for_header: bool, delim: ArithDelim) -> Result<Step, LexError> {
```

Replace the `!body_started` block (~1961-1975, which currently consumes `$((` and
emits `ArithOpen`) with a delim-branched version:

```rust
        if !body_started {
            let off = self.cursor.offset();
            let l = self.cursor.line();
            let c = self.cursor.column();
            debug_assert_eq!(self.cursor.peek(), Some(&'$'), "scan_step_arith entry: expected `$` of `$((`/`$[`");
            match delim {
                ArithDelim::Paren => {
                    self.cursor.next(); // `$`
                    self.cursor.next(); // `(`
                    self.cursor.next(); // `(`
                    if let Some(Mode::Arith { body_started, .. }) = self.modes.last_mut() { *body_started = true; }
                    self.history.push(Token::new(TokenKind::ArithOpen, Span::new(off, l, c)));
                }
                ArithDelim::Bracket => {
                    self.cursor.next(); // `$`
                    self.cursor.next(); // `[`
                    if let Some(Mode::Arith { body_started, .. }) = self.modes.last_mut() { *body_started = true; }
                    self.history.push(Token::new(TokenKind::LegacyArithOpen, Span::new(off, l, c)));
                }
            }
            return Ok(Step::Produced);
        }
```

- [ ] **Step 5: Parametrize the delimiter arms in the body loop**

In `scan_step_arith`'s body loop, just before the `loop {` (after `let mut depth =
paren_depth;` and the `sync_depth!` macro ~1987), compute the delimiter chars:

```rust
        let (open_char, close_char) = match delim {
            ArithDelim::Paren => ('(', ')'),
            ArithDelim::Bracket => ('[', ']'),
        };
```

Replace the three hardcoded delimiter arms — `Some('(')` (~1998), `Some(')') if
depth > 0` (~2003), and `Some(')')` depth-0 (~2008) — with these guard-based arms
(the depth-0 close branches on `delim`):

```rust
                Some(oc) if oc == open_char => {
                    self.cursor.next();
                    text.push(oc);
                    depth += 1;
                }
                Some(cc) if cc == close_char && depth > 0 => {
                    self.cursor.next();
                    text.push(cc);
                    depth -= 1;
                }
                Some(cc) if cc == close_char => {
                    // depth == 0: flush any pending literal FIRST (emit the
                    // terminator/bail on the NEXT call), else classify now.
                    if !text.is_empty() {
                        sync_depth!();
                        self.history.push(Token::new(TokenKind::Lit { text, quoted: true }, Span::new(off, l, c)));
                        return Ok(Step::Produced);
                    }
                    let poff = self.cursor.offset();
                    let pl = self.cursor.line();
                    let pc = self.cursor.column();
                    match delim {
                        ArithDelim::Paren => {
                            if self.cursor.peek_nth(1) == Some(')') {
                                self.cursor.next(); // first `)`
                                self.cursor.next(); // second `)`
                                self.history.push(Token::new(TokenKind::ArithClose, Span::new(poff, pl, pc)));
                            } else {
                                // NOT a `))` close — the `$( (…) )` wrinkle. Do NOT
                                // consume; the parser rewinds via ArithBail.
                                self.history.push(Token::new(TokenKind::ArithBail, Span::new(poff, pl, pc)));
                            }
                        }
                        ArithDelim::Bracket => {
                            // `$[ … ]` closes on a single depth-0 `]` (no `]]` check,
                            // no bail — `$[` has no `$( (` wrinkle).
                            self.cursor.next(); // `]`
                            self.history.push(Token::new(TokenKind::ArithClose, Span::new(poff, pl, pc)));
                        }
                    }
                    return Ok(Step::Produced);
                }
```

The non-delimiter bracket/paren now falls through to the existing catch-all `Some(ch)
=> { self.cursor.next(); text.push(ch); }` (~2159) as a literal — `(`/`)` literal in
`Bracket`, `[`/`]` literal in `Paren` (the latter already worked). The `$`/`${`/`$(`
/backtick/special-param/`;`(for_header) arms are UNCHANGED.

- [ ] **Step 6: Emit `LegacyArithOpen` for `$[` at the two dollar-dispatch sites**

At `crates/huck-syntax/src/lexer.rs` ~3132 (the `quoted: true` dquote context) and
~3838 (the `quoted: false` context), the `Some('[')` arms currently push
`TokenKind::DeferredExpansion`. Change BOTH to push `LegacyArithOpen` (still
zero-width — cursor stays on `$`) and update the comment:

```rust
            // `$[expr]` legacy arith (v258) — zero-width `LegacyArithOpen` signal
            // (cursor stays on `$`); the parser pushes Mode::Arith{delim:Bracket},
            // whose first scan consumes `$[` and emits the real LegacyArithOpen.
            Some('[') => {
                self.history.push(Token::new(TokenKind::LegacyArithOpen, Span::new(off, l, c)));
            }
```

- [ ] **Step 7: Write a focused lexer test for the Bracket-mode scan**

Add to `crates/huck-syntax/src/lexer.rs` `mod tests`:

```rust
    #[test]
    fn arith_bracket_mode_scans_legacy_arith() {
        // A Mode::Arith{delim:Bracket} body: `$[a[0]+1]` → LegacyArithOpen, the
        // body Lit "a[0]+1" (inner [0] bracket-nested), then ArithClose.
        let mut lx = Lexer::new_live_atoms("$[a[0]+1]");
        lx.push_mode(Mode::Arith { paren_depth: 0, in_dquote: false, body_started: false, for_header: false, delim: ArithDelim::Bracket });
        let mut kinds = Vec::new();
        while let Some(k) = lx.next_kind().unwrap() {
            let stop = matches!(k, TokenKind::ArithClose);
            kinds.push(k);
            if stop { break; }
        }
        assert_eq!(kinds.first(), Some(&TokenKind::LegacyArithOpen));
        assert!(kinds.iter().any(|k| matches!(k, TokenKind::Lit { text, .. } if text == "a[0]+1")),
            "expected body Lit \"a[0]+1\", got {kinds:?}");
        assert_eq!(kinds.last(), Some(&TokenKind::ArithClose));
    }
```

(If `Lexer::new_live_atoms` is not the exact constructor used by other lexer tests
for atom-mode scanning, use whatever the sibling arith lexer tests near ~7771 use —
match the existing pattern.)

- [ ] **Step 8: Build + run the full suite (the `Paren` regression net)**

Run: `cargo build -p huck-syntax 2>&1 | tail -3 && cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1 2>&1 | tail -6`
Expected: 0 warnings; ALL tests pass — the entire existing arith suite
(`$((`/`((`/`for ((`) proves `Paren` behavior is byte-unchanged, and the new
`arith_bracket_mode_scans_legacy_arith` passes.

- [ ] **Step 9: Commit**

```bash
git add crates/huck-syntax/src/lexer.rs crates/huck-syntax/src/parser.rs
git commit -m "v258 T1: ArithDelim + Mode::Arith delim field + scan_step_arith parametrization + \$[ dispatch

Adds ArithDelim{Paren,Bracket} + a delim field on Mode::Arith (v256 for_header
pattern; all existing sites -> Paren, byte-unchanged), TokenKind::LegacyArithOpen,
scan_step_arith parametrized on delim (open/close char per delim; Bracket closes on
a single depth-0 ] via ArithClose, no bail), and the two \$[-dispatch arms emit
LegacyArithOpen instead of DeferredExpansion. The existing arith suite is the Paren
regression net. command_atoms stays false; command.rs untouched.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Parser — `parse_legacy_arith_expansion` + `LegacyArithOpen` arms + base corpus + deferral flips

**Files:**
- Modify: `crates/huck-syntax/src/parser.rs` — add `parse_legacy_arith_expansion`
  (next to `parse_arith_expansion` ~1276); add a `LegacyArithOpen` arm parallel to
  every `ArithOpen` arm (sites 84, 200, 320, 432, 1245, 1480, 1930, 2883); flip the
  `$[expr]` deferral tests (~4459).
- Test: `crates/huck-syntax/src/parser.rs` `mod tests`.

**Interfaces:**
- Consumes: `Mode::Arith{delim:ArithDelim::Bracket}`, `TokenKind::LegacyArithOpen`
  (T1); existing `parse_arith_body`, `WordPart::Arith`.
- Produces: `fn parse_legacy_arith_expansion(iter: &mut Lexer, quoted: bool) -> Result<WordPart, ParseError>`.

- [ ] **Step 1: Write the failing base corpus tests**

Add to `crates/huck-syntax/src/parser.rs` `mod tests`:

```rust
    #[test]
    fn atoms_legacy_arith_base() {
        diff_cmd("echo $[1+2]");          // == $((1+2))
        diff_cmd("echo pre$[1+2]post");
        diff_cmd("echo $[ x + 1 ]");
        diff_cmd("echo $[a[0]]");         // inner [0] bracket-nested → body "a[0]"
        diff_cmd("echo $[(1+2)*3]");      // parens are literal body chars
        diff_cmd("x=$[1+2]");             // assignment value
    }

    #[test]
    fn atoms_legacy_arith_embedded() {
        diff_cmd("echo $[$x+1]");
        diff_cmd("echo $[${a}+1]");
        diff_cmd("echo $[$(echo 1)+2]");
        diff_cmd("echo $[`echo 1`+2]");
        diff_cmd("echo $[$((1+2))+3]");   // nested $((
        diff_cmd("echo $[$[1+2]+3]");     // nested $[
        diff_cmd("echo \"$[1+2]\"");      // inside dquote → Quoted{Double,[Arith]}
        diff_cmd("echo \"pre$[1+2]post\"");
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p huck-syntax --jobs 1 --lib atoms_legacy_arith -- --test-threads 1 2>&1 | tail -15`
Expected: FAIL — the atom path currently defers `$[` (`Err(UnsupportedExpansion)`/
`UnsupportedCommand`) while the oracle returns a `WordPart::Arith`, so `diff_cmd`
panics on the mismatch.

- [ ] **Step 3: Add `parse_legacy_arith_expansion`**

In `crates/huck-syntax/src/parser.rs`, immediately after `parse_arith_expansion`
(~1300, after its closing brace) add:

```rust
/// v258: assemble a `WordPart::Arith` for a `$[ … ]` legacy arithmetic expansion
/// (bash treats `$[ expr ]` as exactly `$(( expr ))`). Mirrors
/// `parse_arith_expansion` but with `delim: Bracket` and WITHOUT the bail path:
/// `$[` closes on a single depth-0 `]` (`ArithClose`) — there is no `$( (` wrinkle,
/// so no `mark`/`rewind`. The mode's first scan consumes `$[` and emits
/// `LegacyArithOpen`; `parse_arith_body` assembles the body and returns `Closed` on
/// `ArithClose`.
pub(crate) fn parse_legacy_arith_expansion(iter: &mut Lexer, quoted: bool) -> Result<WordPart, ParseError> {
    iter.push_mode(Mode::Arith { paren_depth: 0, in_dquote: quoted, body_started: false, for_header: false, delim: ArithDelim::Bracket });
    let result = (|| -> Result<ArithBodyOutcome, ParseError> {
        match iter.next_kind()? {
            Some(TokenKind::LegacyArithOpen) => {}
            _ => return Err(ParseError::UnsupportedExpansion),
        }
        parse_arith_body(iter, quoted)
    })();
    iter.pop_mode();
    match result? {
        ArithBodyOutcome::Closed(body) => Ok(WordPart::Arith { body, quoted }),
        // `$[` has no bail path (single-`]` close, no `$( (` wrinkle); a Bail here
        // would mean the lexer emitted an ArithBail in Bracket mode, which it never
        // does. Treat defensively as an unsupported expansion.
        ArithBodyOutcome::Bail => Err(ParseError::UnsupportedExpansion),
    }
}
```

Add `ArithDelim` to the `use crate::lexer::{…}` import if Step T1 didn't already
(it should have). 

- [ ] **Step 4: Add a `LegacyArithOpen` arm parallel to every `ArithOpen` arm**

Add the following arms, each immediately after the corresponding `ArithOpen` arm,
mirroring its shape and `quoted` argument:

At ~84 (consuming `match iter.next_kind()? { … }` — the enclosing match already
consumed the signal, so NO extra `next_kind()`):
```rust
            TokenKind::LegacyArithOpen => {
                let a = parse_legacy_arith_expansion(iter, quoted)?;
                parts.push(a);
            }
```

At ~200 (peeking; `quoted`):
```rust
            Some(TokenKind::LegacyArithOpen) => {
                iter.next_kind()?;
                flush_lit(&mut acc, &mut parts);
                parts.push(parse_legacy_arith_expansion(iter, quoted)?);
            }
```

At ~320 (peeking; `false`):
```rust
            Some(TokenKind::LegacyArithOpen) => { iter.next_kind()?; flush_lit(&mut acc, &mut parts); parts.push(parse_legacy_arith_expansion(iter, false)?); }
```

At ~432 (peeking; `true`):
```rust
                Some(TokenKind::LegacyArithOpen) => {
                    iter.next_kind()?;
                    flush_lit(&mut acc, &mut parts);
                    parts.push(parse_legacy_arith_expansion(iter, true)?);
                }
```

At ~1245 (`parse_arith_body` nested; `true`; NO `flush_lit` — mirrors the `ArithOpen`
arm exactly):
```rust
            Some(TokenKind::LegacyArithOpen)   => { iter.next_kind()?; parts.push(parse_legacy_arith_expansion(iter, true)?); }
```

At ~1480 (`parse_heredoc_body`; `true`):
```rust
            Some(TokenKind::LegacyArithOpen) => {
                iter.next_kind()?;
                flush_lit(&mut acc, &mut parts);
                parts.push(parse_legacy_arith_expansion(iter, true)?);
            }
```

At ~1930 (the word-part-start token SET — add to the `matches!`/`|` list):
```rust
                    | TokenKind::ArithOpen
                    | TokenKind::LegacyArithOpen
```

At ~2883 (`parse_arith_for_body`; `true`):
```rust
            Some(TokenKind::LegacyArithOpen)  => { iter.next_kind()?; cur.push(parse_legacy_arith_expansion(iter, true)?); }
```

- [ ] **Step 5: Run the base + embedded corpus**

Run: `cargo test -p huck-syntax --jobs 1 --lib atoms_legacy_arith -- --test-threads 1 2>&1 | tail -12`
Expected: PASS — `atoms_legacy_arith_base` and `atoms_legacy_arith_embedded` green.
If a `diff_cmd` line fails, the atom AST differs from the oracle: investigate (do NOT
weaken the test). The oracle values were all probed at plan time and produce
`WordPart::Arith` identical to `$((`.

- [ ] **Step 6: Flip the `$[expr]` deferral tests**

At `crates/huck-syntax/src/parser.rs` ~4459, the deferral loop currently asserts
`echo $[1+2]` / `echo pre$[1+2]post` return `UnsupportedCommand`. Remove those two
`$[…]` entries from the deferral array (keeping any non-`$[` entries), and add above
the loop:

```rust
        // v258: `$[expr]` legacy arith is NO LONGER deferred (see atoms_legacy_arith_base).
        diff_cmd("echo $[1+2]");
        diff_cmd("echo pre$[1+2]post");
```

If the deferral array becomes empty after removing the `$[…]` entries, replace the
whole `for … { … }` loop with the two `diff_cmd` lines above. Also search for any
other test asserting `$[` defers (`grep -n 'echo \$\[' crates/huck-syntax/src/parser.rs`
and `grep -n 'DollarBracket\|\$\[expr\]' …`) and flip each to `diff_cmd`/verify.

- [ ] **Step 7: Run the full suite**

Run: `cargo build -p huck-syntax 2>&1 | tail -3 && cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1 2>&1 | tail -6`
Expected: 0 warnings; all tests pass.

- [ ] **Step 8: Commit**

```bash
git add crates/huck-syntax/src/parser.rs
git commit -m "v258 T2: parse_legacy_arith_expansion + LegacyArithOpen arms + base corpus

Adds parse_legacy_arith_expansion (mirrors parse_arith_expansion with delim:Bracket,
no bail/mark/rewind) and a LegacyArithOpen arm parallel to every ArithOpen arm
(command position, dquote, nested arith, heredoc, arith-for-header, word-part set) —
so \$[expr] produces WordPart::Arith byte-identical to \$((expr)) everywhere \$((
works. Flips the \$[expr] deferral tests. command_atoms stays false.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Carry-forward sites + edges + adversarial

Close the accumulated `$[expr]` carry-forwards and lock down the delimiter-protection
edges. Corpus-only unless a real bug is found.

**Files:**
- Test: `crates/huck-syntax/src/parser.rs` `mod tests`.

**Interfaces:**
- Consumes: everything from T1/T2. No production changes expected.

- [ ] **Step 1: Write the carry-forward-site corpus**

Add to `crates/huck-syntax/src/parser.rs` `mod tests`:

```rust
    #[test]
    fn atoms_legacy_arith_carryforward_sites() {
        // Heredoc body (v250 carry-forward) → Arith{quoted:true} + "\n"
        diff_cmd("cat <<E\n$[1+2]\nE\n");
        // Regex operand inside [[ … ]] (v254 carry-forward)
        diff_cmd("[[ x =~ $[1+2] ]]");
        // Array-literal value
        diff_cmd("a=($[1+2])");
        // case subject
        diff_cmd("case $[1+2] in a) :;; esac");
    }
```

- [ ] **Step 2: Run the carry-forward corpus**

Run: `cargo test -p huck-syntax --jobs 1 --lib atoms_legacy_arith_carryforward -- --test-threads 1 2>&1 | tail -10`
Expected: PASS — the `LegacyArithOpen` arms added in T2 cover all these sites (the
oracle values were probed at plan time). If any line fails, investigate as in T2
Step 5 (do not weaken).

- [ ] **Step 3: Pin the delimiter-protection edges**

The atom `Mode::Arith{Bracket}` treats quotes / `\` as literal (no sub-mode), so
`$[ "]" ]` closes early at the quoted `]` — a genuine divergence from the oracle
(which protects the quoted `]`). Unlike `$((` — where an early depth-0 `)`
`ArithBail`s and both paths fall to a command-sub-of-subshell identically — `$[` has
no bail, so this is a real pin. Determine the ACTUAL atom output first:

Run (throwaway probe): add a temporary test
```rust
    #[test]
    fn probe_edges() {
        eprintln!("QUOTE new={:?}\n      old={:?}", new_seq("echo $[ \"]\" ]"), std::panic::catch_unwind(|| old_seq("echo $[ \"]\" ]")));
        eprintln!("BSL   new={:?}\n      old={:?}", new_seq("echo $[ \\] ]"), std::panic::catch_unwind(|| old_seq("echo $[ \\] ]")));
    }
```
Run: `cargo test -p huck-syntax --jobs 1 --lib probe_edges -- --test-threads 1 --nocapture 2>&1 | grep -E "QUOTE|BSL|new=|old="`
Read the actual `new_seq` values, then DELETE the probe and replace it with a pinned
carry-forward test that asserts the real atom behavior and documents the divergence
from the oracle (the oracle produces `Arith{body:" ] "}` / `Arith{body:" \\] "}`):

```rust
    #[test]
    fn atoms_legacy_arith_quote_backslash_carryforward() {
        // v258 LIVE-FLIP CARRY-FORWARD: the atom Mode::Arith{Bracket} treats quotes
        // and `\` as LITERAL chars (no sub-mode, exactly like `$((`), so a `]`
        // inside `"…"` or after `\` closes the `$[ … ]` EARLY. The oracle's
        // scan_legacy_arith_body protects those spans. `$((` shows no such
        // divergence only because its early depth-0 `)` ArithBails to a
        // command-sub-of-subshell on BOTH paths; `$[` has no bail. Pathological
        // (quotes/backslash-escaped brackets in a legacy arith); dormant.
        // <ASSERT the ACTUAL new_seq value from the probe; it differs from
        //  old_seq's Arith{" ] "} / Arith{" \] "}.>
    }
```

(Fill the `<…>` with concrete assertions on the real `new_seq` output — e.g.
`assert!(matches!(new_seq("echo $[ \"]\" ]"), Err(_)));` if the atom errors, or an
`assert_eq!` on the exact `Ok` AST if it parses to something else. Whatever the atom
actually does, assert it and note it ≠ the oracle. Do NOT alter production code to
try to match the oracle here — quote/`\` protection is an inherited `Mode::Arith`
limitation, out of scope.)

- [ ] **Step 4: Note the unterminated case**

Add a comment-documented test that `$[1+2` (no close) errors on both paths (it is a
lex error, so `old_seq` panics and it is not `diff_err`-testable). Assert the atom
side is an error:

```rust
    #[test]
    fn atoms_legacy_arith_unterminated() {
        // `$[1+2` (no closing `]`) → lex error on both paths. `old_seq` panics
        // (`.expect("lex")`), so this is asserted on the atom side only: the atom
        // emits UnterminatedArith (the oracle emits UnterminatedLegacyArith — both
        // ParseError::Lex; dormant, error-kind difference only).
        assert!(new_seq("echo $[1+2").is_err(), "unterminated $[ must error");
    }
```

- [ ] **Step 5: Run the full suite + confirm command.rs empty-diff**

Run: `cargo build -p huck-syntax 2>&1 | tail -3 && cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1 2>&1 | tail -6`
Expected: 0 warnings; all tests pass.

Run: `git diff --stat main -- crates/huck-syntax/src/command.rs`
Expected: EMPTY (no output) — `command.rs` untouched. If not, revert the stray change.

- [ ] **Step 6: Commit**

```bash
git add crates/huck-syntax/src/parser.rs
git commit -m "v258 T3: \$[ ] carry-forward sites + delimiter-protection pins

Closes the accumulated \$[expr] carry-forwards (heredoc/regex/array/case) and pins
the quote/backslash delimiter-protection edges (\$[ \"]\" ] / \$[ \\] ] close early
on the atom path — inherited Mode::Arith literal-quote limitation; the oracle
protects those spans). Unterminated \$[ errors on both. command.rs EMPTY-diff;
command_atoms false.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review

**Spec coverage:**
- `WordPart::Arith` identical to `$((` → T2 `parse_legacy_arith_expansion` + base
  corpus. ✓
- `Mode::Arith` + `delim` (v256 pattern), all sites → `Paren` → T1 Steps 1-3. ✓
- `LegacyArithOpen` (close reuses `ArithClose`) + `scan_step_arith` parametrization →
  T1 Steps 1,4,5. ✓
- Two `$[`→`LegacyArithOpen` dispatch arms → T1 Step 6. ✓
- `LegacyArithOpen` arm parallel to every `ArithOpen` arm → T2 Step 4 (8 sites). ✓
- Carry-forward sites (heredoc/regex/array/case) → T3 Step 1. ✓
- Quote/backslash pins + unterminated → T3 Steps 3-4. ✓
- `command.rs` EMPTY-diff gate → T3 Step 5. ✓
- `Paren` regression net (existing arith suite) → T1 Step 8. ✓

**Placeholder scan:** none. The T3 Step 3 `<…>` is an explicit determine-then-assert
contingency (the exact atom divergence value is only knowable after T1/T2 are built),
with concrete guidance — mirrors the v257/v256 pin discipline, not a TODO.

**Type consistency:** `ArithDelim{Paren,Bracket}` (T1) used in `Mode::Arith` and
`parse_legacy_arith_expansion` (T2). `TokenKind::LegacyArithOpen` (T1) matched in
every parser arm (T2). `parse_legacy_arith_expansion(iter, quoted) -> Result<WordPart,
ParseError>` (T2) — signature consistent. `parse_arith_body` / `ArithBodyOutcome` /
`WordPart::Arith` reused unchanged. Names consistent across tasks.
