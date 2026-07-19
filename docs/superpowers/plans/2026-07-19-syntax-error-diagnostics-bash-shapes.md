# v314 — Top-level syntax-error 3-shape alignment Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace huck's single ad-hoc syntax-error message with bash 5.2.21's three canonical shapes on the top-level (`-c`/script/REPL) paths — naming the offending token, echoing the source line, and matching bash's line numbers.

**Architecture:** Two layers. Front (huck-syntax): an `expect_next_kind` cursor helper + an `ExpectFailure` capture struct + a new `ParseError::Unexpected` variant, populated at the syntax-error sites. Back (huck-engine): a central renderer that classifies a parse/lex error into one of bash's three shapes, spells the offending token/delimiter, and emits the 1- or 2-line form via an extended `emit_syntax_error`.

**Tech Stack:** Rust (huck-syntax, huck-engine crates), bash-diff harnesses (`tests/scripts/*_diff_check.sh`).

## Global Constraints

- **Issue:** [#211](https://github.com/jdstanhope/huck/issues/211). Spec: `docs/superpowers/specs/2026-07-19-syntax-error-diagnostics-bash-shapes-design.md`.
- **Decision A:** bash-exact syntax errors in all modes; huck's descriptive wording is retired. Byte-for-byte match with bash 5.2.21 is the bar.
- **Phasing:** v314 = TOP-LEVEL only. Nested context markers (`eval:`, `command substitution:`) are v315 and OUT OF SCOPE — do NOT touch the eval/comsub error paths' `-c:` marker.
- **THE RULE:** the lexer stays an atom source; `expect_next_kind` is a single-atom cursor op (peek→compare→advance-or-fail), never a forward scanner. The parser owns what is expected.
- **Non-goals:** arithmetic-EXPRESSION errors (`syntax error in expression`), `test`/`[` builtin errors, and the `[[ -n x` trailing second line.
- **Commit trailer (every commit):** `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- **Build/test on this box:** build the binary with `cargo build -p huck --bin huck` (+ `--release` for the sweep). Run crate tests single-threaded: `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` and `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`. NEVER `cargo test --workspace` (OOMs the box). Run any touched `-p huck` integration binary at `--test-threads 2`. `cargo fmt --all` before every commit.
- Use `/usr/bin/grep` (the bare `grep` is a broken shim on this box).

## Bash's three shapes (the target)

All prefixed `<name>: [-c: ]line N: ` (the `-c:` segment only for `-c` input; scripts omit it — existing `Diag::Syntax` machinery already does this).

1. **Near unexpected token** — two lines: `syntax error near unexpected token \`TOK'` then `` `SOURCE_LINE' `` (same prefix on both).
2. **Unexpected end of file** — one line: `syntax error: unexpected end of file`, at the EOF line (input `\n` count + 1).
3. **EOF looking for matching** — one line: `unexpected EOF while looking for matching \`DELIM'` (NO `syntax error:` prefix).

## File structure

- `crates/huck-syntax/src/command.rs` — add `ParseError::Unexpected(ExpectFailure)` + the `ExpectFailure`/`Found`/`Delim` types (they live with `ParseError`).
- `crates/huck-syntax/src/lexer.rs` — add `expect_next_kind` + `unexpected_here` on the token cursor.
- `crates/huck-syntax/src/spell.rs` — NEW: `spell_token`/`spell_delim` pure functions (bash spellings). Small, single-purpose.
- `crates/huck-syntax/src/parser.rs` — migrate the ~dozen Shape-1 `Err(...)` sites to `ParseError::Unexpected`.
- `crates/huck-engine/src/error_emit.rs` — extend `emit_syntax_error` (optional echo line + shape); add `render_syntax_diag`.
- `crates/huck-engine/src/shell.rs` — route the parse-error site (~line 478) through `render_syntax_diag`.
- `tests/scripts/syntax_error_diag_diff_check.sh` — NEW gold-gate harness.

---

### Task 1: Front-layer types + `ParseError::Unexpected` (no behavior change)

**Files:**
- Modify: `crates/huck-syntax/src/command.rs` (ParseError enum ~line 743)
- Modify: `crates/huck-syntax/src/errors.rs` (parse_error_message_impl ~line 27)
- Test: `crates/huck-syntax/src/command.rs` (inline `#[cfg(test)]`)

**Interfaces:**
- Produces: `ParseError::Unexpected(ExpectFailure)`; `struct ExpectFailure { found: Found, matching: Option<Delim>, pos: usize }`; `enum Found { Token(TokenKind), Eof }`; `enum Delim { Paren, Brace, DQuote, SQuote, Backtick, DollarParen, DollarDParen, DollarBrace, DBracket }`. All `pub`, `#[derive(Clone, Debug, PartialEq)]`.

- [ ] **Step 1: Add the types.** In `command.rs`, above `pub enum ParseError`, add:

```rust
#[derive(Clone, Debug, PartialEq)]
pub enum Found {
    Token(crate::lexer::TokenKind),
    Eof,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Delim {
    Paren,        // ( subshell
    Brace,        // { group
    DQuote,       // "
    SQuote,       // '
    Backtick,     // `
    DollarParen,  // $(
    DollarDParen, // $((
    DollarBrace,  // ${
    DBracket,     // [[
}

#[derive(Clone, Debug, PartialEq)]
pub struct ExpectFailure {
    pub found: Found,
    pub matching: Option<Delim>,
    pub pos: usize,
}
```

Add the variant to `ParseError`:

```rust
    /// v314 (#211): a syntax error carrying the offending token / EOF context,
    /// rendered into bash's near-token / unexpected-EOF shapes downstream.
    Unexpected(ExpectFailure),
```

- [ ] **Step 2: Give it a placeholder message** so the crate compiles. In `errors.rs`, in `parse_error_message_impl`, add an arm (final rendering lives in Task 3's engine renderer; this string is only a fallback for `Display`):

```rust
        ParseError::Unexpected(_) => "syntax error near unexpected token".to_string(),
```

- [ ] **Step 3: Add a unit test.** In `command.rs` `#[cfg(test)]`:

```rust
#[test]
fn expect_failure_roundtrips() {
    use crate::lexer::{Operator, TokenKind};
    let e = ParseError::Unexpected(ExpectFailure {
        found: Found::Token(TokenKind::Op(Operator::RParen)),
        matching: None,
        pos: 5,
    });
    assert!(matches!(e, ParseError::Unexpected(ref f) if f.pos == 5));
}
```

- [ ] **Step 4: Build + test.** Run: `cargo build -p huck-syntax && cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1 expect_failure_roundtrips`. Expected: builds, test PASSES.
- [ ] **Step 5: Commit.** `git add -A && git commit` (message: `v314 task 1: ParseError::Unexpected + ExpectFailure capture types`).

---

### Task 2: `expect_next_kind` + `unexpected_here` cursor helpers

**Files:**
- Create: `crates/huck-syntax/src/spell.rs`
- Modify: `crates/huck-syntax/src/lexer.rs` (near `peek_kind`/`next_kind`, ~line 5929; add `pub mod spell;` in `lib.rs`)
- Test: inline in `spell.rs` + `lexer.rs`

**Interfaces:**
- Produces: `Lexer::expect_next_kind(&mut self, expected: &[TokenKind]) -> Result<Token, ExpectFailure>`; `Lexer::unexpected_here(&mut self, matching: Option<Delim>) -> Result<ExpectFailure, LexError>`; `spell::spell_token(&TokenKind) -> String` (returns `"newline"` for `Newline`); `spell::spell_delim(Delim) -> char`.
- Consumes: Task 1's `ExpectFailure`/`Found`/`Delim`.

- [ ] **Step 1: Write `spell.rs`** — pure spelling of the tokens/delims bash names. Cover the operators and reserved words the probe exercises; unknown tokens fall back to `"newline"` (bash's word-position EOF spelling) is WRONG for arbitrary tokens, so return the literal where known and a debug fallback otherwise:

```rust
//! Bash spellings for tokens/delimiters named in syntax-error diagnostics.
use crate::lexer::{Operator, TokenKind, Word};

/// The string bash prints inside `near unexpected token `...'`.
pub fn spell_token(k: &TokenKind) -> String {
    match k {
        TokenKind::Newline => "newline".to_string(),
        TokenKind::Op(op) => spell_op(*op).to_string(),
        TokenKind::Word(w) => reserved_or_word(w),
        _ => "newline".to_string(), // word-position EOF/other → bash says `newline`
    }
}

fn spell_op(op: Operator) -> &'static str {
    match op {
        Operator::Pipe => "|",
        Operator::And => "&&",
        Operator::Or => "||",
        Operator::Semi => ";",
        Operator::Background => "&",
        Operator::LParen => "(",
        Operator::RParen => ")",
        Operator::DoubleSemi => ";;",
        Operator::SemiAmp => ";&",
        Operator::DoubleSemiAmp => ";;&",
        Operator::RedirReadWrite => "<>",
        Operator::RedirOut => ">",
        Operator::RedirAppend => ">>",
        Operator::RedirIn => "<",
        _ => "newline",
    }
}

/// A bare reserved word in command position (`done`, `esac`, `fi`, `then`,
/// `do`, `in`, `elif`, `else`, `}`) is named by bash literally.
fn reserved_or_word(w: &Word) -> String {
    // Word's flat literal text, if it is a single unquoted literal.
    match w.as_reserved_literal() {
        Some(s) => s.to_string(),
        None => "word".to_string(),
    }
}

pub fn spell_delim(d: crate::command::Delim) -> char {
    use crate::command::Delim::*;
    match d {
        Paren | DollarParen | DollarDParen => ')',
        Brace | DollarBrace => '}',
        DQuote => '"',
        SQuote => '\'',
        Backtick => '`',
        DBracket => ']', // rendered as `]]` by the caller
    }
}
```

NOTE: `Word::as_reserved_literal` may not exist — if not, add a small helper on `Word` that returns `Some(&str)` when the word is a single unquoted literal, else `None`. Check `crates/huck-syntax/src/word.rs` for the existing accessor first and reuse it.

- [ ] **Step 2: Add the cursor helpers** in `lexer.rs` right after `next_kind`:

```rust
/// Peek the NEXT atom; if its kind matches one of `expected`, consume and
/// return the whole Token, else return an ExpectFailure capturing what was
/// actually there. SINGLE ATOM ONLY — never scans ahead (THE RULE).
pub fn expect_next_kind(
    &mut self,
    expected: &[TokenKind],
) -> Result<Token, crate::command::ExpectFailure> {
    let pos = self.peek().ok().flatten().map(|t| t.span.offset).unwrap_or(self.byte_cursor());
    match self.peek_kind() {
        Ok(Some(k)) if expected.iter().any(|e| std::mem::discriminant(e) == std::mem::discriminant(k)) => {
            Ok(self.next().ok().flatten().expect("peeked token present"))
        }
        Ok(Some(_)) => {
            let found = self.peek().ok().flatten().map(|t| crate::command::Found::Token(t.kind.clone())).unwrap();
            Err(crate::command::ExpectFailure { found, matching: None, pos })
        }
        _ => Err(crate::command::ExpectFailure { found: crate::command::Found::Eof, matching: None, pos }),
    }
}

/// Capture the CURRENT peeked token (or EOF) as an ExpectFailure — for
/// "this token is invalid in this position" sites.
pub fn unexpected_here(
    &mut self,
    matching: Option<crate::command::Delim>,
) -> Result<crate::command::ExpectFailure, LexError> {
    let (found, pos) = match self.peek()? {
        Some(t) => (crate::command::Found::Token(t.kind.clone()), t.span.offset),
        None => (crate::command::Found::Eof, self.byte_cursor()),
    };
    Ok(crate::command::ExpectFailure { found, matching, pos })
}
```

If `Lexer` has no `byte_cursor()` accessor for the current byte offset, use the existing cursor accessor (`cursor_pos()` is referenced in `shell.rs:478`); reuse whichever exists.

- [ ] **Step 3: Register the module.** In `crates/huck-syntax/src/lib.rs` add `pub mod spell;`.
- [ ] **Step 4: Unit tests** in `spell.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ops_spelled_like_bash() {
        assert_eq!(spell_op(Operator::RParen), ")");
        assert_eq!(spell_op(Operator::DoubleSemi), ";;");
        assert_eq!(spell_op(Operator::Background), "&");
        assert_eq!(spell_op(Operator::Pipe), "|");
    }
    #[test]
    fn newline_token_spelled_newline() {
        assert_eq!(spell_token(&TokenKind::Newline), "newline");
    }
    #[test]
    fn delims_spelled_like_bash() {
        use crate::command::Delim;
        assert_eq!(spell_delim(Delim::DQuote), '"');
        assert_eq!(spell_delim(Delim::Backtick), '`');
        assert_eq!(spell_delim(Delim::DollarParen), ')');
    }
}
```

- [ ] **Step 5: Build + test + commit.** Run: `cargo build -p huck-syntax && cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1 spell`. Expected PASS. `cargo fmt --all`; commit (`v314 task 2: expect_next_kind + unexpected_here cursor helpers + spell module`).

---

### Task 3: The renderer + `emit_syntax_error` echo/shape support (huck-engine)

**Files:**
- Modify: `crates/huck-engine/src/error_emit.rs` (extend `emit_syntax_error` ~line 63; add `render_syntax_diag`)
- Test: inline `#[cfg(test)]` in `error_emit.rs`

**Interfaces:**
- Produces: `emit_syntax_error_ex(shell, line, body: Arguments, echo: Option<&str>)` (Shape 3 needs no `no_prefix` flag — its body already contains the full text after the prologue); `render_syntax_diag(shell, err: &ParseError, source: &str, token_line: u32)`.
- Consumes: `huck_syntax::spell::{spell_token, spell_delim}`, `huck_syntax::command::{ParseError, ExpectFailure, Found, Delim}`.

- [ ] **Step 1: Extend the emitter.** Keep `emit_syntax_error` as a thin wrapper; add the general form:

```rust
/// General syntax-error emitter. `echo` = the source line to reproduce on a
/// second `<prefix>`-line (Shape 1). `no_prefix` suppresses the `syntax error: `
/// body prefix (Shape 3 renders its own body). The `<name>: [-c: ]line N: `
/// prologue is always emitted by `error_prefix`.
pub fn emit_syntax_error_ex(
    shell: &Shell,
    line: u32,
    body: std::fmt::Arguments,
    echo: Option<&str>,
) {
    with_err(|err| {
        let prefix = shell.error_prefix(Diag::Syntax { line });
        let _ = write!(err, "{prefix}");
        let _ = err.write_fmt(body);
        let _ = err.write_all(b"\n");
        if let Some(src) = echo {
            let _ = write!(err, "{prefix}`{src}'\n");
        }
    });
}
```

Leave the existing `emit_syntax_error` in place (it now delegates: `emit_syntax_error_ex(shell, line, body, None)`).

- [ ] **Step 2: Add the classifier.** In `error_emit.rs`:

```rust
use huck_syntax::command::{Delim, ExpectFailure, Found, ParseError};
use huck_syntax::spell::{spell_delim, spell_token};

/// Classify a parse error into bash's three shapes and emit it.
/// `source` is the full input text; `token_line` is the pre-computed line of
/// the error position (Shape 1). v314: top-level only — no eval/comsub markers.
pub fn render_syntax_diag(shell: &Shell, err: &ParseError, source: &str, token_line: u32) {
    let eof_line = 1 + source.bytes().filter(|&b| b == b'\n').count() as u32;
    let echo_line = source_logical_line(source, token_line);
    match err {
        // Shape 1: a real token is present but misplaced.
        ParseError::Unexpected(f) if matches!(f.found, Found::Token(_)) => {
            let Found::Token(k) = &f.found else { unreachable!() };
            let tok = spell_token(k);
            emit_syntax_error_ex(
                shell, token_line,
                format_args!("syntax error near unexpected token `{tok}'"),
                Some(&echo_line),
            );
        }
        // Shape 3: EOF inside an open quote/delimiter.
        ParseError::Unexpected(ExpectFailure { found: Found::Eof, matching: Some(d), .. })
            if is_matching_delim(*d) =>
        {
            emit_matching(shell, *d, source);
        }
        ParseError::Lex(le) if lex_is_shape3(le).is_some() => {
            emit_matching(shell, lex_is_shape3(le).unwrap(), source);
        }
        // Shape 2: EOF while a keyword/paren construct is open.
        ParseError::UnterminatedIf
        | ParseError::UnterminatedLoop
        | ParseError::UnterminatedCase
        | ParseError::UnterminatedSubshell
        | ParseError::UnterminatedBrace
        | ParseError::UnterminatedFunction
        | ParseError::Unexpected(ExpectFailure { found: Found::Eof, .. }) => {
            emit_syntax_error_ex(shell, eof_line,
                format_args!("syntax error: unexpected end of file"), None);
        }
        // Fallback: keep the descriptive message (unmigrated / non-top-level).
        other => emit_syntax_error_ex(shell, token_line,
            format_args!("syntax error: {other}"), None),
    }
}

fn emit_matching(shell: &Shell, d: Delim, source: &str) {
    // Shape-3 line number: quote/`$((`/`${` → delimiter line; `$(` → EOF line.
    let eof_line = 1 + source.bytes().filter(|&b| b == b'\n').count() as u32;
    let line = match d {
        Delim::DollarParen | Delim::Paren => eof_line,
        _ => 1,
    };
    let spelled = spell_delim(d);
    // DBracket renders as `]]`.
    let matchtxt = if matches!(d, Delim::DBracket) { "]]".to_string() } else { spelled.to_string() };
    emit_syntax_error_ex(shell, line,
        format_args!("unexpected EOF while looking for matching `{matchtxt}'"), None);
    // Suppress the default `syntax error: ` body prefix by writing the whole body here.
}
```

Provide the small helpers `source_logical_line(source, line) -> String` (return the `line`-th 1-based line of `source`, trailing `\n` stripped), `is_matching_delim(Delim) -> bool` (true for quotes/`$(`/`$((`/`${`/backtick, false for Paren/Brace keyword constructs — but note `Paren` here means subshell → Shape 2, so exclude it), and `lex_is_shape3(&LexError) -> Option<Delim>` mapping `LexError::UnterminatedQuote → DQuote` (pick `'`/`"` — see NOTE), `UnterminatedSubstitution → DollarParen`, `UnterminatedArith/UnterminatedLegacyArith/UnterminatedArithBlock → DollarDParen`, `UnterminatedBrace → DollarBrace`.

NOTE (quote char): `LexError::UnterminatedQuote` may not record which quote. If it doesn't, thread the quote char into that variant OR default to `"` and refine in Task 5 against the harness (the harness has both `'` and `"` cases). Record the choice in the task report.

- [ ] **Step 3: Unit tests** — assert exact bytes for one case per shape (non-interactive, `-c`):

```rust
#[test]
fn shape1_near_token_with_echo() {
    // build a Shell in -c mode; render Unexpected{RParen} against source "echo )"
    // assert buf == b"huck: -c: line 1: syntax error near unexpected token `)'\n\
    //                  huck: -c: line 1: `echo )'\n"
}
#[test]
fn shape2_unexpected_eof() { /* UnterminatedIf, source "if true", assert `... line 2: syntax error: unexpected end of file\n` */ }
#[test]
fn shape3_matching_dquote() { /* Lex(UnterminatedQuote) source 'echo "hi', assert `... line 1: unexpected EOF while looking for matching `"'\n` */ }
```

Fill in the Shell setup mirroring `emit_syntax_error_carries_its_own_line` (set `is_interactive=false`, `is_command_string=true`, `shell_argv0="huck"`), and the `install_err_sinks` capture. Assert the exact byte strings above.

- [ ] **Step 4: Build + test.** `cargo build -p huck-engine && cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 shape`. Expected PASS.
- [ ] **Step 5: fmt + commit** (`v314 task 3: syntax-error renderer + 3-shape classifier`).

---

### Task 4: Wire Shape 1 (migrate parser sites + route the driver) + harness

**Files:**
- Modify: `crates/huck-syntax/src/parser.rs` (the Shape-1 `Err` sites listed below)
- Modify: `crates/huck-engine/src/shell.rs` (~line 478, replace `emit_syntax_error(... "syntax error: {e}")` with `render_syntax_diag(shell, &e, line, ln)`)
- Create: `tests/scripts/syntax_error_diag_diff_check.sh`
- Test: the harness

**Interfaces:**
- Consumes: Task 2 `unexpected_here`/`expect_next_kind`; Task 3 `render_syntax_diag`.

- [ ] **Step 1: Route the driver.** In `shell.rs`, replace the `emit_syntax_error(shell, ln, format_args!("syntax error: {e}"))` call (~line 478) with `crate::render_syntax_diag(shell, &e, line, ln)` (export `render_syntax_diag` from the engine root). Keep returning `ExecOutcome::Continue(2)`.

- [ ] **Step 2: Migrate the "current-token-invalid" Shape-1 sites** in `parser.rs`. Each of these currently returns a bare `ParseError::UnexpectedToken`; replace with a capture of the current token. The transformation pattern (apply to each site — the parser has `iter` in scope):

```rust
// BEFORE:
return Err(ParseError::UnexpectedToken);
// AFTER:
return Err(ParseError::Unexpected(iter.unexpected_here(None)?));
```

Sites (from `/usr/bin/grep -nE 'Err\(ParseError::UnexpectedToken\)' parser.rs`): **483, 2981, 3014, 3706, 4021, 4434, 4498, 4527, 4565, 4580**. Also the `MissingCommand`-at-operator sites that bash names as a token — **2988, 3095** (`return Err(ParseError::MissingCommand)`): these fire when an operator (`;;`, `&`, `|`) appears where a command is expected. Convert those two to `Err(ParseError::Unexpected(iter.unexpected_here(None)?))` **only if** the current token is a real token (operator); if the cursor is at EOF there, leave `MissingCommand` (that path is handled by the `unterminated` fallback at 4081). Guard: `if iter.peek_kind()?.is_some() { return Err(ParseError::Unexpected(iter.unexpected_here(None)?)); }`.

- [ ] **Step 3: Migrate `UnexpectedKeyword` + `UnexpectedBackground`.** `UnexpectedKeyword(kw)` (bare reserved word in command position, e.g. `done`/`esac`/`fi`/`then`/`do`/`in`) → `ParseError::Unexpected(iter.unexpected_here(None)?)` (the current token IS the keyword; `spell_token` names it). `UnexpectedBackground` (site 3648) → same. `MissingRedirectTarget` (sites 2701, 2721) → `ParseError::Unexpected(iter.unexpected_here(None)?)` (bash names the token after the redirect — `newline` when it's end-of-line, which `spell_token(Newline)` yields).

- [ ] **Step 4: Write the harness** `tests/scripts/syntax_error_diag_diff_check.sh` (model on `readonly_assign_discard_diff_check.sh` — run both shells, normalize only each shell's own name prefix, diff stderr + rc):

```bash
#!/usr/bin/env bash
# v314 (#211): huck's top-level syntax-error diagnostics match bash's 3 shapes.
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: build with cargo build -p huck" >&2; exit 1; }
FAIL=0
norm() { sed -E "s#^(bash|.*/huck|huck): #SH: #"; }
check() {
  local label=$1 frag=$2 b h br hr
  b=$(bash -c "$frag" 2>&1 >/dev/null | norm); br=$?
  h=$("$HUCK" -c "$frag" 2>&1 >/dev/null | norm); hr=$?
  if [ "$b" != "$h" ]; then echo "FAIL [$label]"; echo "  bash: [$b]"; echo "  huck: [$h]"; FAIL=1
  else echo "PASS [$label]"; fi
}
# Shape 1 — near unexpected token
check s1-rparen   'echo )'
check s1-dsemi    'echo a ;; echo b'
check s1-done     'done'
check s1-esac     'esac'
check s1-fi       'fi'
check s1-then     'then echo x'
check s1-caseesac 'case esac in esac) ;; esac'
check s1-amp      '& echo x'
check s1-pipe     '| echo x'
check s1-lessgt   'echo <>'
check s1-in       'for x in ; do :; done; in'
check s1-do       'do echo x'
# Shape 2 — unexpected end of file
check s2-subshell '( echo hi'
check s2-brace    '{ echo hi'
check s2-if       'if true'
check s2-then     'if true; then echo'
check s2-case     'case x in'
check s2-for      'for i in 1 2'
check s2-while    'while true'
# Shape 3 — EOF looking for matching
check s3-dquote   'echo "hi'
check s3-squote   "echo 'hi"
check s3-cmdsub   'echo $(foo'
check s3-arith    'echo $((1+'
check s3-paramexp 'echo ${x'
if [ $FAIL -ne 0 ]; then echo "syntax_error_diag_diff_check FAILED" >&2; exit 1; fi
echo "syntax_error_diag_diff_check OK"
```

Make it executable: `chmod +x tests/scripts/syntax_error_diag_diff_check.sh`.

- [ ] **Step 5: Build + run harness.** `cargo build -p huck --bin huck && bash tests/scripts/syntax_error_diag_diff_check.sh`. Expected: all **Shape 1** cases PASS. Shape 2/3 cases may still FAIL (wired in Task 5) — that is expected at this task; note which pass. If a Shape-1 case fails, the huck error site detected the error at a different token than bash — record it; small per-site adjustments (which token `unexpected_here` captures) are in scope here.
- [ ] **Step 6: fmt + commit** (`v314 task 4: wire Shape 1 (near-token) + diff harness`).

---

### Task 5: Wire Shape 2 & Shape 3 (unterminated constructs)

**Files:**
- Modify: `crates/huck-engine/src/error_emit.rs` (refine `lex_is_shape3` + quote-char threading if needed)
- Modify: `crates/huck-syntax/src/lexer.rs` (if `UnterminatedQuote` must carry its quote char — add the field)
- Test: the harness (Shape 2/3 sections)

- [ ] **Step 1: Run the harness** to see current Shape 2/3 diffs: `bash tests/scripts/syntax_error_diag_diff_check.sh`. The `render_syntax_diag` classifier from Task 3 already routes `UnterminatedIf/Loop/Case/Subshell/Brace` → Shape 2 and `Lex(Unterminated*)` → Shape 3, so most should already pass once the driver (Task 4) routes through it. Record failures.
- [ ] **Step 2: Fix Shape-2 line number.** Confirm Shape 2 reports the EOF line (input `\n` count + 1). For one-line `-c` that is line 2 — matches `emit`'s `eof_line`. If huck emits line 1, the classifier's `eof_line` computation or the `Diag::Syntax{line}` value is being overridden upstream; ensure `render_syntax_diag` passes `eof_line` (not the parser's token line) for Shape 2. Re-run harness; `s2-*` PASS.
- [ ] **Step 3: Fix Shape-3 quote char.** If `s3-dquote` prints `` `'' `` or vice-versa, thread the quote char: add `UnterminatedQuote(char)` to `LexError` (or a bool `double`), set it where the lexer raises the unterminated-quote error, and map it in `lex_is_shape3` to `Delim::DQuote`/`Delim::SQuote`. Re-run harness; `s3-dquote`/`s3-squote` PASS.
- [ ] **Step 4: Backtick + cmdsub delimiters.** `s3-cmdsub` (`echo $(foo`) must say matching `` `)' `` at line 2; `s3-arith`/`s3-paramexp` at line 1. Adjust `emit_matching`'s per-`Delim` line map if any diverge. The backtick case (`` echo `foo ``) is a KNOWN fiddly bit (huck normalizes backtick to cmdsub) — if it can't be made to say `` ` `` without threading the delimiter kind through the backtick lexer, leave it failing, note it in the report, and file a follow-on issue (do NOT expand scope to fix the backtick lexer here).
- [ ] **Step 5: Build + run full harness.** `cargo build -p huck --bin huck && bash tests/scripts/syntax_error_diag_diff_check.sh`. Expected: all cases PASS except any explicitly-deferred backtick case.
- [ ] **Step 6: fmt + commit** (`v314 task 5: wire Shape 2 (unexpected EOF) + Shape 3 (matching delimiter)`).

---

### Task 6: Update existing tests + sweep + baseline

**Files:**
- Modify: huck unit tests asserting the retired wording (find with grep below)
- Modify: `tests/scripts/run_diff_checks.sh` harnesses that pin old wording (e.g. `error_message_diff_check.sh`)
- Modify: `docs/bash-test-suite-baseline.md` (record movement)

- [ ] **Step 1: Find retired-wording assertions.** Run:
`/usr/bin/grep -rnE "unexpected token after command|unterminated 'if'|unterminated 'case'|unterminated loop|unterminated quote|unterminated '\\\$\{|unterminated '\\('" crates/ tests/`
For each hit, update the expected string to the new bash-exact form (run the fragment through bash 5.2.21 to get the exact text). These are behavior-confirming updates, not new tests.

- [ ] **Step 2: Run the full crate test suites.** `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` and `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`. Fix any remaining assertions. Expected: green.
- [ ] **Step 3: Run touched integration binaries** at `--test-threads 2`: any `-p huck` binary that asserts syntax-error output (grep `tests/*.rs` for the retired strings). Expected: green.
- [ ] **Step 4: Full diff-check sweep.** Build both binaries (`cargo build -p huck --bin huck` + `cargo build --release -p huck --bin huck`), then `ulimit -v 1500000; timeout 600 bash tests/scripts/run_diff_checks.sh`. Expected: green including the new `syntax_error_diag_diff_check.sh`.
- [ ] **Step 5: Re-sweep affected bash-suite categories.** `export BASH_SOURCE_DIR=/tmp/bash-5.2.21; for c in parser errors comsub comsub-posix array; do HUCK_BASH_TEST_CATEGORY=$c bash tests/bash-test-suite/runner.sh 2>&1 | /usr/bin/grep -E "\\| $c \\|"; done`. Record which improved/flipped.
- [ ] **Step 6: Update baseline doc + commit.** Note v314's syntax-error alignment and any category movement in `docs/bash-test-suite-baseline.md`. `cargo fmt --all`; commit (`v314 task 6: update tests + harnesses + baseline for 3-shape syntax errors`).

---

## Notes for the executor

- **posix2 will NOT flip** in v314 (needs v315's `eval:` marker). That is expected — do not chase it.
- If a Shape-1 site names a different token than bash (because huck's parser detects the error one token earlier/later), that is the interesting case: adjust WHERE `unexpected_here` is called or WHICH token it captures, guided by the harness. Do not paper over it by special-casing the message.
- Keep the fallback arm in `render_syntax_diag` (descriptive `syntax error: {other}`) — non-migrated / non-top-level errors must still render sanely.
