# `[[ … =~ REGEX ]]` regex-operand lexing (M-100) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Lex the right-hand operand of `=~` inside `[[ … ]]` as a single literal regex `Word` (parens/`|`/`((` literal, `$var`/quotes/`\`-escapes intact), so real regexes like `[[ $option =~ (\[((no|dont)-?)\]). ]]` parse and match.

**Architecture:** Lexer-only. `tokenize_core` gains two state fields (`dbracket_depth`, `expect_regex`) maintained at each word emit; when `expect_regex` is set, a new `scan_regex_operand` reads the operand as one `Word` (modeled on the existing `scan_extglob_group`). The word flows into the unchanged `TestExpr::Regex { pattern: Word }` — no parser or evaluator change.

**Tech Stack:** Rust. `src/lexer.rs` only (+ new test files). Tests: `cargo test --bin huck`, `cargo test --test <name>`, `bash tests/scripts/<name>_diff_check.sh`.

**Spec:** `docs/superpowers/specs/2026-06-07-dbracket-regex-operand-design.md`. Read it first.

**Commit trailer (MANDATORY, canonical — every commit):**
```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Background the implementer needs (verified anchors)

- The lexer has NO `[[ ]]` mode; `[[`/`]]`/`=~` are ordinary `Token::Word`s. `(`/`((` are turned into `Op(LParen)` / `Token::ArithBlock` by the `'('` arm (`src/lexer.rs:549`), purely by adjacency — this is what eats the regex parens.
- Words are emitted at **three** sites in `tokenize_core` via `emit_word_with_braces(&mut tokens, parts)?` followed (since v104) by `for _ in 0..n { offsets.push(token_start); }`. Find them: `grep -n 'emit_word_with_braces(&mut tokens' src/lexer.rs` (whitespace-boundary flush; the operator-arm pre-flush; the end-of-input flush).
- `read_dollar_expansion(chars, parts, quoted)`, `scan_backtick_substitution(chars)`, `flush_literal(parts, current, quoted)` exist and are reused by `scan_extglob_group` (`src/lexer.rs:842`) — **use `scan_extglob_group` as your template** for `scan_regex_operand`.
- `WordPart::Literal { text, quoted }`, `WordPart::CommandSub { sequence, quoted }`; `Word(pub Vec<WordPart>)`; `Token::Word(Word)`.
- `=~` parse: `parse_test_atom` `=~` arm → `next_test_word` (`src/command.rs:2018`, rejects `Op`). With the operand now a `Word`, this works unchanged.

---

## Task 1: Lexer state + `scan_regex_operand`

**Files:** Modify `src/lexer.rs` only.

- [ ] **Step 1: Write failing lexer unit tests**

Add to the `#[cfg(test)] mod tests` in `src/lexer.rs` (use the existing `tokenize`/`tokenize_with_opts` test helpers and the `Token`/`WordPart`/`Operator` imports already in scope there):

```rust
// Helper: is this token a single unquoted-literal Word with the given text?
fn word_text(t: &Token) -> Option<String> {
    if let Token::Word(Word(parts)) = t {
        if parts.len() == 1 {
            if let WordPart::Literal { text, quoted: false } = &parts[0] {
                return Some(text.clone());
            }
        }
    }
    None
}

#[test]
fn dbracket_regex_paren_operand_is_one_word() {
    // `[[ x =~ (a) ]]` -> Word([[) Word(x) Word(=~) Word((a)) Word(]])
    let toks = tokenize("[[ x =~ (a) ]]").unwrap();
    let texts: Vec<_> = toks.iter().filter_map(word_text).collect();
    assert_eq!(texts, vec!["[[", "x", "=~", "(a)", "]]"]);
    // No LParen/ArithBlock leaked into the stream.
    assert!(!toks.iter().any(|t| matches!(t, Token::Op(Operator::LParen) | Token::ArithBlock(_))));
}

#[test]
fn dbracket_regex_double_paren_not_arithblock() {
    let toks = tokenize("[[ ab =~ ((a)) ]]").unwrap();
    let texts: Vec<_> = toks.iter().filter_map(word_text).collect();
    assert_eq!(texts, vec!["[[", "ab", "=~", "((a))", "]]"]);
    assert!(!toks.iter().any(|t| matches!(t, Token::ArithBlock(_))));
}

#[test]
fn dbracket_regex_line847_shape() {
    let toks = tokenize(r"[[ $option =~ (\[((no|dont)-?)\]). ]]").unwrap();
    let texts: Vec<_> = toks.iter().filter_map(word_text).collect();
    // the regex operand is one word (its leading `(`):
    assert!(texts.iter().any(|t| t.starts_with("(\\[")));
    assert!(texts.contains(&"]]".to_string()));
    assert!(!toks.iter().any(|t| matches!(t, Token::ArithBlock(_))));
}

#[test]
fn dbracket_regex_space_inside_parens_kept() {
    // depth>0 whitespace is part of the operand: `(a b)` is ONE word.
    let toks = tokenize("[[ x =~ (a b) ]]").unwrap();
    let texts: Vec<_> = toks.iter().filter_map(word_text).collect();
    assert_eq!(texts, vec!["[[", "x", "=~", "(a b)", "]]"]);
}

#[test]
fn grouping_paren_outside_regex_still_op() {
    // `(` not after `=~` is still grouping (Op), NOT a regex operand.
    let toks = tokenize("[[ ( -n a ) ]]").unwrap();
    assert!(toks.iter().any(|t| matches!(t, Token::Op(Operator::LParen))));
    assert!(toks.iter().any(|t| matches!(t, Token::Op(Operator::RParen))));
}

#[test]
fn arith_block_outside_dbracket_unchanged() {
    // `(( ))` outside [[ ]] is still an ArithBlock.
    let toks = tokenize("(( 1 + 1 ))").unwrap();
    assert!(toks.iter().any(|t| matches!(t, Token::ArithBlock(_))));
}

#[test]
fn quoted_dbracket_word_does_not_change_depth() {
    // `'[['` is quoted -> NOT the keyword -> `=~` after it is NOT a regex trigger.
    let toks = tokenize("[[ '[[' = x ]]").unwrap();
    // `=` (not `=~`) anyway; just assert no panic and `]]` present.
    assert!(toks.iter().filter_map(word_text).any(|t| t == "]]"));
}
```

Run: `cargo test --bin huck dbracket_regex 2>&1 | tail` → FAIL (operand still split into `Op`/`ArithBlock`).

- [ ] **Step 2: Add the `single_unquoted_literal` helper**

Add a free function in `src/lexer.rs` (near `scan_extglob_group`):

```rust
/// `Some(text)` when `parts` is exactly one unquoted `Literal` (the form that
/// can be a keyword like `[[` / `]]` / `=~`); `None` otherwise. Mirrors how the
/// parser's `keyword_of` only treats a single unquoted literal as a keyword.
fn single_unquoted_literal(parts: &[WordPart]) -> Option<&str> {
    match parts {
        [WordPart::Literal { text, quoted: false }] => Some(text.as_str()),
        _ => None,
    }
}
```

- [ ] **Step 3: Add the lexer state + maintenance at each word emit**

In `tokenize_core`, declare next to `tokens`/`offsets`:
```rust
let mut dbracket_depth: u32 = 0;
let mut expect_regex = false;
```

At **each** of the three `emit_word_with_braces(&mut tokens, …)` sites, capture the
keyword classification from `parts` BEFORE the `std::mem::take`, and update the
state AFTER the emit. Concretely, each site becomes (the `flush_literal` line, if
present at that site, stays before this):

```rust
let kw = single_unquoted_literal(&parts).map(str::to_owned);
let n = emit_word_with_braces(&mut tokens, std::mem::take(&mut parts))?;
for _ in 0..n { offsets.push(token_start); }
match kw.as_deref() {
    Some("[[") => dbracket_depth += 1,
    Some("]]") => dbracket_depth = dbracket_depth.saturating_sub(1),
    Some("=~") if dbracket_depth > 0 => expect_regex = true,
    _ => {}
}
```

To avoid repeating this block three times, factor it into a small local closure or
helper, e.g.:
```rust
// Note: this updates dbracket_depth/expect_regex from the word's pre-emit parts.
let mut track_kw = |kw: Option<String>, depth: &mut u32, expect: &mut bool| {
    match kw.as_deref() {
        Some("[[") => *depth += 1,
        Some("]]") => *depth = depth.saturating_sub(1),
        Some("=~") if *depth > 0 => *expect = true,
        _ => {}
    }
};
```
…and call it after each emit with the captured `kw`. (A closure can't borrow
`tokens`/`offsets`, so pass `&mut dbracket_depth, &mut expect_regex` explicitly, or
just inline the `match` at the three sites — either is fine; keep it DRY.)

Do NOT run this tracking for the regex-operand emit in Step 5 (a regex operand is
not a `[[`/`]]`/`=~` keyword).

- [ ] **Step 4: Write `scan_regex_operand`**

Model on `scan_extglob_group` (`src/lexer.rs:842`). It reads the operand as a
`Word`, tracking paren depth, terminating at depth-0 unquoted whitespace (which it
must NOT consume — the main loop's whitespace handling expects to see it):

```rust
/// Scan the right-hand operand of `=~` inside `[[ … ]]` as a single regex word.
/// `(`/`)`/`|`/`((` are literal; paren depth keeps unquoted whitespace part of the
/// operand while > 0. `$…`/`` `…` ``/quotes/`\` behave as in a normal word. No
/// brace expansion, no extglob. The cursor is positioned at the first operand
/// char; on return it sits just before the terminating depth-0 whitespace (or at
/// EOF). The leading whitespace before the operand has already been consumed by
/// the main loop.
fn scan_regex_operand(chars: &mut CharCursor<'_>) -> Result<Vec<WordPart>, LexError> {
    let mut parts: Vec<WordPart> = Vec::new();
    let mut lit = String::new();
    let mut depth: u32 = 0;

    fn flush(lit: &mut String, parts: &mut Vec<WordPart>) {
        if !lit.is_empty() {
            parts.push(WordPart::Literal { text: std::mem::take(lit), quoted: false });
        }
    }

    loop {
        let c = match chars.peek() {
            None => break, // EOF ends the operand (see unterminated note below)
            Some(&c) => c,
        };

        // Depth-0 unquoted whitespace terminates the operand WITHOUT consuming it.
        if depth == 0 && c.is_whitespace() {
            break;
        }

        chars.next(); // consume c
        match c {
            '$' => {
                flush(&mut lit, &mut parts);
                read_dollar_expansion(chars, &mut parts, false)?;
            }
            '`' => {
                flush(&mut lit, &mut parts);
                let sequence = scan_backtick_substitution(chars)?;
                parts.push(WordPart::CommandSub { sequence, quoted: false });
            }
            '\'' => {
                flush(&mut lit, &mut parts);
                let mut inner = String::new();
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(ch) => inner.push(ch),
                        None => return Err(LexError::UnterminatedQuote),
                    }
                }
                parts.push(WordPart::Literal { text: inner, quoted: true });
            }
            '"' => {
                flush(&mut lit, &mut parts);
                let mut q = String::new();
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(esc @ ('"' | '\\' | '$' | '`')) => q.push(esc),
                            Some('\n') => {}
                            Some(other) => { q.push('\\'); q.push(other); }
                            None => return Err(LexError::UnterminatedQuote),
                        },
                        Some('$') => {
                            flush_literal(&mut parts, &mut q, true);
                            read_dollar_expansion(chars, &mut parts, true)?;
                        }
                        Some('`') => {
                            flush_literal(&mut parts, &mut q, true);
                            let sequence = scan_backtick_substitution(chars)?;
                            parts.push(WordPart::CommandSub { sequence, quoted: true });
                        }
                        Some(ch) => q.push(ch),
                        None => return Err(LexError::UnterminatedQuote),
                    }
                }
                flush_literal(&mut parts, &mut q, true);
            }
            '\\' => {
                match chars.next() {
                    Some('\n') => {} // line continuation: drop backslash-newline
                    Some(next) => { lit.push('\\'); lit.push(next); }
                    None => lit.push('\\'),
                }
            }
            '(' => { lit.push('('); depth += 1; }
            ')' => { lit.push(')'); depth = depth.saturating_sub(1); }
            other => lit.push(other), // includes | < > ; & and depth>0 whitespace
        }
    }
    flush(&mut lit, &mut parts);
    Ok(parts)
}
```

Notes:
- A depth-0 newline is whitespace → terminates the operand (leaving the newline for
  the main loop → the following `]]` works, incl. v87 multi-line). A newline at
  depth > 0 is consumed by the `other => lit.push(other)` arm (whitespace included
  while inside parens), so `=~ (a\nb)` keeps reading — bash-faithful.
- EOF mid-operand returns whatever parts were read (the `break` on `None`). The
  parser then sees an unterminated `[[ ]]` (no `]]`) and raises
  `UnterminatedDoubleBracket`, so continuation still works. (An unterminated quote
  inside the operand returns `LexError::UnterminatedQuote` → classify maps it to
  incomplete, same as today.) Verify in Task 2.

- [ ] **Step 5: Branch to `scan_regex_operand` at token start**

In the main loop, after the whitespace-skipping/word-boundary handling and BEFORE
the normal `match c { … }` char dispatch, add the regex-operand branch. The exact
insertion point is where the loop has just determined a new non-whitespace token is
starting (where `token_start`/`c_off` is set and `has_token` would transition).
Insert:

```rust
if expect_regex {
    expect_regex = false;
    // A regex operand begins here. `token_start`/`c_off` is the operand's first
    // byte. Do NOT consume the first char yet — scan_regex_operand peeks.
    let parts = scan_regex_operand(&mut chars)?;
    tokens.push(Token::Word(Word(parts)));
    offsets.push(token_start);     // (or c_off — whichever marks this token's start)
    has_token = false;
    in_assignment_value = false;
    continue; // skip the normal dispatch for this char
}
```

IMPORTANT: this branch must run at the point where the main loop has identified the
START of a token but has NOT yet consumed/dispatched its first char (so
`scan_regex_operand` sees the full operand from its first char). Study how the main
loop reads `let c_off = chars.offset(); let c = chars.next();` and where `has_token`
flips — you likely need to insert the check right after the whitespace branch and
before `let c = chars.next()` for the token's first char, using `chars.peek()` to
know a non-whitespace char is present. Adjust so `token_start` is the operand's
first byte (`chars.offset()` just before scanning). If the structure makes
"before consuming the first char" awkward, an alternative is to handle it inside the
existing dispatch: when `expect_regex` is set, route the current char `c` into a
variant of `scan_regex_operand` that takes the already-read first char. Pick
whichever keeps the offset correct and the existing loop intact; the unit tests
pin the behavior.

- [ ] **Step 6: Run tests + clippy**

- `cargo test --bin huck dbracket_regex 2>&1 | tail` → the Step-1 tests pass.
- `cargo test --bin huck 2>&1 | tail -15` → ALL lexer/unit tests pass (no regression to `(( ))`, `$(( ))`, extglob, normal `[[ ]]`).
- `cargo clippy --bin huck 2>&1 | tail -3` → clean (let-chains if suggested).

- [ ] **Step 7: Commit**

```bash
git add src/lexer.rs
git commit -m "feat(lexer): lex =~ regex operand inside [[ ]] as a literal word (M-100)

Track [[ ]] depth + an expect-regex flag; after =~ inside [[ ]], scan the operand
as one Word via scan_regex_operand (parens/|/(( literal, naive paren-depth keeps
depth>0 whitespace, \$-expansion/quotes/escapes intact, no brace/extglob). Fixes
the lexer grabbing a regex (( as an arith block. Parser/evaluator unchanged.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Integration tests + bash_completion smoke

**Files:** Create `tests/dbracket_regex_integration.rs`.

- [ ] **Step 1: Write match-semantics tests (vs bash)**

Copy the `run(script) -> (stdout, stderr, code)` helper from `tests/set_x_integration.rs`. Then assert huck's match results equal bash's:

```rust
#[test]
fn space_inside_parens_matches() {
    assert_eq!(run("[[ \"a b\" =~ (a b) ]] && echo M || echo N\n").0, "M\n");
    assert_eq!(run("[[ ab =~ (a b) ]] && echo M || echo N\n").0, "N\n");
}

#[test]
fn line847_shape_parses_and_matches() {
    assert_eq!(run("[[ \"[no-]\" =~ (\\[((no|dont)-?)\\]). ]] && echo M || echo N\n").0, "M\n");
}

#[test]
fn anchored_groups() {
    assert_eq!(run("c=foo=bar; [[ $c =~ ^([A-Za-z_][A-Za-z0-9_]*)=(.*)$ ]] && echo M || echo N\n").0, "M\n");
}

#[test]
fn bracket_with_double_close_inside() {
    // `(-[^]]+)` contains `]]` that must NOT end the test.
    assert_eq!(run("[[ \"-abc\" =~ (-[^]]+) ]] && echo M || echo N\n").0, "M\n");
}

#[test]
fn var_interpolation_in_operand() {
    assert_eq!(run("re='(a|b)'; [[ a =~ $re ]] && echo M || echo N\n").0, "M\n");
}

#[test]
fn alternation_operand() {
    assert_eq!(run("[[ /etc =~ ^\\~.*|^\\/.* ]] && echo M || echo N\n").0, "M\n");
}

#[test]
fn grouping_not_regex_still_works() {
    // `( … )` grouping (no =~) unaffected.
    assert_eq!(run("[[ -n a && ( -z \"\" || -n b ) ]] && echo M || echo N\n").0, "M\n");
}

#[test]
fn multiline_dbracket_regex() {
    assert_eq!(run("[[ ab =~ (a)(b)\n]] && echo M || echo N\n").0, "M\n");
}
```
For each, confirm the expected string by running the same fragment under `bash` first (the comments above are bash-verified shapes; re-verify any you're unsure of). Run: `cargo test --test dbracket_regex_integration 2>&1 | tail -20` → FAIL before Task 1 is built / PASS after.

- [ ] **Step 2: Build + run**

`cargo build 2>&1 | tail -2` then `cargo test --test dbracket_regex_integration 2>&1 | tail -20` → all pass.

- [ ] **Step 3: bash_completion smoke (not a committed test)**

```bash
printf 'source /usr/share/bash-completion/bash_completion\necho HUCK_END\n' > /tmp/bc.sh
./target/debug/huck /tmp/bc.sh 2>&1 | grep -nE "line 847|unterminated '\\(\\(|missing operand" | head
```
Expected: NO `line 847` / `unterminated '((' ` / `missing operand` errors (the line-847 block now parses). Report the FIRST remaining error, if any (that's the next gap — do not fix it here). If `/usr/share/bash-completion/bash_completion` is absent, skip and say so.

- [ ] **Step 4: Commit**

```bash
git add tests/dbracket_regex_integration.rs
git commit -m "test: =~ regex-operand match semantics + bash_completion line-847 fix

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: bash-diff harness (30th)

**Files:** Create `tests/scripts/dbracket_regex_diff_check.sh`.

- [ ] **Step 1: Write the harness**

Copy the structure of `tests/scripts/source_reader_diff_check.sh` (same `HUCK_BIN` resolution, `check()` helper comparing combined stdout+stderr+exit of `bash --norc --noprofile` vs huck, `Total/Pass/Fail` footer, non-zero exit on failure). `cargo build` first. Fragments (each prints a deterministic `yes`/`no`):

```
[[ "a b" =~ (a b) ]] && echo yes || echo no
[[ ab =~ (a b) ]] && echo yes || echo no
[[ "[no-]" =~ (\[((no|dont)-?)\]). ]] && echo yes || echo no
c=foo=bar; [[ $c =~ ^([A-Za-z_][A-Za-z0-9_]*)=(.*)$ ]] && echo yes || echo no
[[ "-abc" =~ (-[^]]+) ]] && echo yes || echo no
[[ /etc =~ ^\~.*|^\/.* ]] && echo yes || echo no
re='(a|b)'; [[ a =~ $re ]] && echo yes || echo no
[[ -n a && ( -z "" || -n b ) ]] && echo yes || echo no
```

- [ ] **Step 2: Run**

`bash tests/scripts/dbracket_regex_diff_check.sh 2>&1 | tail` → `Total: 8, Pass: 8, Fail: 0`. If a fragment legitimately diverges due to the documented quoted-literal divergence or another UNRELATED pre-existing limitation, confirm by running both shells manually, then drop it with a `# dropped: <reason>` comment and report it — do NOT mask a real M-100 bug.

- [ ] **Step 3: Commit**

```bash
git add tests/scripts/dbracket_regex_diff_check.sh
git commit -m "test: bash-diff harness for [[ =~ ]] regex operands (30th)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Documentation

**Files:** Modify `docs/bash-divergences.md`, `README.md`.

- [ ] **Step 1: Read structure**

`grep -n '^## Change log\|Tier 1\|Last updated\|^- \*\*L-22\|2026-06-07' docs/bash-divergences.md | head` and `grep -n 'v104' README.md`. Read the M-99 entry, the v104 change-log entry + README row, and the L-22 note to match formatting. Confirm next free is **M-100** and next L is **L-23**.

- [ ] **Step 2: Add M-100 `[fixed v105]`**

Tier-1 (Bugs) entry: the lexer grabbed a `=~` regex's `((` as an arith block (scan to EOF → `unterminated '(('`) / `(` as grouping (→ `missing operand`), because `[[ ]]` lexing was context-free; fix = `dbracket_depth`/`expect_regex` state + `scan_regex_operand` (one literal regex Word, naive paren-depth, `$`/quotes/escapes intact, no brace/extglob); parser/evaluator unchanged; reached via v104. Bump the Tier-1 count.

- [ ] **Step 3: Add L-23**

Tier-4 `[intentional]`: bash matches a *quoted* substring of an `=~` regex literally (escaping regex metacharacters); huck expands the Word and passes it to `regex::Regex`, so quoted metachars stay active — pre-existing, unaffected by M-100 (bash_completion uses `\`-escapes, not quoting-for-literal). Bump the Tier-4 count / "Last updated".

- [ ] **Step 4: Change-log + README row**

`2026-06-07` v105 change-log entry (style of v104): mechanism, the bash_completion line-847 payoff, 30th harness, test count, L-23. Add the v105 README iteration row after v104.

- [ ] **Step 5: Verify + commit**

`grep -n 'M-100\|fixed v105\|L-23\|v105' docs/bash-divergences.md README.md` → real numbers, no placeholders.
```bash
git add docs/bash-divergences.md README.md
git commit -m "docs: v105 [[ =~ ]] regex-operand lexing (M-100) — changelog, README, L-23

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Final (after all tasks)

- [ ] Whole-branch review: `git log --oneline main..HEAD`, `git diff --stat main..HEAD`.
- [ ] `cargo test 2>&1 | tail -5` (full suite green), `cargo clippy --all-targets 2>&1 | tail -2` (clean).
- [ ] All harnesses: `for f in tests/scripts/*_diff_check.sh; do bash "$f" >/dev/null 2>&1 || echo "FAIL $f"; done` (silent = all pass).
- [ ] AskUserQuestion merge gate, then `git merge --no-ff` + push + delete branch, then update memory files.
