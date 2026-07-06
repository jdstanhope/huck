# v263 Subscript quote-wrap (array-lit + param-expansion `[sub]`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** On the dormant atom-command path, wrap a bare `"…"`/`'…'` inside a subscript operand (`Mode::ParamSubscriptOperand` — array-literal `a=([sub]=v)` and param-expansion `${a[sub]}`) in `Quoted{Double}`/`Quoted{Single}` to match the `command.rs` oracle, without touching value-family operands.

**Architecture:** Two arms in `scan_step_param_operand` (lexer.rs) gated on `end == ']'` emit signals (`BeginDquote` for `"`, `QuoteRun{Single}` for `'`); one new `QuoteRun` arm in `parse_word` (parser.rs, the operand assembler) wraps them. The `BeginDquote` double-quote wrap is already handled by the v259 F3 arm. `command.rs` UNTOUCHED. `command_atoms` stays `false` — dormant/differential, verified via `new_seq` (atom) vs `old_seq` (oracle) full-AST equality.

**Tech Stack:** Rust, single crate `huck-syntax`.

## Global Constraints

- `command.rs` diff-vs-`main` = EMPTY. The change is in `lexer.rs` (`scan_step_param_operand`) + `parser.rs` (`parse_word` + tests).
- `command_atoms` stays `false` at both constructor sites.
- Box is 1 core / 1.9 GB. The ONLY test command is `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` (append a test name before `--` to run one). NEVER `--workspace`, NEVER multi-threaded — it OOM-kills the session.
- `cargo build -p huck-syntax` → 0 warnings; `scan_step_param_operand` and `parse_word`'s match stay exhaustive.
- `diff_cmd(s)` asserts `new_seq(s).unwrap() == old_seq(s).unwrap()` (full-AST). `old_seq` uses `.expect("lex")`, so a lex-error input PANICS — every corpus input here is parse-clean, so `diff_cmd` is safe.
- The gate is `end == ']'` — uniquely `Mode::ParamSubscriptOperand`. Value families (`end == '}'`) MUST keep their flat inlining unchanged.
- Commit trailer VERBATIM: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- rust-analyzer PHANTOM diagnostics — trust `cargo`, not the editor.

---

### Task 1: Subscript quote-wrap — two lexer signals + one parser arm + corpus

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` — `scan_step_param_operand` (the `'…'` arm ~1531 and the bare-`"` arm ~1557).
- Modify: `crates/huck-syntax/src/parser.rs` — `parse_word` (~26; add a `QuoteRun` arm) + a new test in `mod tests`.

**Interfaces:**
- Consumes: `TokenKind::BeginDquote`, `TokenKind::QuoteRun { style, text }`, `QuoteStyle::Single` (all already in scope; `QuoteStyle` imported in parser.rs line 15). `end: char` param of `scan_step_param_operand` (`']'` ⇔ subscript). `parse_word`'s existing `BeginDquote` F3 arm.
- Produces: no new public interface.

- [ ] **Step 1: Write the failing test**

Add to `crates/huck-syntax/src/parser.rs mod tests`:
```rust
    #[test]
    fn atoms_subscript_quote_wrap() {
        // v263: a bare "…"/'…' in a SUBSCRIPT operand (array-literal [sub]= and
        // param-expansion ${a[sub]}) wraps in Quoted{Double}/Quoted{Single} to
        // match the oracle's scan_subscript. Value families stay flat (guards).
        diff_cmd("a=([\"k\"]=v)");
        diff_cmd("a=(['k']=v)");
        diff_cmd("a=([\"\"]=v)");
        diff_cmd("a=(['']=v)");
        diff_cmd("a=([\"k$x\"]=v)");
        diff_cmd("a=([x\"y\"z]=v)");
        diff_cmd("a=([x'y'z]=v)");
        diff_cmd("a+=([\"k\"]=v)");
        diff_cmd("${a[\"k\"]}");
        diff_cmd("${a['k']}");
        diff_cmd("${a[x\"y\"]}");
        diff_cmd("declare -A m=([\"k\"]=v)");
        // Regression guards — must STAY byte-identical.
        diff_cmd("${x:-\"y\"}");      // value operand — FLAT (not wrapped)
        diff_cmd("${x:-'y'}");        // value single-quote — FLAT
        diff_cmd("a=([$\"k\"]=v)");   // v259 F3 dquote — already wraps
        diff_cmd("${a[$\"k\"]}");     // F3 in param-expansion subscript
        diff_cmd("a=([k]=v)");        // plain — flat quoted:false
        diff_cmd("${a[k]}");          // plain
    }
```

- [ ] **Step 2: Run the test to verify it FAILS**

Run: `cargo test -p huck-syntax --jobs 1 --lib atoms_subscript_quote_wrap -- --test-threads 1`
Expected: FAIL — `a=(["k"]=v)` panics in `diff_cmd` (atom subscript `[Literal{"k",true}]` flat vs oracle `[Quoted{Double,[Literal{"k",true}]}]`).

- [ ] **Step 3: Lexer — bare-`"` arm emits `BeginDquote` for subscripts**

In `crates/huck-syntax/src/lexer.rs`, `scan_step_param_operand`, at the START of the `Some('"') =>` arm (the "Opening `"` — begin a double-quoted span" arm ~1557, BEFORE its `self.cursor.next(); // consume opening "`), insert the subscript gate:
```rust
                Some('"') => {
                    // v263: in a subscript operand, wrap a bare `"…"` in
                    // Quoted{Double} (like the oracle's scan_subscript). Emit a
                    // zero-width BeginDquote — leave the `"` for parse_dquote,
                    // exactly like the `$"` arm — so parse_word's F3 arm wraps it;
                    // the mode switch to Mode::DoubleQuote guarantees forward
                    // progress. Value families (end == '}') keep the flat inline.
                    if end == ']' {
                        self.history.push(Token::new(TokenKind::BeginDquote, Span::new(off, l, c)));
                        return Ok(Step::Produced);
                    }
                    self.cursor.next(); // consume opening `"`
                    // ... existing flat-inline logic unchanged ...
```
(Only the `if end == ']' { … }` block is added; the rest of the arm is untouched.)

- [ ] **Step 4: Lexer — `'…'` arm emits `QuoteRun{Single}` for subscripts**

In the same function, the single-quote arm (~1531) currently ends with:
```rust
                    self.history.push(Token::new(
                        TokenKind::Lit { text, quoted: true },
                        Span::new(off, l, c),
                    ));
                    return Ok(Step::Produced);
```
Replace the pushed token with a subscript-gated choice:
```rust
                    // v263: a subscript operand wraps a bare `'…'` in
                    // Quoted{Single} (oracle scan_subscript). Emit QuoteRun{Single}
                    // so parse_word wraps it; value families keep the flat Lit.
                    let tok = if end == ']' {
                        TokenKind::QuoteRun { style: QuoteStyle::Single, text }
                    } else {
                        TokenKind::Lit { text, quoted: true }
                    };
                    self.history.push(Token::new(tok, Span::new(off, l, c)));
                    return Ok(Step::Produced);
```
Confirm `QuoteStyle` is in scope in lexer.rs (it is — `QuoteRun { style: QuoteStyle::Single, .. }` is used elsewhere, e.g. lexer.rs:3516).

- [ ] **Step 5: Parser — add a `QuoteRun` arm to `parse_word`**

In `crates/huck-syntax/src/parser.rs`, `parse_word` (~26), in the `match kind { … }` that has the `TokenKind::BeginDquote =>` F3 arm, add a `QuoteRun` arm BEFORE the final `_ => { return Err(ParseError::UnsupportedExpansion); }`:
```rust
            // v263: a bare `'…'` inside a SUBSCRIPT operand (scan_step_param_operand
            // emits QuoteRun{Single} only when end==']') wraps in Quoted{Single} to
            // match the oracle's scan_subscript. QuoteRun reaches parse_word solely
            // from Mode::ParamSubscriptOperand; value families keep emitting flat Lit.
            TokenKind::QuoteRun { style, text } => {
                parts.push(WordPart::Quoted { style, parts: vec![WordPart::Literal { text, quoted: true }] });
            }
```

- [ ] **Step 6: Run the test to verify it PASSES**

Run: `cargo test -p huck-syntax --jobs 1 --lib atoms_subscript_quote_wrap -- --test-threads 1`
Expected: PASS (all 18 `diff_cmd` cases — 12 fixed + 6 regression guards — byte-identical).

If a FIXED case still fails, inspect the atom vs oracle AST (the empty `""` case relies on `parse_dquote`'s empty-`""` marker producing `Quoted{Double,[Literal{"",true}]}`; the mixed case relies on `parse_word` pushing each plain literal separately). Do NOT weaken a test — fix the mechanism. If a REGRESSION GUARD fails, the `end == ']'` gate leaked into value families — narrow it.

- [ ] **Step 7: Run the full suite + gates**

Run: `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` → all green.
Run: `cargo build -p huck-syntax` → 0 warnings.
Run: `git diff --stat main -- crates/huck-syntax/src/command.rs` → EMPTY.

If any pre-existing operand/value-family test now fails, STOP and report it (the gate must keep value families flat). Do NOT weaken any test.

- [ ] **Step 8: Commit**

```bash
git add crates/huck-syntax/src/lexer.rs crates/huck-syntax/src/parser.rs
git commit -m "v263: wrap bare quotes in subscript operands (array-lit + param-expansion)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review

- **Spec coverage:** double-quote wrap (bare-`"`→`BeginDquote`) → Step 3 + the existing F3 arm; single-quote wrap (`'…'`→`QuoteRun{Single}`→new arm) → Steps 4-5; both contexts (array-lit + param-expansion share `Mode::ParamSubscriptOperand`/`end==']'`) covered by one gate; value-family guards → Step 1 + the `end==']'` gate. ✓
- **Placeholder scan:** none. Fix code and all 18 test inputs verbatim.
- **Type consistency:** `TokenKind::BeginDquote`, `TokenKind::QuoteRun { style, text }`, `QuoteStyle::Single`, `WordPart::Quoted { style, parts }`, `WordPart::Literal { text, quoted }` — all match existing usage (lexer.rs:3516, parser.rs:356/523). `end: char` and `off/l/c` are the existing params/locals of `scan_step_param_operand`.
