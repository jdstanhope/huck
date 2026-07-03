# v253 — `[[ … ]]` conditional expressions on the atom-command path — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Port `[[ … ]]` extended-test compound commands (minus `=~`, deferred to v254) onto huck's DORMANT atom-command parser, byte-identical to the `command.rs` oracle.

**Architecture:** PARSER-ONLY (no lexer change). The atom command scanner already emits the right atoms inside `[[ ]]` (the oracle applies no special tokenization there except `=~`). Add an atom-native `parse_double_bracket` + the precedence cascade (`or→and→not→primary→atom`) to `parser.rs`, reading each operand/operator word via `parse_word_command` and reusing the oracle's pure text-classifiers (made `pub(crate)`). `=~` returns a clean `UnsupportedCommand` deferral.

**Tech Stack:** Rust; `crates/huck-syntax/src/parser.rs` (new atom-native grammar + tests), `crates/huck-syntax/src/command.rs` (visibility widenings ONLY — no logic change).

## Global Constraints

- **Dormant + differential.** `command_atoms` stays `false` at both definition sites in `lexer.rs`. Every in-scope input must parse to the SAME AST / same error on the atom path (`new_seq`) as the oracle (`old_seq`); `diff_cmd(s)` asserts equality. A well-formed in-scope divergence is a BUG to fix in the ATOM path — never change the oracle's logic.
- **Production logic untouched.** Do NOT modify `command.rs`'s `parse_double_bracket` / `parse_double_bracket_with_assigns` / the `parse_test_*` cascade / the `TestExpr` grammar, nor the lexer's `dbracket_depth` / `expect_regex` / `scan_regex_operand`. The ONLY permitted `command.rs` change is widening `pub(crate)` visibility on reused helpers (no body change). `git diff main -- crates/huck-syntax/src/command.rs` must contain ONLY visibility-keyword changes.
- **`=~` DEFERRED to v254.** On the atom path, `[[ x =~ re ]]` returns `ParseError::UnsupportedCommand`. The parser MUST bail on the `=~` operator word BEFORE reading the regex RHS (so the regex operand is never pulled/mis-lexed). Tested as a deferral-parity assertion, NOT a `diff_cmd`, NOT a pinned divergence.
- **No new lexer mode.** `Mode::DoubleBracket` / `Mode::Regex` stay reserved (Regex arrives in v254).
- **Test runner (box is 1 core / 1.9 GiB — `--workspace`/parallel OOM-kills the session).** ALWAYS: `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` (narrow with a name filter while iterating). Warnings: `cargo build -p huck-syntax 2>&1 | grep -c warning` → must print `0`.
- **Branch** `v253-double-bracket` (NOT main). Every commit ends with `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- **rust-analyzer phantom diagnostics**: the IDE intermittently shows false compile errors after edits. Trust `cargo`, not the IDE.

---

## Background the implementer needs

**The oracle is the exact reference to mirror** (`crates/huck-syntax/src/command.rs`):
- `parse_double_bracket` (2753) / `parse_double_bracket_with_assigns` (2759): consume `[[`; `skip_test_newlines`; `]]`-immediately → `EmptyDoubleBracket`; EOF → `UnterminatedDoubleBracket`; `expr = parse_test_or`; `skip_test_newlines`; consume `]]` (missing/EOF → `UnterminatedDoubleBracket`); return `Command::DoubleBracket { expr: Box::new(expr), inline_assignments }`.
- `parse_test_or` (`||`, `Op(Or)`) → `parse_test_and` (`&&`, `Op(And)`) → `parse_test_not` (`!` right-assoc, `is_bang_word`) → `parse_test_primary` (`( expr )` via `Op(LParen)`/`Op(RParen)`) → `parse_test_atom`.
- `parse_test_atom` (2613): EOF → `UnterminatedDoubleBracket`; a present terminator (`]]`/`)`) → `EmptyDoubleBracket`; read first operand Word; if `try_unary_op` → read one more operand → `Unary`; else lhs = first, if NOT `next_is_test_binary_operator` → lone-word `Unary{StringNonEmpty}`; else consume operator (`Op(RedirIn)`→StringLt, `Op(RedirOut)`→StringGt, or a Word matched on `==`/`=`/`!=`/`=~`/`<`/`>`/`-eq`/`-ne`/`-lt`/`-gt`/`-le`/`-ge`/`-nt`/`-ot`/`-ef`), read rhs → `Binary`. Unknown op → `TestExprBadOperator`; `]]` in operator slot → `UnterminatedDoubleBracket`.
- Helpers: `next_test_word` (2530: EOF→`UnterminatedDoubleBracket`; `]]`/`Op(_)`→`TestExprMissingOperand`; else the Word), `next_is_test_binary_operator` (2448), `try_unary_op` (2486), `is_bang_word` (2517), `skip_test_newlines` (2465), `word_literal_text` (2472, ALREADY `pub`).

**Key atom-path adaptation:** the oracle reads operands/operators as pre-lexed `TokenKind::Word` tokens; the atom stream has `Lit`/expansion atoms instead. So the atom version reads each operand by ASSEMBLING a Word via `parse_word_command(iter, false)` (parser.rs:118). Test OPERATORS are single unquoted `Lit` atoms on the atom path (space-delimited: `==`, `!=`, `=~`, `-eq`, … each lex to one `Lit`; `<`/`>` are `Op(RedirIn)`/`Op(RedirOut)`), so peeking one atom classifies them.

**Dispatch hook:** the atom `parse_command` keyword dispatch (parser.rs:1926) has arms for `LBrace`/`If`/`While`/`For`/`Select`/`Case`/`Function` and `Some(_) => Err(UnsupportedCommand)`. `Keyword::DoubleBracketOpen` currently hits that default.

**Reusable:** `parse_word_command(iter, false)`; `TestExpr`/`TestUnaryOp`/`TestBinaryOp` (all `pub` in `command.rs`); `word_literal_text` (`pub`); `keyword_of`/`peek_leading_keyword` (atom keyword recognition, parser.rs). `ParseError::{EmptyDoubleBracket, UnterminatedDoubleBracket, TestExprMissingOperand, TestExprBadOperator, UnsupportedCommand}` all exist.

**The differential harness** (`parser.rs mod tests`): `new_seq(s)` (atom path), `old_seq(s)` (oracle), `diff_cmd(s)` (asserts equal ASTs). For lex-level rejects use `new_seq(s).is_err()` + the crate's fallible oracle helper (grep how `atoms_array_literal_error_parity` does it).

---

## Task 1: `[[ ]]` grammar core — dispatch + cascade + unary/binary/lone-word

**Files:**
- Modify: `crates/huck-syntax/src/command.rs` — widen `try_unary_op`, `is_bang_word`, `next_is_test_binary_operator`, `skip_test_newlines` to `pub(crate)` (visibility ONLY; do NOT touch bodies). (`word_literal_text` is already `pub`.)
- Modify: `crates/huck-syntax/src/parser.rs` — `[[` dispatch arm in `parse_command`; new atom-native `parse_double_bracket` + `parse_test_or`/`_and`/`_not`/`_primary`/`_atom` + `next_test_word_atom` + atom `next_is_test_binary_operator` peek; tests in `mod tests`.

**Interfaces:**
- Produces: `fn parse_double_bracket(iter: &mut Lexer, inline_assignments: Vec<Assignment>) -> Result<Command, ParseError>` and the five cascade fns (all `fn ... (iter: &mut Lexer) -> Result<TestExpr, ParseError>`).
- Consumes: `parse_word_command`, `peek_leading_keyword`, `keyword_of`, `word_literal_text`, `try_unary_op`, `is_bang_word`, `skip_test_newlines`.

- [ ] **Step 1: Widen the oracle classifiers to `pub(crate)`.** In `command.rs`, change `fn try_unary_op` (2486) → `pub(crate) fn try_unary_op`; `fn is_bang_word` (2517) → `pub(crate) fn is_bang_word`; `fn next_is_test_binary_operator` (2448) → `pub(crate) fn next_is_test_binary_operator`; `fn skip_test_newlines` (2465) → `pub(crate) fn skip_test_newlines`. Do NOT change any body. Build to confirm: `cargo build -p huck-syntax 2>&1 | grep -c warning` → `0`.

- [ ] **Step 2: Add the `[[` dispatch arm.** In `parser.rs` `parse_command`, in the `match peek_leading_keyword(iter)? { … }` (line ~1926), add before `Some(_) => return Err(ParseError::UnsupportedCommand),`:

```rust
        Some(Keyword::DoubleBracketOpen) => return parse_double_bracket(iter, Vec::new()),
```

- [ ] **Step 3: Write the atom-native grammar** in `parser.rs` (place near the other compound parsers). Import `TestExpr`, `TestUnaryOp`, `TestBinaryOp`, `Assignment` from `crate::command` if not already imported.

```rust
/// v253: atom-native `[[ … ]]`. Mirrors command.rs parse_double_bracket_with_assigns,
/// but reads operands via parse_word_command (the atom stream has Lit atoms, not
/// pre-lexed Word tokens). `=~` is DEFERRED to v254 (returns UnsupportedCommand).
fn parse_double_bracket(iter: &mut Lexer, inline_assignments: Vec<Assignment>) -> Result<Command, ParseError> {
    iter.next_kind()?; // consume `[[`
    skip_test_newlines(iter)?;
    if iter.peek_kind()?.and_then(keyword_of) == Some(Keyword::DoubleBracketClose) {
        return Err(ParseError::EmptyDoubleBracket);
    }
    if iter.peek_kind()?.is_none() {
        return Err(ParseError::UnterminatedDoubleBracket);
    }
    let expr = parse_test_or(iter)?;
    skip_test_newlines(iter)?;
    match iter.next_kind()? {
        Some(tok) if keyword_of(&tok) == Some(Keyword::DoubleBracketClose) => {}
        _ => return Err(ParseError::UnterminatedDoubleBracket),
    }
    Ok(Command::DoubleBracket { expr: Box::new(expr), inline_assignments })
}

fn parse_test_or(iter: &mut Lexer) -> Result<TestExpr, ParseError> {
    let mut lhs = parse_test_and(iter)?;
    skip_test_newlines(iter)?;
    while matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::Or))) {
        iter.next_kind()?;
        skip_test_newlines(iter)?;
        let rhs = parse_test_and(iter)?;
        lhs = TestExpr::Or(Box::new(lhs), Box::new(rhs));
        skip_test_newlines(iter)?;
    }
    Ok(lhs)
}

fn parse_test_and(iter: &mut Lexer) -> Result<TestExpr, ParseError> {
    let mut lhs = parse_test_not(iter)?;
    skip_test_newlines(iter)?;
    while matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::And))) {
        iter.next_kind()?;
        skip_test_newlines(iter)?;
        let rhs = parse_test_not(iter)?;
        lhs = TestExpr::And(Box::new(lhs), Box::new(rhs));
        skip_test_newlines(iter)?;
    }
    Ok(lhs)
}

fn parse_test_not(iter: &mut Lexer) -> Result<TestExpr, ParseError> {
    if iter.peek_kind()?.as_ref().map(is_bang_word).unwrap_or(false) {
        iter.next_kind()?;
        let inner = parse_test_not(iter)?;
        return Ok(TestExpr::Not(Box::new(inner)));
    }
    parse_test_primary(iter)
}

fn parse_test_primary(iter: &mut Lexer) -> Result<TestExpr, ParseError> {
    if matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::LParen))) {
        iter.next_kind()?;
        let inner = parse_test_or(iter)?;
        match iter.next_kind()? {
            Some(TokenKind::Op(Operator::RParen)) => {}
            None => return Err(ParseError::UnterminatedDoubleBracket),
            _ => return Err(ParseError::TestExprMissingOperand),
        }
        return Ok(inner);
    }
    parse_test_atom(iter)
}
```

  Notes: `is_bang_word` takes `&TokenKind`; `iter.peek_kind()?` returns `Option<TokenKind>` (owned) — adapt the `.as_ref().map(is_bang_word)` to the actual peek return type (grep an existing `peek_kind` use to confirm whether it returns owned or borrowed; match that). `Operator::And`/`Or`/`LParen`/`RParen` are the atom operator variants (grep `Operator::` to confirm names).

- [ ] **Step 4: Write `parse_test_atom` + `next_test_word_atom` + the binary-operator peek.** Continue in `parser.rs`:

```rust
/// Reads one operand Word inside `[[ ]]`. EOF → UnterminatedDoubleBracket;
/// a `]]`/`)`/operator where an operand was expected → TestExprMissingOperand.
fn next_test_word_atom(iter: &mut Lexer) -> Result<Word, ParseError> {
    match iter.peek_kind()? {
        None => return Err(ParseError::UnterminatedDoubleBracket),
        Some(ref tok) => {
            if keyword_of(tok) == Some(Keyword::DoubleBracketClose)
                || matches!(tok, TokenKind::Op(_))
            {
                return Err(ParseError::TestExprMissingOperand);
            }
        }
    }
    parse_word_command(iter, false)
}

/// True if the next atom is a `[[ ]]` binary operator. `<`/`>` → Op(RedirIn/RedirOut);
/// every other operator is a single unquoted Lit atom. KEEP IN SYNC with parse_test_atom.
fn next_is_test_binary_operator_atom(iter: &mut Lexer) -> Result<bool, ParseError> {
    Ok(match iter.peek_kind()? {
        Some(TokenKind::Op(Operator::RedirIn)) | Some(TokenKind::Op(Operator::RedirOut)) => true,
        Some(TokenKind::Lit { text, quoted: false }) => matches!(
            text.as_str(),
            "==" | "=" | "!=" | "=~" | "-eq" | "-ne" | "-lt" | "-gt"
                | "-le" | "-ge" | "-nt" | "-ot" | "-ef"
        ),
        _ => false,
    })
}

fn parse_test_atom(iter: &mut Lexer) -> Result<TestExpr, ParseError> {
    if iter.peek_kind()?.is_none() {
        return Err(ParseError::UnterminatedDoubleBracket);
    }
    // Present terminator with nothing before it → empty body.
    match iter.peek_kind()? {
        Some(ref tok) if keyword_of(tok) == Some(Keyword::DoubleBracketClose)
            || matches!(tok, TokenKind::Op(Operator::RParen)) => {
            return Err(ParseError::EmptyDoubleBracket);
        }
        _ => {}
    }
    // A leading operator (not `(`) where an operand was expected.
    if matches!(iter.peek_kind()?, Some(TokenKind::Op(_))) {
        return Err(ParseError::TestExprMissingOperand);
    }

    let first = parse_word_command(iter, false)?;

    if let Some(op) = try_unary_op(&first) {
        let operand = next_test_word_atom(iter)?;
        return Ok(TestExpr::Unary { op, operand });
    }

    let lhs = first;
    if !next_is_test_binary_operator_atom(iter)? {
        return Ok(TestExpr::Unary { op: TestUnaryOp::StringNonEmpty, operand: lhs });
    }

    // Consume the operator.
    match iter.peek_kind()? {
        Some(TokenKind::Op(Operator::RedirIn)) => { iter.next_kind()?; let rhs = next_test_word_atom(iter)?; Ok(TestExpr::Binary { op: TestBinaryOp::StringLt, lhs, rhs }) }
        Some(TokenKind::Op(Operator::RedirOut)) => { iter.next_kind()?; let rhs = next_test_word_atom(iter)?; Ok(TestExpr::Binary { op: TestBinaryOp::StringGt, lhs, rhs }) }
        _ => {
            let op_word = parse_word_command(iter, false)?;
            let op_text = match word_literal_text(&op_word) {
                Some(t) => t.to_string(),
                None => return Err(ParseError::TestExprBadOperator(format!("{op_word:?}"))),
            };
            match op_text.as_str() {
                "==" | "=" => { let rhs = next_test_word_atom(iter)?; Ok(TestExpr::Binary { op: TestBinaryOp::StringEq, lhs, rhs }) }
                "!=" => { let rhs = next_test_word_atom(iter)?; Ok(TestExpr::Binary { op: TestBinaryOp::StringNe, lhs, rhs }) }
                "=~" => Err(ParseError::UnsupportedCommand), // v254 deferral — DO NOT read the pattern
                "<" => { let rhs = next_test_word_atom(iter)?; Ok(TestExpr::Binary { op: TestBinaryOp::StringLt, lhs, rhs }) }
                ">" => { let rhs = next_test_word_atom(iter)?; Ok(TestExpr::Binary { op: TestBinaryOp::StringGt, lhs, rhs }) }
                "-eq" => { let rhs = next_test_word_atom(iter)?; Ok(TestExpr::Binary { op: TestBinaryOp::IntEq, lhs, rhs }) }
                "-ne" => { let rhs = next_test_word_atom(iter)?; Ok(TestExpr::Binary { op: TestBinaryOp::IntNe, lhs, rhs }) }
                "-lt" => { let rhs = next_test_word_atom(iter)?; Ok(TestExpr::Binary { op: TestBinaryOp::IntLt, lhs, rhs }) }
                "-gt" => { let rhs = next_test_word_atom(iter)?; Ok(TestExpr::Binary { op: TestBinaryOp::IntGt, lhs, rhs }) }
                "-le" => { let rhs = next_test_word_atom(iter)?; Ok(TestExpr::Binary { op: TestBinaryOp::IntLe, lhs, rhs }) }
                "-ge" => { let rhs = next_test_word_atom(iter)?; Ok(TestExpr::Binary { op: TestBinaryOp::IntGe, lhs, rhs }) }
                "-nt" => { let rhs = next_test_word_atom(iter)?; Ok(TestExpr::Binary { op: TestBinaryOp::NewerThan, lhs, rhs }) }
                "-ot" => { let rhs = next_test_word_atom(iter)?; Ok(TestExpr::Binary { op: TestBinaryOp::OlderThan, lhs, rhs }) }
                "-ef" => { let rhs = next_test_word_atom(iter)?; Ok(TestExpr::Binary { op: TestBinaryOp::SameFile, lhs, rhs }) }
                other => Err(ParseError::TestExprBadOperator(other.to_string())),
            }
        }
    }
}
```

  Remove the stray `let bin = |op| { };` placeholder line — it is NOT real code, it marks where the match lives. Confirm the exact `TestBinaryOp` variant names by grepping `enum TestBinaryOp` in `command.rs` (the names above — `StringEq`/`StringNe`/`StringLt`/`StringGt`/`IntEq`/…/`SameFile` — are from `parse_test_atom`; verify). Confirm `TokenKind::Lit { text, quoted }` field names by grepping an existing `Lit` match in `parser.rs`.

- [ ] **Step 5: Write the failing test, run it (FAIL), implement Steps 1-4, run (PASS).** Add to `parser.rs` `mod tests`:

```rust
    #[test]
    fn atoms_double_bracket_core() {
        diff_cmd("[[ -f /etc/passwd ]]");     // unary file test
        diff_cmd("[[ -z $x ]]");              // unary string test w/ expansion
        diff_cmd("[[ hello ]]");              // lone word ≡ -n hello
        diff_cmd("[[ $x ]]");                 // lone word w/ expansion
        diff_cmd("[[ a == b ]]");             // string eq
        diff_cmd("[[ a = b ]]");              // string eq (single =)
        diff_cmd("[[ a != b ]]");             // string ne
        diff_cmd("[[ $x == a* ]]");           // glob RHS stays a pattern word
        diff_cmd("[[ 3 -eq 3 ]]");            // int eq
        diff_cmd("[[ 3 -lt 5 ]]");            // int lt
        diff_cmd("[[ a < b ]]");              // string lt via Op(RedirIn)
        diff_cmd("[[ a > b ]]");              // string gt via Op(RedirOut)
        diff_cmd("[[ f1 -nt f2 ]]");          // file newer-than
        diff_cmd("[[ -f a && -f b ]]");       // logical and
        diff_cmd("[[ -f a || -f b ]]");       // logical or
        diff_cmd("[[ ! -d c ]]");             // negation
        diff_cmd("[[ ( a == b ) ]]");         // grouping
        diff_cmd("[[ -f a && -f b || ! -d c ]]"); // precedence
    }
```

  Run: `cargo test -p huck-syntax --jobs 1 --lib atoms_double_bracket_core -- --test-threads 1`. Expected: FAIL before (parser defers → `UnsupportedCommand` unwrap panic in `new_seq`), PASS after. For any divergence, print `new_seq(s)` vs `old_seq(s)` and reconcile to the oracle (fix the atom path).

- [ ] **Step 6: Full-suite + warnings gate, then commit.** `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` (≥ prior count, all green); `cargo build -p huck-syntax 2>&1 | grep -c warning` → `0`.

```bash
git add crates/huck-syntax/src/command.rs crates/huck-syntax/src/parser.rs
git commit -m "v253 T1: [[ … ]] grammar core (unary/binary/lone-word/logical/grouping) via atom parse_double_bracket

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Precedence, grouping & newlines corpus

Hardens the T1 cascade with an adversarial precedence/grouping/newline corpus (the code lands in T1; T2 proves it and reconciles any divergence).

**Files:** Modify `crates/huck-syntax/src/parser.rs` (tests; plus any atom-path reconciliation the corpus surfaces).

- [ ] **Step 1: Write the corpus test, run FAIL/reconcile→PASS.**

```rust
    #[test]
    fn atoms_double_bracket_precedence() {
        diff_cmd("[[ a && b && c ]]");            // left-assoc &&
        diff_cmd("[[ a || b || c ]]");            // left-assoc ||
        diff_cmd("[[ a && b || c && d ]]");       // && binds tighter than ||
        diff_cmd("[[ ! a && b ]]");               // ! binds tighter than &&
        diff_cmd("[[ ! ! a ]]");                  // right-assoc double negation
        diff_cmd("[[ ( a || b ) && c ]]");        // grouping overrides precedence
        diff_cmd("[[ ( ( a ) ) ]]");              // nested grouping
        diff_cmd("[[ -n a && ( -z b || -f c ) ]]");
        diff_cmd("[[ a\n&&\nb ]]");               // newlines around &&
        diff_cmd("[[\n  a == b\n]]");             // newlines after [[ and before ]]
        diff_cmd("[[ a ||\n b ]]");               // newline after ||
    }
```

  Run: `cargo test -p huck-syntax --jobs 1 --lib atoms_double_bracket_precedence -- --test-threads 1`. Reconcile every divergence to `old_seq`.

- [ ] **Step 2: Full-suite + warnings gate, then commit.**

```bash
git add crates/huck-syntax/src/parser.rs
git commit -m "v253 T2: [[ … ]] precedence/grouping/newlines corpus

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Inline assignments + stage integration + `=~` deferral

**Files:** Modify `crates/huck-syntax/src/parser.rs` (the `parse_command`/`parse_simple_with_leading_word` inline-assignment routing + tests).

**Interfaces:** Consumes `parse_double_bracket(iter, assigns)` from Task 1; `is_assignment_word`/`try_split_assignment` (already used by the atom parser at parser.rs:1817/1840).

- [ ] **Step 1: `=~` deferral assertion** (verify the T1 bail is correct). Add:

```rust
    #[test]
    fn atoms_double_bracket_regex_deferred() {
        // v254 will make these diff_cmd. For v253 the atom path defers on `=~`
        // BEFORE reading the regex RHS; the oracle parses TestExpr::Regex.
        assert!(matches!(new_seq("[[ x =~ ^a.*b$ ]]"), Err(ParseError::UnsupportedCommand)));
        assert!(matches!(new_seq("[[ $s =~ [0-9]+ ]]"), Err(ParseError::UnsupportedCommand)));
        // And the oracle DOES support them (sanity — different result, hence not diff_cmd):
        assert!(old_seq("[[ x =~ ^a.*b$ ]]").is_ok());
    }
```

  If `ParseError::UnsupportedCommand` is not directly importable in the test module, match on the crate path used elsewhere (grep an existing `Err(ParseError::UnsupportedCommand)` assertion in `parser.rs` tests). Run and confirm PASS.

- [ ] **Step 2: Inline assignments `FOO=hi [[ … ]]`.** The atom `parse_command` falls through to the funcdef-lookahead / `parse_simple` for a leading assignment word. Mirror the oracle's `parse_command_or_pipeline` dispatch (command.rs:1088-1111): when the leading word(s) are assignments and `[[` follows, route to `parse_double_bracket(iter, assigns)`.

  Implementation guidance (choose the least-invasive insertion, verified by `diff_cmd`): in `parse_simple_with_leading_word`'s word-assembly loop, BEFORE assembling the next word, if every word collected so far is an `is_assignment_word` AND `peek_leading_keyword(iter)? == Some(Keyword::DoubleBracketOpen)`, peel the collected words into `Vec<Assignment>` via `try_split_assignment` and `return parse_double_bracket(iter, assigns)`. Study parser.rs:1808-1845 (where `all_words` is built and leading assignments are peeled) to find the exact spot; the goal is to intercept BEFORE `[[` is assembled as an ordinary command word. Reconcile against the oracle via the tests below; if the interception proves to require restructuring beyond a localized check, STOP and report it (candidate to narrow scope) rather than forcing it.

```rust
    #[test]
    fn atoms_double_bracket_inline_assignments() {
        diff_cmd("FOO=hi [[ -n $FOO ]]");
        diff_cmd("A=1 B=2 [[ $A == 1 ]]");        // multiple leading assignments
        diff_cmd("x=y [[ $x == y && -n $x ]]");
    }
```

  Run FAIL→PASS; reconcile to `old_seq`.

- [ ] **Step 3: `[[ ]]` as a pipeline / logical / negated / redirected stage — OBSERVATION.** Add probes and reconcile; determine by observation whether the atom compound-stage wiring already covers these (as it does for `if`/`while`):

```rust
    #[test]
    fn atoms_double_bracket_as_stage() {
        diff_cmd("[[ -f a ]] && echo yes");        // as && stage
        diff_cmd("[[ -f a ]] || echo no");         // as || stage
        diff_cmd("! [[ -f a ]]");                  // negated command
        diff_cmd("[[ -f a ]]; echo done");         // in a sequence
        diff_cmd("if [[ -n $x ]]; then echo y; fi"); // as an if condition
    }
```

  For any shape that does NOT `diff_cmd`-match because of stage/redirect wiring UNRELATED to `[[ ]]` itself (e.g. a compound-trailing-redirect gap shared with other compounds), document it inline as an out-of-scope observation and report it as a concern — do NOT force it. Trailing redirect (`[[ -f a ]] >out`) is such a case: include it only if it matches; otherwise note it.

- [ ] **Step 4: Full-suite + warnings gate, then commit.**

```bash
git add crates/huck-syntax/src/parser.rs
git commit -m "v253 T3: [[ … ]] inline assignments + stage integration + =~ deferral

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Adversarial corpus + error parity + gate

**Files:** Modify `crates/huck-syntax/src/parser.rs` (tests; plus any final reconciliation).

- [ ] **Step 1: Error-parity test.**

```rust
    #[test]
    fn atoms_double_bracket_errors() {
        // Both paths error the same way (parser-level).
        assert_eq!(new_seq("[[ ]]").is_err(), old_seq("[[ ]]").is_err());          // EmptyDoubleBracket
        assert_eq!(new_seq("[[ a == b").is_err(), old_seq("[[ a == b").is_err());  // UnterminatedDoubleBracket (EOF)
        assert_eq!(new_seq("[[ < b ]]").is_err(), old_seq("[[ < b ]]").is_err());   // MissingOperand (leading Op)
        assert_eq!(new_seq("[[ == b ]]").is_err(), old_seq("[[ == b ]]").is_err()); // `==` is a Word → lone-word then leftover `b` → Unterminated
        assert_eq!(new_seq("[[ -f ]]").is_err(), old_seq("[[ -f ]]").is_err());     // unary missing operand
        assert_eq!(new_seq("[[ a == ]]").is_err(), old_seq("[[ a == ]]").is_err()); // binary missing rhs
        // Same ERROR VARIANT where both are parser-level:
        assert_eq!(new_seq("[[ ]]"), old_seq("[[ ]]"));
        assert_eq!(new_seq("[[ a == b"), old_seq("[[ a == b"));
        assert_eq!(new_seq("[[ -f ]]"), old_seq("[[ -f ]]"));
    }
```

  If a case is a LEX-level reject on the oracle (`old_seq` panics via `.expect("lex")`), switch that line to the `is_err()`-on-both idiom used by `atoms_array_literal_error_parity` (grep it). Run and reconcile.

- [ ] **Step 2: Adversarial corpus.**

```rust
    #[test]
    fn atoms_double_bracket_corpus() {
        diff_cmd("[[ \"$x\" == \"$y\" ]]");          // quoted operands
        diff_cmd("[[ ${a[0]} -gt 0 ]]");             // subscript expansion operand
        diff_cmd("[[ $(cmd) == out ]]");             // command-sub operand
        diff_cmd("[[ a=b ]]");                       // `a=b` is ONE word (lone-word -n), NOT an assignment
        diff_cmd("[[ -n a=b ]]");
        diff_cmd("[[ x != y* ]]");                   // glob pattern RHS of !=
        diff_cmd("[[ -f 'a b' ]]");                  // quoted operand w/ space
        diff_cmd("[[ a\\ b == c ]]");                // escaped space in operand
        diff_cmd("[[ ! ( a == b ) || c ]]");         // ! before a group
        diff_cmd("[[ -e / ]]");
        diff_cmd("[[ -o errexit ]]");                // -o shell-option unary
    }
```

  Reconcile every divergence to `old_seq`. If a value legitimately differs due to an UNPORTED family inside an operand (e.g. an operand containing `=~`-unrelated deferred construct), use the established deferral posture, not a `diff_cmd`.

- [ ] **Step 3: Final gate.** `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` (all green); `cargo test -p huck-syntax --jobs 1 --doc -- --test-threads 1` (green); `cargo build -p huck-syntax 2>&1 | grep -c warning` (`0`); confirm `git diff main -- crates/huck-syntax/src/command.rs` shows ONLY `pub(crate)` visibility changes (no body/logic change); both `command_atoms` sites still `false`.

- [ ] **Step 4: Commit.**

```bash
git add crates/huck-syntax/src/parser.rs
git commit -m "v253 T4: [[ … ]] adversarial corpus + error parity + gate

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Notes carried from prior iterations (read before starting)

- **Byte-identical is the bar.** The differential corpus is the real spec; when a `diff_cmd` fails, print `new_seq(s)` and `old_seq(s)` and fix the ATOM path to match the oracle — never the oracle's logic.
- **Progress guarantee (v247 OOM hazard).** `parse_word_command`/`next_test_word_atom` are only called when the peeked atom is a genuine word atom (never a separator/operator/`]]`), so no zero-progress loop. If a test hangs, it is a non-progress bug, not the runner.
- **No mark/rewind.** This port needs none — the grammar is a straight recursive-descent pull. Do not add one.
- **`=~` must bail before reading the RHS.** The `"=~"` arm returns `Err(UnsupportedCommand)` WITHOUT calling `next_test_word_atom`, so the regex operand is never pulled/mis-lexed. This is the single most important deferral detail.
- **Operators are single `Lit` atoms.** Space-delimited `==`/`!=`/`=~`/`-eq`/… each lex to one unquoted `Lit`; `<`/`>` are `Op(RedirIn)`/`Op(RedirOut)`. If a `diff_cmd` shows an operator split across atoms, that is the bug to diagnose first.
