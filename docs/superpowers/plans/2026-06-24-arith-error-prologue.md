# v216 — Bash error-prologue foundation + arith error-text slice — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make huck's arithmetic error messages byte-match bash 5.2.21 for the cases where both shells error, by (a) building a reusable bash-compatible error-prologue helper and (b) routing the arith error sites through it with bash's expression-echo, message wording, and `(error token is "…")` suffix.

**Architecture:** A new `Shell::error_prefix(cmd)` produces bash's `<name>: [line N: ][cmd: ]` prologue (interactive vs non-interactive, `$0`/`BASH_SOURCE[0]`, `current_lineno`). `arith.rs` gains per-token byte-offset tracking so a failed parse/eval can report the leading-trimmed expression and the `lasttp`-style error token (`src[offset..]`). A pure `render_error_body(src, &ArithError)` assembles the post-prologue body; the four arith emission sites (`$(( ))`, `(( ))`, `let`, substring) call `error_prefix` + `render_error_body` with their command context.

**Tech Stack:** Rust (workspace crates `huck-engine`, `huck-syntax`); bash-diff harness shell scripts under `tests/scripts/`.

## Global Constraints

- Commit trailer on every commit, verbatim: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- Work on branch `v216-arith-error-prologue` (create from `main`; do NOT push or merge without confirmation).
- Bash reference version is **5.2.21**; `$BASH_SOURCE_DIR=/tmp/bash-5.2.21` and a `bash` on PATH are assumed for the diff-check harness.
- huck error tail (`cd: msg`, `let: msg`) already matches bash; v216 changes **only the prologue** and **only the arith sites** — the other ~400 `huck:` sites are untouched (later iterations).
- Interactive-mode arith errors must keep the `huck:`-style prologue (no interactive-test churn).
- **In scope = both-shells-error cases only.** Behavioral divergences (`++7`/`--7`, lazy dead-branch eval, array-element lvalues, integer overflow wrapping, substring ternary colons, `$var` literal in arith) are OUT of scope and excluded from the harness.
- Exact bash message strings (copied from `expr.c`, not from GPL'd `.right` output):
  `attempted assignment to non-variable`, `division by 0`, `invalid arithmetic base`,
  `invalid integer constant`, `value too great for base`, `invalid number`,
  `` missing `)' ``, `syntax error: operand expected`, `expression expected`,
  `` `:' expected for conditional expression ``, `syntax error in expression`,
  `bad array subscript`.

---

### Task 1: `Shell::error_prefix` prologue helper

**Files:**
- Modify: `crates/huck-engine/src/shell_state.rs` (add method near `lookup_var`, ~line 852; `get_indexed` exists at line 2001, `shell_argv0` at 451, `is_interactive` at 474, `current_lineno` at 575)
- Test: same file's `#[cfg(test)]` module

**Interfaces:**
- Produces: `pub fn error_prefix(&self, cmd: Option<&str>) -> String` on `Shell`.
  Output shape: `"<name>: [line <N>: ][<cmd>: ]"`.
  - `name` = when `!is_interactive`: `BASH_SOURCE[0]` if present & non-empty, else `shell_argv0`; when interactive: `"huck"`.
  - ` line <N>: ` only when `!is_interactive` && `current_lineno > 0`.
  - `<cmd>: ` only when `cmd` is `Some`.

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)]` module in `shell_state.rs`:

```rust
#[test]
fn error_prefix_noninteractive_script_with_line_and_cmd() {
    let mut sh = Shell::new_for_test();
    sh.is_interactive = false;
    sh.shell_argv0 = "./arith.tests".to_string();
    sh.current_lineno = 168;
    assert_eq!(sh.error_prefix(None), "./arith.tests: line 168: ");
    assert_eq!(sh.error_prefix(Some("let")), "./arith.tests: line 168: let: ");
    assert_eq!(sh.error_prefix(Some("((")), "./arith.tests: line 168: ((: ");
}

#[test]
fn error_prefix_interactive_keeps_huck_no_line() {
    let mut sh = Shell::new_for_test();
    sh.is_interactive = true;
    sh.shell_argv0 = "huck".to_string();
    sh.current_lineno = 5;
    assert_eq!(sh.error_prefix(None), "huck: ");
    assert_eq!(sh.error_prefix(Some("((")), "huck: ((: ");
}

#[test]
fn error_prefix_prefers_bash_source_zero() {
    let mut sh = Shell::new_for_test();
    sh.is_interactive = false;
    sh.shell_argv0 = "huck".to_string();
    sh.current_lineno = 3;
    sh.set_indexed_var("BASH_SOURCE", std::iter::once((0usize, "./sourced.sh".to_string())).collect());
    assert_eq!(sh.error_prefix(None), "./sourced.sh: line 3: ");
}
```

Confirm the test-constructor name: if `Shell::new_for_test()` does not exist, grep the test module for the constructor other tests use (e.g. `Shell::default()` / a local helper) and use that instead. Confirm `set_indexed_var` signature at line ~2100 (`set_indexed_var(&mut self, name, BTreeMap<usize,String>)`).

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p huck-engine error_prefix 2>&1 | tail -20`
Expected: FAIL — `no method named error_prefix`.

- [ ] **Step 3: Implement `error_prefix`**

Add to `impl Shell` (near `lookup_var`):

```rust
/// Bash-compatible error prologue: `<name>: [line N: ][cmd: ]`.
/// Mirrors bash `get_name_for_error` + `error_prolog`/`builtin_error_prolog`.
/// `cmd` is the command context (`let`, `((`) or `None` for `$(( ))`.
pub fn error_prefix(&self, cmd: Option<&str>) -> String {
    let name = if !self.is_interactive {
        self.get_indexed("BASH_SOURCE")
            .and_then(|m| m.get(&0))
            .filter(|s| !s.is_empty())
            .cloned()
            .unwrap_or_else(|| self.shell_argv0.clone())
    } else {
        "huck".to_string()
    };
    let mut out = format!("{name}: ");
    if !self.is_interactive && self.current_lineno > 0 {
        out.push_str(&format!("line {}: ", self.current_lineno));
    }
    if let Some(c) = cmd {
        out.push_str(&format!("{c}: "));
    }
    out
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p huck-engine error_prefix 2>&1 | tail -20`
Expected: PASS (3 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/huck-engine/src/shell_state.rs
git commit -m "v216 task 1: Shell::error_prefix bash-compatible error prologue

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: `ArithError` carries an offset + exposes bash-message text

**Files:**
- Modify: `crates/huck-engine/src/arith.rs` (`ArithError` enum at 405-414, `Display` at 416-429; all construction sites in the file)
- Test: `arith.rs` `#[cfg(test)]` module

**Interfaces:**
- Produces:
  - `ArithError` becomes `pub struct ArithError { pub kind: ArithErrorKind, pub offset: Option<usize> }`.
  - `pub enum ArithErrorKind` = the OLD variants (`Parse(String)`, `DivisionByZero`, `ModuloByZero`, `NotAnInteger{var,value}`, `NegativeExponent`, `ShiftCountOutOfRange{count}`, `ReadonlyVar(String)`) PLUS new parse-kind variants carrying bash wording directly (see below).
  - `ArithError::bash_message(&self) -> String` — the post-`<expr>:` text bash prints.
  - Helper constructors: `ArithError::parse(msg)` (offset `None`), used by existing non-positioned sites.
  - `Display` for `ArithError` delegates to `kind` and reproduces the OLD text (so any non-arith caller and existing `{e}` formatting still compile and existing non-updated assertions keep their text until Task 5 rewrites the emission).

To keep wording explicit and avoid string-matching later, introduce dedicated kinds for the bash-mapped parse errors:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArithErrorKind {
    // Legacy free-form parse message (kept for sites not yet bash-mapped).
    Parse(String),
    // Bash-mapped parse/lex errors (each renders a fixed bash string):
    AssignToNonVar,          // "attempted assignment to non-variable"
    InvalidBase,             // "invalid arithmetic base"
    InvalidIntegerConstant,  // "invalid integer constant"
    ValueTooGreatForBase,    // "value too great for base"
    InvalidNumber,           // "invalid number"
    MissingCloseParen,       // "missing `)'"
    OperandExpected,         // "syntax error: operand expected"
    ExpressionExpected,      // "expression expected"
    ColonExpected,           // "`:' expected for conditional expression"
    SyntaxErrorInExpression, // "syntax error in expression"
    BadArraySubscript,       // "bad array subscript"
    // Eval-time:
    DivisionByZero,
    ModuloByZero,
    NotAnInteger { var: String, value: String },
    NegativeExponent,
    ShiftCountOutOfRange { count: i64 },
    ReadonlyVar(String),
}
```

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn arith_error_bash_message_mapping() {
    use ArithErrorKind::*;
    let mk = |k| ArithError { kind: k, offset: None };
    assert_eq!(mk(AssignToNonVar).bash_message(), "attempted assignment to non-variable");
    assert_eq!(mk(DivisionByZero).bash_message(), "division by 0");
    assert_eq!(mk(InvalidBase).bash_message(), "invalid arithmetic base");
    assert_eq!(mk(InvalidIntegerConstant).bash_message(), "invalid integer constant");
    assert_eq!(mk(ValueTooGreatForBase).bash_message(), "value too great for base");
    assert_eq!(mk(MissingCloseParen).bash_message(), "missing `)'");
    assert_eq!(mk(OperandExpected).bash_message(), "syntax error: operand expected");
    assert_eq!(mk(ExpressionExpected).bash_message(), "expression expected");
    assert_eq!(mk(ColonExpected).bash_message(), "`:' expected for conditional expression");
    assert_eq!(mk(SyntaxErrorInExpression).bash_message(), "syntax error in expression");
    assert_eq!(mk(InvalidNumber).bash_message(), "invalid number");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p huck-engine arith_error_bash_message_mapping 2>&1 | tail -20`
Expected: FAIL — `ArithError` is an enum / no field `kind` / no `bash_message`.

- [ ] **Step 3: Refactor `ArithError` to a struct + add `bash_message` + keep `Display`**

Replace the `ArithError` enum (405-414) and its `Display` (416-429) with:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArithError {
    pub kind: ArithErrorKind,
    /// Byte offset into the parsed source of the error token (bash `lasttp`).
    pub offset: Option<usize>,
}

impl ArithError {
    /// Legacy free-form parse error, no position.
    pub fn parse(msg: impl Into<String>) -> Self {
        ArithError { kind: ArithErrorKind::Parse(msg.into()), offset: None }
    }
    /// A bash-mapped error with a token offset.
    pub fn at(kind: ArithErrorKind, offset: usize) -> Self {
        ArithError { kind, offset: Some(offset) }
    }
    /// The text bash prints after the `<expr>: ` echo.
    pub fn bash_message(&self) -> String {
        use ArithErrorKind::*;
        match &self.kind {
            Parse(m) => m.clone(),
            AssignToNonVar => "attempted assignment to non-variable".into(),
            InvalidBase => "invalid arithmetic base".into(),
            InvalidIntegerConstant => "invalid integer constant".into(),
            ValueTooGreatForBase => "value too great for base".into(),
            InvalidNumber => "invalid number".into(),
            MissingCloseParen => "missing `)'".into(),
            OperandExpected => "syntax error: operand expected".into(),
            ExpressionExpected => "expression expected".into(),
            ColonExpected => "`:' expected for conditional expression".into(),
            SyntaxErrorInExpression => "syntax error in expression".into(),
            BadArraySubscript => "bad array subscript".into(),
            DivisionByZero | ModuloByZero => "division by 0".into(),
            NotAnInteger { value, .. } => format!("{value}: syntax error: operand expected"),
            NegativeExponent => "exponent less than 0".into(),
            ShiftCountOutOfRange { .. } => "shift count out of range".into(),
            ReadonlyVar(name) => format!("{name}: readonly variable"),
        }
    }
}

impl std::fmt::Display for ArithError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Legacy rendering kept so existing `{e}` callers compile unchanged
        // until Task 5 swaps the emission sites to bash formatting.
        use ArithErrorKind::*;
        match &self.kind {
            Parse(m) => write!(f, "{m}"),
            DivisionByZero => write!(f, "division by zero"),
            ModuloByZero => write!(f, "modulo by zero"),
            NotAnInteger { var, value } =>
                write!(f, "variable '{var}' is not an integer: '{value}'"),
            NegativeExponent => write!(f, "exponentiation with negative exponent"),
            ShiftCountOutOfRange { count } => write!(f, "shift count out of range: {count}"),
            ReadonlyVar(name) => write!(f, "{name}: readonly variable"),
            // Bash-mapped kinds render their bash text under Display too.
            other => write!(f, "{}", ArithError { kind: other.clone(), offset: None }.bash_message()),
        }
    }
}
```

Then update **every** existing constructor in `arith.rs` to the struct form. These are mechanical; the transformation is `ArithError::Parse(x)` → `ArithError::parse(x)` and `ArithError::DivisionByZero` → `ArithError { kind: ArithErrorKind::DivisionByZero, offset: None }`. Apply to all sites, e.g.:
- `arith.rs:49,52,88,95,102,125,127,139,144,163,328` (tokenize/number helpers) → `ArithError::parse(...)` (offsets added in Task 3; leave `parse` for now).
- `arith.rs:437,519,537,552,607,614,653,664,672,677,680` (parser) → `ArithError::parse(...)`.
- `arith.rs:694,722` (`NotAnInteger`), `795,801,843,851,859` (eval div/mod/shift/exp), `703,755,760` (`ReadonlyVar`) → struct form `ArithError { kind: ..., offset: None }`.

Search to confirm none missed:
`grep -n "ArithError::" crates/huck-engine/src/arith.rs` — every hit must be `ArithError::parse`, `ArithError::at`, or `ArithError { kind: ... }`.

- [ ] **Step 4: Run the full arith + engine test suite**

Run: `cargo test -p huck-engine arith 2>&1 | tail -25`
Expected: PASS — the new mapping test passes and all pre-existing arith tests still pass (Display text unchanged for the legacy kinds they assert).

- [ ] **Step 5: Commit**

```bash
git add crates/huck-engine/src/arith.rs
git commit -m "v216 task 2: ArithError struct with offset + bash_message mapping

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Token byte-offsets + positioned parse errors + `render_error_body`

**Files:**
- Modify: `crates/huck-engine/src/arith.rs` (`tokenize` 109-333, `parse`/`Parser` 432-683)
- Test: `arith.rs` `#[cfg(test)]` module

**Interfaces:**
- Consumes: `ArithError::at(kind, offset)`, `ArithErrorKind::*`, `bash_message` (Task 2).
- Produces:
  - `tokenize(input) -> Result<(Vec<ArithToken>, Vec<usize>), ArithError>` — `offsets[i]` = byte offset of `tokens[i]`'s first char. Tokenize-time errors use `ArithError::at(kind, start_of_offending_number)`.
  - `Parser` gains `offsets: Vec<usize>` and `err_off: usize`; `bump()` sets `err_off` to the consumed token's offset.
  - `pub fn render_error_body(src: &str, err: &ArithError) -> String` →
    `"<expr-leading-trimmed>: <bash_message> (error token is \"<tok>\")"`, where
    `<tok>` = `src[err.offset.unwrap_or(src.len())..]` (empty string when offset is `None` or at/after end).

Implementation notes for offsets: convert `tokenize` to index by byte position. Replace the `chars.peek()`/`chars.next()` iterator with a `let bytes = input.as_bytes();` + `let mut i = 0usize;` index loop, or wrap the existing loop by tracking a running byte cursor. Record `let start = i;` at the top of each token branch and `offsets.push(start)` next to each `out.push(token)`. Arith input is ASCII in practice (operators, digits, identifiers); a byte cursor is safe — but guard identifiers/`$` with `input[start..].char_indices()` if a non-ASCII byte appears, falling back to `ArithError::parse("unexpected character")` as today.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn tokenize_reports_offsets() {
    let (toks, offs) = tokenize("7 = 43 ").unwrap();
    assert_eq!(toks.len(), offs.len());
    // tokens: 7@0, =@2, 43@4
    assert_eq!(offs, vec![0, 2, 4]);
}

#[test]
fn render_assign_to_nonvar() {
    // `$(( 7 = 43 ))` inner text, untrimmed
    let err = parse(" 7 = 43 ").unwrap_err();
    assert_eq!(err.bash_message(), "attempted assignment to non-variable");
    assert_eq!(render_error_body(" 7 = 43 ", &err),
        "7 = 43 : attempted assignment to non-variable (error token is \"= 43 \")");
}

#[test]
fn render_operand_expected_at_eof() {
    let err = parse(" 4 + ").unwrap_err();
    assert_eq!(render_error_body(" 4 + ", &err),
        "4 + : syntax error: operand expected (error token is \"+ \")");
}

#[test]
fn render_missing_close_paren() {
    let err = parse("rv = 7 + (43 * 6").unwrap_err();
    assert_eq!(render_error_body("rv = 7 + (43 * 6", &err),
        "rv = 7 + (43 * 6: missing `)' (error token is \"6\")");
}

#[test]
fn render_trailing_junk() {
    let err = parse("a b").unwrap_err();
    assert_eq!(render_error_body("a b", &err),
        "a b: syntax error in expression (error token is \"b\")");
}

#[test]
fn render_invalid_base_and_constants() {
    assert_eq!(render_error_body("3425#56", &parse("3425#56").unwrap_err()),
        "3425#56: invalid arithmetic base (error token is \"3425#56\")");
    assert_eq!(render_error_body("2#", &parse("2#").unwrap_err()),
        "2#: invalid integer constant (error token is \"2#\")");
    assert_eq!(render_error_body("2#44", &parse("2#44").unwrap_err()),
        "2#44: value too great for base (error token is \"2#44\")");
}

#[test]
fn render_ternary_branches() {
    assert_eq!(render_error_body("4 ? : 3 + 5", &parse("4 ? : 3 + 5").unwrap_err()),
        "4 ? : 3 + 5: expression expected (error token is \": 3 + 5\")");
}
```

(Note: these assert the leading-trim + tail-token semantics. `parse` must return the positioned error.)

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p huck-engine 'render_' 2>&1 | tail -25` and `cargo test -p huck-engine tokenize_reports_offsets 2>&1 | tail`
Expected: FAIL — `tokenize` returns a `Vec`, not a tuple; `render_error_body` undefined.

- [ ] **Step 3: Thread offsets through `tokenize`**

Change the signature to `pub(crate) fn tokenize(input: &str) -> Result<(Vec<ArithToken>, Vec<usize>), ArithError>`. Maintain a byte cursor `i`. At each token push, also `offsets.push(start)`. For the number branch, capture `let num_start = i;` before reading digits and use `ArithError::at(kind, num_start)` for the four number errors:
- base `> 64` → `ArithErrorKind::InvalidBase`
- base parse failure or base `< 2` or a stray second `#` → `ArithErrorKind::InvalidNumber`
- no digits after `#` (`2#`) → `ArithErrorKind::InvalidIntegerConstant`
- a digit `>=` base (`2#44`) → `ArithErrorKind::ValueTooGreatForBase`

Update `parse_base_n_digits`/`parse_hex_digits` to return the kind (or signal via the caller). Keep the legacy octal/`integer literal out of range` as `ArithError::parse(...)` (overflow is out of scope — those literals are excluded from the harness).

Return `Ok((out, offsets))` at the end.

- [ ] **Step 4: Thread offsets + `err_off` through `Parser`; positioned errors; `render_error_body`**

In `parse`:

```rust
pub fn parse(input: &str) -> Result<ArithExpr, ArithError> {
    let (tokens, offsets) = tokenize(input)?;
    let mut p = Parser { tokens, offsets, pos: 0, err_off: 0 };
    let expr = p.parse_comma_expr()?;
    if p.pos < p.tokens.len() {
        let off = p.offsets[p.pos];
        return Err(ArithError::at(ArithErrorKind::SyntaxErrorInExpression, off));
    }
    Ok(expr)
}

struct Parser {
    tokens: Vec<ArithToken>,
    offsets: Vec<usize>,
    pos: usize,
    err_off: usize,
}
```

In `bump`, record the consumed token's offset (bash `lasttp` is set only for non-EOF tokens):

```rust
fn bump(&mut self) -> Option<ArithToken> {
    let t = self.tokens.get(self.pos).cloned();
    if t.is_some() {
        self.err_off = self.offsets[self.pos];
    }
    self.pos += 1;
    t
}
```

Add a small helper to build positioned errors from the current `err_off`:

```rust
fn fail(&self, kind: ArithErrorKind) -> ArithError {
    ArithError::at(kind, self.err_off)
}
```

Replace the parser's bash-mappable `return Err(ArithError::Parse(...))` sites with `self.fail(...)`:
- assignment-to-nonvar (537-539) → `return Err(self.fail(ArithErrorKind::AssignToNonVar))`
  (the `=` was consumed by the `self.bump()` at 534, so `err_off` is the `=` offset — matches bash).
- ternary missing colon (552-554) → `return Err(self.fail(ArithErrorKind::ColonExpected))`.
- paren close (672-674) → `Err(self.fail(ArithErrorKind::MissingCloseParen))`.
- `parse_prefix` operand cases:
  - `Some(ArithToken::Colon)` reached via `Some(t) => "expected expression, got {t}"` (677-679): when the unexpected primary token is `Colon`, map to `ExpressionExpected`; otherwise map to `OperandExpected`. Implement as:
    ```rust
    Some(t) => {
        let kind = if matches!(t, ArithToken::Colon) {
            ArithErrorKind::ExpressionExpected
        } else {
            ArithErrorKind::OperandExpected
        };
        Err(self.fail(kind))
    }
    ```
  - `None => "unexpected end of input"` (680) → `Err(self.fail(ArithErrorKind::OperandExpected))`
    (at EOF, `bump` did not update `err_off`, so it points at the last real token — matches bash's `+ `).
- subscript errors (607-609, 614-616) → leave as `ArithError::parse(...)` for now (array-subscript paths are largely behavioral/out of scope) OR map the `bad array subscript` read path to `BadArraySubscript` if trivially reachable; not required for the harness.
- postfix-on-nonlvalue (519-521) and prefix `++`/`--` (653-655, 663-665): these are the **behavioral** `7--`/`++7` cases — leave their messages as `ArithError::parse(...)` (unchanged); excluded from the harness.

Add `render_error_body` (free function, after `parse`):

```rust
/// Builds bash's post-prologue error body:
/// "<expr>: <msg> (error token is \"<tok>\")", where <expr> is `src`
/// with leading whitespace trimmed and <tok> is `src[offset..]`.
pub fn render_error_body(src: &str, err: &ArithError) -> String {
    let expr = src.trim_start();
    let tok = match err.offset {
        Some(off) if off <= src.len() => &src[off..],
        _ => "",
    };
    format!("{expr}: {} (error token is \"{tok}\")", err.bash_message())
}
```

- [ ] **Step 5: Fix the other `tokenize` call site**

`grep -n "tokenize(" crates/huck-engine/src/arith.rs` — `parse` is the only non-test caller. Update any test that called `tokenize` expecting a bare `Vec` to destructure the tuple (e.g. the existing `tokenize_number_overflow_is_parse_error` at ~976 and any others): `let (toks, _offs) = tokenize(...)...`.

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p huck-engine arith 2>&1 | tail -30`
Expected: PASS — new `render_*`/`tokenize_reports_offsets` tests pass; pre-existing arith parser tests still pass (they assert via `Display`/eval results, unaffected).

- [ ] **Step 7: Commit**

```bash
git add crates/huck-engine/src/arith.rs
git commit -m "v216 task 3: arith token offsets + positioned errors + render_error_body

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Division/modulo-by-zero error token (eval-time positioning)

**Files:**
- Modify: `crates/huck-engine/src/arith.rs` (`ArithExpr::Div`/`Mod` 373-374, the Pratt table 588-590, compound `/=`/`%=` eval 871-878, eval `Div`/`Mod` 793-802)
- Test: `arith.rs` `#[cfg(test)]` module

**Interfaces:**
- Consumes: `Parser.err_off`, `ArithError::at`, `render_error_body`.
- Produces: `Div`/`Mod` AST nodes carry the divisor token's byte offset so a by-zero error reports `(error token is "<divisor-tail>")`.

Rationale: huck evaluates after parsing, so eval-time errors otherwise lose position. `44 / 0` is a headline both-error case; carry the divisor offset on just these two nodes.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn render_division_by_zero_token() {
    let err = parse("44 / 0 ").unwrap_err_on_eval(); // see helper note below
    assert_eq!(render_error_body("44 / 0 ", &err),
        "44 / 0 : division by 0 (error token is \"0 \")");
}
```

Since by-zero is detected in `eval` (needs a `Shell`), write the test against eval directly using the test shell helper (mirror an existing eval test in the module, e.g. `eval_overflow_wraps` at ~1535 shows the eval-test pattern). Concretely:

```rust
#[test]
fn render_division_by_zero_token() {
    let mut sh = Shell::new_for_test();
    let expr = parse("44 / 0 ").unwrap();
    let err = eval(&expr, &mut sh).unwrap_err();
    assert_eq!(render_error_body("44 / 0 ", &err),
        "44 / 0 : division by 0 (error token is \"0 \")");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p huck-engine render_division_by_zero_token 2>&1 | tail`
Expected: FAIL — error token is empty (`offset` is `None` for eval-time `DivisionByZero`).

- [ ] **Step 3: Carry the divisor offset on `Div`/`Mod`**

Change the AST nodes:

```rust
Div(Box<ArithExpr>, Box<ArithExpr>, usize), // usize = divisor token offset
Mod(Box<ArithExpr>, Box<ArithExpr>, usize),
```

Special-case `Slash`/`Percent` in the Pratt loop (before the generic table; mirror the `Power` special-case at 564). Capture `err_off` after parsing the rhs (it points at the rhs's last token — the divisor literal for the simple case):

```rust
if matches!(op, ArithToken::Slash | ArithToken::Percent) && 22 >= min_bp {
    self.bump();
    let rhs = self.parse_expr(23)?;
    let off = self.err_off;
    lhs = if op == ArithToken::Slash {
        ArithExpr::Div(Box::new(lhs), Box::new(rhs), off)
    } else {
        ArithExpr::Mod(Box::new(lhs), Box::new(rhs), off)
    };
    continue;
}
```

Remove `Slash`/`Percent` rows from the generic `BinOpEntry` table (588-590) so they no longer match there.

In `eval`, update the `Div`/`Mod` arms to read the offset and attach it:

```rust
ArithExpr::Div(a, b, off) => {
    let lhs = eval(a, shell)?;
    let rhs = eval(b, shell)?;
    if rhs == 0 {
        return Err(ArithError::at(ArithErrorKind::DivisionByZero, *off));
    }
    Ok(lhs.wrapping_div(rhs))
}
ArithExpr::Mod(a, b, off) => {
    let lhs = eval(a, shell)?;
    let rhs = eval(b, shell)?;
    if rhs == 0 {
        return Err(ArithError::at(ArithErrorKind::ModuloByZero, *off));
    }
    Ok(lhs.wrapping_rem(rhs))
}
```

Fix any other match on `ArithExpr::Div`/`Mod` (e.g. a `Debug`-driven test or pretty-printer) flagged by the compiler. Leave compound `/=`/`%=` by-zero as-is (their offset stays `None`; those lines are excluded from the harness).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p huck-engine arith 2>&1 | tail -25`
Expected: PASS — including `render_division_by_zero_token`.

- [ ] **Step 5: Commit**

```bash
git add crates/huck-engine/src/arith.rs
git commit -m "v216 task 4: carry divisor offset on Div/Mod for by-zero error token

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Route the arith emission sites through the prologue + body formatter

**Files:**
- Modify: `crates/huck-engine/src/expand.rs` (`eval_arith_word` 116-127; `$(( ))` emission 1113-1128 and 1565-1580)
- Modify: `crates/huck-engine/src/executor.rs` (`run_arith` 1965-1980)
- Modify: `crates/huck-engine/src/builtins.rs` (`builtin_let` 5417-5433)
- Modify: `crates/huck-engine/src/param_expansion.rs` (substring arith error 392-405)
- Test: existing arith unit tests in those files that assert `huck: arithmetic:` / `huck: ((:` text (update them); the byte-match guard is Task 6's harness.

**Interfaces:**
- Consumes: `Shell::error_prefix(cmd)` (Task 1), `arith::render_error_body(src, &err)` (Task 3).
- Produces: arith errors emitted as `"{prefix}{body}"` with `cmd` = `None` (`$(( ))`), `Some("((")` (`(( ))`), `Some("let")` (`let`).

Key detail: the **source string** passed to `render_error_body` must be the exact string passed to `parse` (untrimmed-leading), so offsets line up and the leading-trim/echo matches bash.

- [ ] **Step 1: Update `eval_arith_word` to expose the source on error**

Change it to parse the **leading-preserving** expanded string and hand the source back to callers. Replace 116-127 with:

```rust
/// Returns the expanded arith source string and the eval result, so callers
/// can render bash-compatible errors (which echo the source + error token).
pub(crate) fn eval_arith_word_src(
    body: &Word,
    shell: &mut Shell,
) -> (String, Result<i64, crate::arith::ArithError>) {
    let s = crate::param_expansion::expand_word_to_string(body, shell);
    if s.trim().is_empty() {
        return (s, Ok(0));
    }
    let res = crate::arith::parse(&s).and_then(|e| crate::arith::eval(&e, shell));
    (s, res)
}

/// Back-compat thin wrapper for callers that only need the value
/// (arith-`for`, `${a[i]}` coercion, etc.).
pub(crate) fn eval_arith_word(
    body: &Word,
    shell: &mut Shell,
) -> Result<i64, crate::arith::ArithError> {
    eval_arith_word_src(body, shell).1
}
```

Note: previously `parse` received the fully-trimmed `t`; now it receives `s` (the tokenizer skips whitespace, so values are unchanged), which is required for offsets to be relative to the echoed source.

- [ ] **Step 2: Update the two `$(( ))` emission sites in `expand.rs`**

At 1113-1128 (`WordPart::Arith`), replace with:

```rust
WordPart::Arith { body, quoted: _ } => {
    let (src, res) = eval_arith_word_src(body, shell);
    match res {
        Ok(n) => { current.push_str(&n.to_string(), true); has_emitted = true; }
        Err(e) => {
            let prefix = shell.error_prefix(None);
            with_err(|err| e!(err, "{prefix}{}", crate::arith::render_error_body(&src, &e)));
            has_emitted = true;
        }
    }
}
```

Apply the identical change at the second `$(( ))` site (1565-1580). Confirm both with `grep -n "huck: arithmetic:" crates/huck-engine/src/expand.rs`.

- [ ] **Step 3: Update `run_arith` (`(( ))`) in `executor.rs`**

Replace 1972-1979:

```rust
let (src, res) = crate::expand::eval_arith_word_src(body, shell);
match res {
    Ok(0) => ExecOutcome::Continue(1),
    Ok(_) => ExecOutcome::Continue(0),
    Err(e) => {
        let prefix = shell.error_prefix(Some("(("));
        { let mut err = err_writer(err_sink, sink);
          e!(&mut *err, "{prefix}{}", crate::arith::render_error_body(&src, &e)); }
        ExecOutcome::Continue(1)
    }
}
```

- [ ] **Step 4: Update `builtin_let`**

`builtin_let` parses each arg `a` directly. Replace 5423-5431:

```rust
for a in args {
    match crate::arith::parse(a).and_then(|e| crate::arith::eval(&e, shell)) {
        Ok(v) => last = v,
        Err(e) => {
            let prefix = shell.error_prefix(Some("let"));
            e!(err, "{prefix}{}", crate::arith::render_error_body(a, &e));
            return ExecOutcome::Continue(1);
        }
    }
}
```

Leave the empty-args case (5418-5420) as `huck: let: expression expected` for now (bash prints a `let:` usage line; out of scope — it is not an arith-expression error).

- [ ] **Step 5: Update the substring arith error in `param_expansion.rs`**

At 392-405 the offset/length arith errors emit `huck: arithmetic: {e}`. The substring **value** (offset/length) expression text is the expanded `s`. Replace those two `with_err(|err| e!(err, "huck: arithmetic: {}", e));` with:

```rust
let prefix = shell.error_prefix(None);
with_err(|err| e!(err, "{prefix}{}", crate::arith::render_error_body(&s, &e)));
```

(Confirm `s` is the expanded source string in scope at both the parse-fail and eval-fail arms; if the variable is named differently, use that name.)

- [ ] **Step 6: Update existing unit tests that assert the old text**

`grep -rn 'huck: arithmetic:\|huck: ((:\|is not an integer\|assignment requires variable\|division by zero' crates/huck-engine/src/{expand,executor,param_expansion,builtins}.rs` — for each test asserting the OLD message, update the expected string to the new bash-formatted output (compute it: `<prefix><expr>: <msg> (error token is "<tok>")`; in unit tests `is_interactive` is typically true so the prefix is `huck: ` / `huck: ((: ` with no line number). Example: a test asserting `huck: arithmetic: division by zero` for `$((1/0))` becomes `huck: 1/0: division by 0 (error token is "0")` if interactive (no `line N:`). Verify each against an actual run rather than guessing.

- [ ] **Step 7: Run the engine test suite**

Run: `cargo test -p huck-engine 2>&1 | tail -25`
Expected: PASS (all updated tests green).

- [ ] **Step 8: Commit**

```bash
git add crates/huck-engine/src/expand.rs crates/huck-engine/src/executor.rs crates/huck-engine/src/builtins.rs crates/huck-engine/src/param_expansion.rs
git commit -m "v216 task 5: route arith error sites through bash prologue + body formatter

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: `arith_error_diff_check.sh` gold-standard harness

**Files:**
- Create: `tests/scripts/arith_error_diff_check.sh`

**Interfaces:**
- Consumes: a built `huck` binary and a `bash` on PATH.
- Produces: a self-contained diff harness asserting byte-identical **stderr** between bash and huck for a curated set of both-error arith fragments, each invoked as a **script file** (so non-interactive mode + a known `$0` apply).

- [ ] **Step 1: Write the harness script**

Model it on an existing `tests/scripts/*_diff_check.sh` (read one first, e.g. `arith_error_status_diff_check.sh` if present, for the repo's harness conventions — binary path discovery, temp-dir cleanup, pass/fail reporting). The harness must:
1. Build/locate `huck` (`cargo build --release --bin huck`; `target/release/huck`).
2. For each fragment, write it to a temp script file `frag.sh`, run `bash ./frag.sh` and `huck ./frag.sh` from the temp dir (so `$0` = `./frag.sh` in both), capture **stderr only**, and `diff` them.
3. Use a **fixed script name** in both runs so the `$0`-derived prologue matches.

Curated fragments (each both shells error; behavioral cases excluded):

```sh
fragments=(
  'echo $(( 7 = 43 ))'
  'echo $(( 44 / 0 ))'
  'echo $(( 2#44 ))'
  'echo $(( 3425#56 ))'
  'echo $(( 2# ))'
  'echo $(( 4 ? : 3 + 5 ))'
  'echo $(( a b ))'
  '(( x = 9 y = 41 ))'
  '(( a b ))'
  'let "rv = 7 + (43 * 6"'
  'echo $(( 1 ? 20 : x += 2 ))'
  'echo $(( 0 && B = 42 ))'
)
```

For each, assert `diff <(bash run) <(huck run)` is empty. Print `PASS`/`FAIL fragment` per line and exit non-zero on any failure.

- [ ] **Step 2: Make it executable and run it**

Run: `chmod +x tests/scripts/arith_error_diff_check.sh && bash tests/scripts/arith_error_diff_check.sh 2>&1 | tail -30`
Expected: every fragment `PASS`. If a fragment diverges on line-number or token whitespace, investigate against the captured diff (do NOT relax the assertion silently — either fix the offset/wording in Tasks 3-5 or, if the fragment is actually behavioral, remove it from the list with a comment explaining why).

- [ ] **Step 3: Commit**

```bash
git add tests/scripts/arith_error_diff_check.sh
git commit -m "v216 task 6: arith_error_diff_check.sh byte-match harness vs bash

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 7: Docs, baseline re-triage, divergence + memory bookkeeping

**Files:**
- Modify: `docs/bash-test-suite-baseline.md` (the `arith` row note)
- Modify: `docs/bash-divergences.md` (add a `[deferred]` entry for the remaining arith behavioral divergences; note the prologue foundation)
- Modify: `docs/architecture.md` (error-reporting / "where to add" area)
- Modify: memory files `project_huck_iterations.md` + `MEMORY.md` (under the memory dir)

**Interfaces:** none (documentation only).

- [ ] **Step 1: Re-run the arith category and update the baseline note**

Run:
```bash
export BASH_SOURCE_DIR=/tmp/bash-5.2.21
HUCK_BASH_TEST_CATEGORY=arith bash tests/bash-test-suite/runner.sh > /tmp/arith-sweep.md 2>&1
```
Inspect the new `arith.diff` under the printed `/tmp/huck-bash-tests-*/` dir. Update the `arith` row Note in `docs/bash-test-suite-baseline.md` to record: the error-prologue + message-wording + error-token lines now match bash for the both-error cases; remaining failures are the deferred **behavioral** divergences (overflow wrapping, `++`/`--` on non-lvalues, lazy dead-branch eval, array-element lvalues, substring ternary colons). Keep the doc huck-authored (no GPL'd bash text).

- [ ] **Step 2: Add the deferred divergence entry**

In `docs/bash-divergences.md`, add a `[deferred]`, low/medium entry (e.g. `M-`/`L-` per the doc's ranking) capturing the remaining arith **behavioral** divergences listed above, and a one-line note that `Shell::error_prefix` is the bash-compatible prologue and shell-wide adoption is staged across iterations (arith done in v216; builtins/parser/etc. follow).

- [ ] **Step 3: Architecture note**

In `docs/architecture.md`, in the error-reporting / cross-cutting-conventions area (and the "where to add common features" cheatsheet), add a pointer: arith and (future) other errors render their prologue via `Shell::error_prefix(cmd)`; the bash format is `<name>: [line N: ][cmd: ]`; arith bodies use `arith::render_error_body`.

- [ ] **Step 4: Memory + iteration log**

Append a v216 entry to `project_huck_iterations.md` and a one-line index pointer in `MEMORY.md` (memory dir), summarizing: built the bash error-prologue foundation; converted arith error sites end-to-end; behavioral arith divergences deferred.

- [ ] **Step 5: Full workspace test + final commit**

Run: `cargo test 2>&1 | tail -15` and `bash tests/scripts/arith_error_diff_check.sh 2>&1 | tail -5`
Expected: all tests pass; harness all-PASS.

```bash
git add docs/ "$(git rev-parse --show-toplevel)"/../.claude/projects/-home-john-projects-huck/memory/ 2>/dev/null || true
git add docs/
git commit -m "v216 task 7: baseline re-triage + deferred divergence + docs/memory

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

(Memory files live outside the repo tree; commit only the in-repo `docs/` changes here and write the memory files separately with the Write tool.)

---

## Self-Review

**Spec coverage:**
- Prologue mechanism (spec §1) → Task 1. ✓
- Arith expression-echo + error-token tracking (spec §2) → Tasks 3 (parse-time) + 4 (div/mod eval-time). ✓
- Message-wording map (spec §3) → Task 2 (`bash_message`) + applied in Tasks 3-4. ✓
- Routing emission sites (spec §2 callers) → Task 5. ✓
- Testing / diff-check harness (spec §4) → Task 6; unit-test updates in Tasks 2/3/5. ✓
- Baseline re-triage + divergence + architecture + memory bookkeeping (spec §"Divergence-doc bookkeeping" + workflow) → Task 7. ✓
- Out-of-scope behavioral cases excluded from harness + documented → Tasks 5/6/7 notes. ✓

**Placeholder scan:** No "TBD"/"handle edge cases" — each code step shows the code; the only deliberate deferrals (overflow, inc/dec, lazy eval, array lvalues, substring colons, compound-assign by-zero token) are named explicitly as out of scope, not hidden.

**Type consistency:** `error_prefix(Option<&str>) -> String` (Task 1) is consumed verbatim in Task 5. `tokenize -> Result<(Vec<ArithToken>, Vec<usize>), ArithError>` (Task 3) consumed only by `parse`. `render_error_body(&str, &ArithError) -> String` (Task 3) consumed in Tasks 3-5 tests and Task 5 sites. `ArithError { kind, offset }` + `ArithError::at`/`parse` + `bash_message` (Task 2) used consistently in Tasks 3-5. `eval_arith_word_src` (Task 5) returns `(String, Result<i64, ArithError>)` and is consumed at the `$(( ))`/`(( ))` sites; `eval_arith_word` kept as the value-only wrapper for existing callers. `ArithExpr::Div`/`Mod` gain a `usize` (Task 4) — every match arm updated.

**Known risk to watch during execution:** exact line numbers in the prologue depend on `current_lineno` matching bash's `executing_line_number()`; the harness uses one-statement scripts to keep this deterministic. If a multi-line `(( ))` fragment misattributes the line, simplify the fragment rather than relaxing the diff.
