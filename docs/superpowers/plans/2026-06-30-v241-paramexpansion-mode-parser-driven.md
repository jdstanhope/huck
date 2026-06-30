# v241 — ParamExpansion Mode + Parser-Driven `${…}` Assembly Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build, dormant and comprehensive, a parser-driven `${…}` path — the lexer emits small atoms under parser-pushed modes, and a new `parser.rs` assembles `WordPart::ParamExpansion` from those atoms — proven equal to the current lexer's output by differential tests.

**Architecture:** New `TokenKind` atoms + a `ParamExpansion` head mode + operand sub-modes in `lexer.rs` (each emits ONE atom per pull, never scanning ahead for a matching `}`). A new `crates/huck-syntax/src/parser.rs` drives the v240 mode stack to assemble the existing `WordPart::ParamExpansion`/`ParamModifier` AST. Production (`Command` mode, `command.rs`) is untouched; the new path is reached only by tests via `parser::parse_param_expansion`.

**Tech Stack:** Rust, `crates/huck-syntax/src/{lexer.rs,parser.rs,lib.rs,command.rs}`.

## Global Constraints

- **Byte-identical / dormant:** `cargo test --workspace` green + full `tests/scripts/*_diff_check.sh` release sweep byte-identical; 0 warnings. No production path pushes a non-`Command` mode or calls `parser.rs`. `Command` mode and `command.rs` parsing are unchanged.
- **Lexer emits atoms only; never scans ahead for a matching delimiter.** A `}`/`/`/`:` becomes a close/separator atom the instant it is seen unquoted; the parser owns all matching/nesting. The old `scan_braced_*` scanners are NOT used by the new path (they stay only for the untouched production path).
- **Existing AST types only:** build `WordPart::ParamExpansion`, `ParamModifier`, `SubscriptKind`, `Word`, `TildeSpec`, reusing `CaseDirection`/`TransformOp`/`SubstAnchor`. No engine change.
- **Additive `TokenKind`** (`#[non_exhaustive]`); existing `Word(Word)` token unchanged.
- New methods/types `pub(crate)`; dormant items get `#[allow(dead_code)]` (as v240 did) to keep 0 warnings.
- **Deferred (return `ParseError::UnsupportedExpansion`):** `$(…)`, `$((…))`, backtick inside any operand/subscript, and the `${$'x'}` extquote-name form. The differential corpus excludes these.
- The authoritative semantic reference for every `${…}` form is the current lexer: `scan_braced_param_expansion` (lexer.rs:3255) and `dispatch_braced_modifier` (lexer.rs:3963–4174). The differential corpus is the completeness oracle — if the new path mismaps a form, a corpus case fails.
- Commit trailer on every commit: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

## File Structure

- `crates/huck-syntax/src/lexer.rs` — add atom `TokenKind`s, `ParamOpKind`/`SubstKind` enums, `Mode` operand variants, and `scan_step` arms for the new modes.
- `crates/huck-syntax/src/parser.rs` — NEW. The atom→AST assembler (`parse_param_expansion`, `parse_word`, the `ParamModifier` mapping). Hosts the differential tests.
- `crates/huck-syntax/src/lib.rs` — add `pub mod parser;`.
- `crates/huck-syntax/src/command.rs` — add `ParseError::UnsupportedExpansion` variant only.

---

### Task 1: Scaffolding — atoms, enums, modes, error, parser.rs skeleton

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` (`enum TokenKind` ~line 347; `enum Mode` ~line 500; add `ParamOpKind`/`SubstKind` near `CaseDirection` ~line 168)
- Modify: `crates/huck-syntax/src/command.rs` (`enum ParseError` ~line 731)
- Create: `crates/huck-syntax/src/parser.rs`
- Modify: `crates/huck-syntax/src/lib.rs` (add `pub mod parser;`)

**Interfaces produced (used by Tasks 2–5):**
- TokenKind atoms: `ParamOpen`, `ParamClose`, `LBracket`, `RBracket`, `ParamSep`, `ParamName(String)`, `ParamLengthPrefix`, `ParamIndirect`, `ParamOp(ParamOpKind)`, `Lit { text: String, quoted: bool }`, `DollarName(String)`, `DeferredExpansion`.
- `pub(crate) enum ParamOpKind { UseDefault(bool), AssignDefault(bool), ErrorIfUnset(bool), UseAlternate(bool), RemovePrefix(bool), RemoveSuffix(bool), Substitute(SubstKind), Case(CaseDirection, bool), Transform(TransformOp), Substring }` (derive `Debug, Clone, PartialEq, Eq`).
- `pub(crate) enum SubstKind { First, All, Prefix, Suffix }` (derive `Debug, Clone, Copy, PartialEq, Eq`).
- `Mode` variants already declared in v240: `ParamExpansion`; ADD: `ParamWordOperand`, `ParamSubstPatternOperand`, `ParamSubstringOffsetOperand`, `ParamSubscriptOperand`.
- `command::ParseError::UnsupportedExpansion`.
- `parser::parse_param_expansion`, `parser::parse_word` (skeletons returning `unimplemented!()` for now — see step 3).

- [ ] **Step 1: Write the failing test** — add to a new `#[cfg(test)] mod tests` at the bottom of `crates/huck-syntax/src/parser.rs`:

```rust
#[test]
fn scaffolding_types_exist() {
    use crate::lexer::{TokenKind, ParamOpKind, SubstKind, Mode};
    let _ = TokenKind::ParamOpen;
    let _ = TokenKind::Lit { text: "x".into(), quoted: false };
    let _ = ParamOpKind::Substitute(SubstKind::All);
    let _ = Mode::ParamWordOperand;
    let _ = crate::command::ParseError::UnsupportedExpansion;
}
```

- [ ] **Step 2: Run it to verify it fails** — Run: `cargo test -p huck-syntax --lib scaffolding_types_exist 2>&1 | tail -20`. Expected: compile errors (unknown variants / `parser` module not found).

- [ ] **Step 3: Implement the scaffolding.**

(a) In `lexer.rs`, near `CaseDirection` (~line 168), add:
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SubstKind { First, All, Prefix, Suffix }   // / , // , /# , /%

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ParamOpKind {
    UseDefault(bool), AssignDefault(bool), ErrorIfUnset(bool), UseAlternate(bool), // bool = colon-prefixed
    RemovePrefix(bool), RemoveSuffix(bool),   // bool = longest (## / %%)
    Substitute(SubstKind),
    Case(CaseDirection, bool),                // bool = all (^^ / ,,)
    Transform(TransformOp),
    Substring,
}
```

(b) In `lexer.rs` `enum TokenKind` (~line 347), add the atom variants (place after `RedirFd`):
```rust
    // --- Phase C parser-driven atoms (dormant in v241; emitted only under the
    // ParamExpansion/operand modes, consumed only by parser.rs). ---
    ParamOpen, ParamClose, LBracket, RBracket, ParamSep,
    ParamName(String),
    ParamLengthPrefix, ParamIndirect,
    ParamOp(ParamOpKind),
    Lit { text: String, quoted: bool },
    DollarName(String),
    DeferredExpansion,   // $( / $(( / backtick inside an operand — v241 stops here
```

(c) In `lexer.rs` `enum Mode` (~line 500), add the four operand variants after `ParamExpansion`:
```rust
    ParamWordOperand,
    ParamSubstPatternOperand,
    ParamSubstringOffsetOperand,
    ParamSubscriptOperand,
```

(d) In `command.rs` `enum ParseError` (~line 731), add:
```rust
    /// A nested expansion the parser-driven path does not handle yet
    /// (`$(…)` / `$((…))` / backtick inside a `${…}` operand). v241 boundary.
    UnsupportedExpansion,
```
If `ParseError` has a `Display`/message mapping (errors.rs), add an arm returning `"unsupported expansion".to_string()`.

(e) Create `crates/huck-syntax/src/parser.rs` with the skeleton:
```rust
//! Parser-driven front-end (Phase C). Consumes the stack-mode lexer's atoms and
//! builds the existing AST (`WordPart`/`Word`). DORMANT in v241: reached only by
//! tests; production still uses the lexer's pre-built Words + command.rs.
#![allow(dead_code)]

use crate::command::ParseError;
use crate::lexer::{Lexer, Mode, TokenKind, Word, WordPart};

/// Assemble a single `WordPart::ParamExpansion` starting at a `${`. Pushes
/// `Mode::ParamExpansion` itself, so the caller passes a lexer positioned at `${`
/// (under any mode — the push ensures `${` is lexed as atoms, not a pre-built Word).
pub(crate) fn parse_param_expansion(iter: &mut Lexer, quoted: bool) -> Result<WordPart, ParseError> {
    let _ = (iter, quoted);
    unimplemented!("parse_param_expansion: Task 4/5")
}

/// Assemble a `Word` (Vec<WordPart>) from atoms in the CURRENT mode, stopping at a
/// boundary atom for that mode (`}` / `ParamSep` / `]`). Used for operands.
pub(crate) fn parse_word(iter: &mut Lexer) -> Result<Word, ParseError> {
    let _ = iter;
    unimplemented!("parse_word: Task 4/5")
}

#[cfg(test)]
mod tests {
    use super::*;
    // tests added in later steps/tasks
}
```

(f) In `lib.rs`, after `pub mod lexer;`, add `pub mod parser;`.

- [ ] **Step 4: Run it to verify it passes** — Run: `cargo test -p huck-syntax --lib scaffolding_types_exist 2>&1 | tail -5` (PASS) and `cargo build -p huck-syntax 2>&1 | grep -E "error|warning" || echo clean` (clean). The full lexer suite must still pass (additive enums don't change behavior): `cargo test -p huck-syntax 2>&1 | grep "test result" | tail -2`.

- [ ] **Step 5: Commit**
```bash
git add crates/huck-syntax/src/lexer.rs crates/huck-syntax/src/command.rs crates/huck-syntax/src/parser.rs crates/huck-syntax/src/lib.rs
git commit -m "v241 T1: ParamExpansion atoms/modes scaffolding + parser.rs skeleton

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: `ParamExpansion` head mode — atom emission

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` (`scan_step` dispatch ~line 676; add `scan_step_param_head`)
- Test: `crates/huck-syntax/src/lexer.rs` (`#[cfg(test)]` module near the `next_token_*`/`mode_*` tests)

**Interfaces consumed:** Task 1 atoms + `ParamOpKind`. The v240 `push_mode`/`current_mode`/`next_token`/`peek`. `CharCursor` (`peek`/`next`/`offset`).
**Interfaces produced:** `scan_step` emits, in `Mode::ParamExpansion`, the head atom sequence for a `${…}`.

The head mode is a small per-char emitter. It tracks a tiny internal position via the **atoms already emitted is NOT available**, so use a per-`Lexer` field `param_head_seen_name: bool` (add it, init false in `new`/`from_tokens`, reset when `ParamExpansion` is pushed/popped — simplest: set false in `push_mode` when pushing `ParamExpansion`). Rules (consult lexer.rs:3255–3541 for exact char handling):

- [ ] **Step 1: Write failing tests** — helper drives the mode like the parser will:

```rust
#[cfg(test)]
fn head_atoms(s: &str) -> Vec<TokenKind> {
    let mut lx = Lexer::new(s, LexerOptions::default(), true);
    lx.push_mode(Mode::ParamExpansion);
    let mut out = Vec::new();
    // pull until ParamClose inclusive (head mode only; operands are separate modes)
    while let Some(t) = lx.next_token().unwrap() {
        let stop = matches!(t.kind, TokenKind::ParamClose);
        out.push(t.kind);
        if stop { break; }
    }
    out
}

#[test]
fn head_bare_name() {
    assert_eq!(head_atoms("${name}"),
        vec![TokenKind::ParamOpen, TokenKind::ParamName("name".into()), TokenKind::ParamClose]);
}
#[test]
fn head_value_operator() {
    // stops emitting at the operator; operand is a different mode (Task 3)
    let a = head_atoms_until_op("${x:-foo}");
    assert_eq!(a, vec![TokenKind::ParamOpen, TokenKind::ParamName("x".into()),
                       TokenKind::ParamOp(ParamOpKind::UseDefault(true))]);
}
#[test]
fn head_length_and_indirect() {
    assert_eq!(head_atoms("${#x}"),
        vec![TokenKind::ParamOpen, TokenKind::ParamLengthPrefix,
             TokenKind::ParamName("x".into()), TokenKind::ParamClose]);
    assert_eq!(head_atoms("${!x}"),
        vec![TokenKind::ParamOpen, TokenKind::ParamIndirect,
             TokenKind::ParamName("x".into()), TokenKind::ParamClose]);
}
#[test]
fn head_special_param_names() {
    // bare special-param names: ${@} ${#} ${?} ${!}
    for (s, n) in [("${@}","@"),("${#}","#"),("${?}","?"),("${!}","!")] {
        assert_eq!(head_atoms(s),
            vec![TokenKind::ParamOpen, TokenKind::ParamName(n.into()), TokenKind::ParamClose],
            "for {s}");
    }
}
#[test]
fn head_subscript() {
    // ${a[ ... emits ParamOpen, ParamName(a), LBracket then yields to subscript mode
    let mut lx = Lexer::new("${a[1]}", LexerOptions::default(), true);
    lx.push_mode(Mode::ParamExpansion);
    assert!(matches!(lx.next_token().unwrap().unwrap().kind, TokenKind::ParamOpen));
    assert!(matches!(lx.next_token().unwrap().unwrap().kind, TokenKind::ParamName(ref n) if n=="a"));
    assert!(matches!(lx.next_token().unwrap().unwrap().kind, TokenKind::LBracket));
}
```

(Add the small `head_atoms_until_op` helper: like `head_atoms` but stops after the first `ParamOp`.)

- [ ] **Step 2: Run to verify they fail** — Run: `cargo test -p huck-syntax --lib head_ 2>&1 | tail -20`. Expected: `unreachable!` panic ("Mode::ParamExpansion not implemented") or assertion failures.

- [ ] **Step 3: Implement `scan_step_param_head`.** In `scan_step` (~line 676) replace the `ParamExpansion` arm's `unreachable!` with `Mode::ParamExpansion => self.scan_step_param_head(),`. Implement:

```rust
fn scan_step_param_head(&mut self) -> Result<Step, LexError> {
    // 1. `${` opener (only at the very start of this mode — cursor sits on `$`).
    if self.cursor.peek() == Some(&'$') {
        // lookahead one char for `{`
        let off = self.cursor.offset();
        let mut probe = self.cursor.clone();   // CharCursor is Clone (over &str)
        probe.next(); // consume $
        if probe.peek() == Some(&'{') {
            let (l, c) = (self.cursor.line(), self.cursor.column());
            self.cursor.next(); self.cursor.next(); // $ {
            self.param_head_seen_name = false;
            self.history.push(Token::new(TokenKind::ParamOpen, Span::new(off, l, c)));
            return Ok(Step::Produced);
        }
    }
    // ... name/prefix/operator/bracket/close logic (mirrors scan_braced_param_expansion
    //     char handling — see lexer.rs:3255-3541; emit atoms instead of building WordParts)
    // After ParamOpen: if !param_head_seen_name, handle ${#…}/${!…} prefixes + the name
    //   + special-param names + optional `[` (emit LBracket, then the parser pushes the
    //   subscript mode). Set param_head_seen_name=true after emitting ParamName.
    // After the name: peek the operator chars and emit ParamOp(kind) (use the recognition
    //   table below), or `}` -> ParamClose, or `[`/`]` -> LBracket/RBracket, else the
    //   form is bad-subst: emit a sentinel the parser maps to BadSubst (emit ParamClose
    //   after consuming through the operand; OR emit ParamOp + let parser detect — the
    //   plan's recommended approach: on an unrecognized operator char, do NOT emit ParamOp;
    //   instead emit ParamName("") already done? Keep it simple: recognize the FULL set
    //   below; anything else => the parser sees a non-operator, non-`}` atom and returns
    //   BadSubst by reconstructing raw from span.)
    unimplemented!("fill per the operator-recognition table")
}
```

Operator-recognition table (emit `ParamOp(kind)` after consuming the chars; `:` then one of `-=?+` is the colon form, `:` then anything else is `Substring` (consume only the `:`)):

| chars | `ParamOpKind` |
|---|---|
| `:-` / `-` | `UseDefault(true/false)` |
| `:=` / `=` | `AssignDefault(true/false)` |
| `:?` / `?` | `ErrorIfUnset(true/false)` |
| `:+` / `+` | `UseAlternate(true/false)` |
| `#` / `##` | `RemovePrefix(false/true)` |
| `%` / `%%` | `RemoveSuffix(false/true)` |
| `/` / `//` / `/#` / `/%` | `Substitute(First/All/Prefix/Suffix)` |
| `^` / `^^` | `Case(Upper, false/true)` |
| `,` / `,,` | `Case(Lower, false/true)` |
| `@` then letter L | `Transform(<map L>)` (`Q E P A K a k U L u` → `TransformOp`, lexer.rs:179) |
| `:` (not `-=?+`) | `Substring` (consume only the `:`) |

Add the `param_head_seen_name: bool` field (init `false` in `new` line ~590 and `from_tokens` line ~1383) and reset it to `false` inside `push_mode` when `m == Mode::ParamExpansion`.

NOTE on `${#}` vs `${#x}` and `${!}` vs `${!x}`: after `ParamOpen`, peek2 — `#`/`!` followed by `}` is the special-param NAME (emit `ParamName("#"/"!")`); `#`/`!` followed by a name char is `ParamLengthPrefix`/`ParamIndirect`. Mirror lexer.rs:3318–3498.

- [ ] **Step 4: Run to verify they pass** — Run: `cargo test -p huck-syntax --lib head_ 2>&1 | grep "test result"`. All head_* PASS. Full lexer suite still green: `cargo test -p huck-syntax 2>&1 | grep "test result" | tail -2` (production unaffected — these modes are never pushed in production).

- [ ] **Step 5: Commit**
```bash
git add crates/huck-syntax/src/lexer.rs
git commit -m "v241 T2: ParamExpansion head mode atom emission

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Operand modes — atom emission

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` (`scan_step` arms for the 4 operand modes; add `scan_step_param_operand`)
- Test: `crates/huck-syntax/src/lexer.rs` (test module)

**Interfaces consumed:** Task 1 atoms; v240 modes. **Produced:** `scan_step` emits operand atoms (`Lit`/`DollarName`/`ParamOpen`/`ParamSep`/`ParamClose`/`DeferredExpansion`) in the four operand modes.

All four operand modes share one routine parameterized by which separator (if any) terminates the segment. Define:
```rust
fn scan_step_param_operand(&mut self, sep: Option<char>, end: char) -> Result<Step, LexError>
// end = '}' for ParamWordOperand / pattern / offset-length; ']' for subscript.
// sep = Some('/') for ParamSubstPatternOperand; Some(':') for ParamSubstringOffsetOperand; None otherwise.
```
Dispatch in `scan_step`:
```rust
Mode::ParamWordOperand           => self.scan_step_param_operand(None,       '}'),
Mode::ParamSubstPatternOperand   => self.scan_step_param_operand(Some('/'),  '}'),
Mode::ParamSubstringOffsetOperand=> self.scan_step_param_operand(Some(':'),  '}'),
Mode::ParamSubscriptOperand      => self.scan_step_param_operand(None,       ']'),
```

Per-pull behavior (mirror `parse_braced_operand_opts` lexer.rs:3080–3172 for the literal/quote/`$` handling), emitting ONE atom:
- unquoted `end` (`}` or `]`) → emit `ParamClose` (for `}`) or `RBracket` (for `]`); do NOT consume past it... actually consume it and emit the matching atom.
- unquoted `sep` char → emit `ParamSep`.
- `$` then `{` → emit `ParamOpen` (parser recurses).
- `$` then name-char/special → consume the `$name`/special, emit `DollarName(name)`.
- `$` then `(` , or `` ` `` → emit `DeferredExpansion` (consume just the opener; parser returns `UnsupportedExpansion`).
- `'` → consume the single-quoted span, emit `Lit { text, quoted: true }` (single-quote content fully literal).
- `"` → DOUBLE-quote span: within it, `$`/`` ` `` are expansion triggers, so a `"…"` may yield several atoms (Lit + DollarName + …) with `quoted: true`; emit them one per pull (track an in-dquote sub-state on the Lexer, e.g. `param_in_dquote: bool`).
- `\` → escape: emit `Lit { text: <escaped char>, quoted: <current> }`.
- otherwise → accumulate a maximal literal run until the next special char (`end`, `sep`, `$`, `"`, `'`, `\`, backtick) and emit `Lit { text, quoted: <current dquote state> }`.

- [ ] **Step 1: Write failing tests:**
```rust
#[cfg(test)]
fn operand_atoms(s: &str, mode: Mode) -> Vec<TokenKind> {
    let mut lx = Lexer::new(s, LexerOptions::default(), true);
    lx.push_mode(mode);
    let mut out = Vec::new();
    while let Some(t) = lx.next_token().unwrap() {
        let stop = matches!(t.kind, TokenKind::ParamClose | TokenKind::RBracket);
        out.push(t.kind);
        if stop { break; }
    }
    out
}
#[test]
fn operand_plain_literal() {
    assert_eq!(operand_atoms("foo}", Mode::ParamWordOperand),
        vec![TokenKind::Lit { text: "foo".into(), quoted: false }, TokenKind::ParamClose]);
}
#[test]
fn operand_var_and_nested() {
    assert_eq!(operand_atoms("$a}", Mode::ParamWordOperand),
        vec![TokenKind::DollarName("a".into()), TokenKind::ParamClose]);
    assert_eq!(operand_atoms("${b}}", Mode::ParamWordOperand),
        vec![TokenKind::ParamOpen, /* nested head atoms not pulled here */ ]); // first atom is ParamOpen
}
#[test]
fn operand_subst_separator() {
    assert_eq!(operand_atoms("pat/", Mode::ParamSubstPatternOperand),
        vec![TokenKind::Lit { text: "pat".into(), quoted: false }, TokenKind::ParamSep]);
}
#[test]
fn operand_substring_separator() {
    assert_eq!(operand_atoms("1:", Mode::ParamSubstringOffsetOperand),
        vec![TokenKind::Lit { text: "1".into(), quoted: false }, TokenKind::ParamSep]);
}
#[test]
fn operand_deferred_cmdsub() {
    let a = operand_atoms("$(x)}", Mode::ParamWordOperand);
    assert_eq!(a[0], TokenKind::DeferredExpansion);
}
#[test]
fn operand_subscript_close() {
    assert_eq!(operand_atoms("3]", Mode::ParamSubscriptOperand),
        vec![TokenKind::Lit { text: "3".into(), quoted: false }, TokenKind::RBracket]);
}
```
(For `operand_var_and_nested`'s nested case, assert only the first atom is `ParamOpen`.)

- [ ] **Step 2: Run to verify they fail** — Run: `cargo test -p huck-syntax --lib operand_ 2>&1 | tail -20`. Expected: `unreachable!` panic / assertion failures.

- [ ] **Step 3: Implement `scan_step_param_operand`** per the rules above. Add the `param_in_dquote: bool` field (init false; reset when an operand mode is pushed). Use `scan_dollar_expansion`-style name reading for `$name` (lexer.rs:2200) but emit a `DollarName` atom rather than a WordPart; for specials (`$?`, `$1`, `$@`, `$#`, `$$`, `$!`, `$-`, `$*`) emit `DollarName("?"/"1"/…)`. (The parser maps `DollarName` → `WordPart::Var`/`LastStatus`/`AllArgs` in Task 4.)

- [ ] **Step 4: Run to verify they pass** — `cargo test -p huck-syntax --lib operand_ 2>&1 | grep "test result"` PASS; full lexer suite green.

- [ ] **Step 5: Commit**
```bash
git add crates/huck-syntax/src/lexer.rs
git commit -m "v241 T3: ParamExpansion operand modes atom emission

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: `parser.rs` assembler — core forms + differential infrastructure

**Files:**
- Modify: `crates/huck-syntax/src/parser.rs` (implement `parse_word`, `parse_param_expansion`; add the differential helper + tests)

**Interfaces consumed:** Task 1–3 atoms/modes; v240 `push_mode`/`pop_mode`/`peek_kind`/`next_kind`; existing AST (`WordPart`, `ParamModifier`, `Word`, `SubscriptKind`).
**Produced:** working `parse_param_expansion`/`parse_word` for: bare `${name}`, subscript `${a[i]}`/`${a[@]}`/`${a[*]}`, length `${#name}`, indirect `${!name}`, the value family (`:- := :? :+ - = ? +`), and the differential helpers.

`parse_word(iter)` loop: read atoms in the current (operand) mode, building `Vec<WordPart>`, until a boundary atom (`ParamClose`/`ParamSep`/`RBracket`) — which it must NOT consume (the caller consumes it). Atom→part:
- `Lit{text,quoted}` → `WordPart::Literal { text, quoted }`
- `DollarName(n)` → map: `"@"`→`AllArgs{quoted, joined:false}`, `"*"`→`AllArgs{quoted, joined:true}`, `"?"`→`LastStatus{quoted}`, else `Var{name:n, quoted}`. (The `quoted` is threaded from the enclosing call.)
- `ParamOpen` (detected via `peek_kind`, NOT consumed here) → `let p = parse_param_expansion(iter, quoted)?;` — `parse_param_expansion` owns the `ParamExpansion` mode push/pop AND consumes the `ParamOpen`. Push the returned `p`.
- `DeferredExpansion` → `return Err(ParseError::UnsupportedExpansion)`
- boundary atom (`ParamClose`/`ParamSep`/`RBracket`) → break (leave it for the caller; detect via `peek_kind`, do not consume)

`parse_param_expansion(iter, quoted)` — **owns its mode**: it `push_mode(Mode::ParamExpansion)` exactly once at entry and `pop_mode()` once before returning (so callers — the differential helper and `parse_word` — never wrap it in push/pop):
1. `iter.push_mode(Mode::ParamExpansion);` then expect `ParamOpen` (`next_kind`), else error. (The `ParamOpen` may already be buffered from a `parse_word` peek; consuming it does not scan, and the next scan happens under the just-pushed `ParamExpansion` mode.)
2. Read optional `ParamLengthPrefix` → `length_form = true`; optional `ParamIndirect` → `indirect = true`.
3. Expect `ParamName(name)`.
4. Optional `LBracket`: push `ParamSubscriptOperand`, `let sub_word = parse_word(iter)?`, expect `RBracket`, pop; `subscript = Some(subscript_kind_from(sub_word))` where `[@]`→`All`, `[*]`→`Star`, else `Index(sub_word)`.
5. Next atom:
   - `ParamClose` → modifier = (`length_form` ? `Length` : `None`); finish.
   - `ParamOp(kind)` → handle per Task 5's mapping (in Task 4, implement ONLY the value family: `UseDefault`/`AssignDefault`/`ErrorIfUnset`/`UseAlternate` → push `ParamWordOperand`, `parse_word`, pop, expect `ParamClose`, build the variant with `colon` from the bool). Other kinds → `unimplemented!()` for now (Task 5 fills them).
6. `iter.pop_mode()`; return `WordPart::ParamExpansion { name, modifier, quoted, subscript, indirect }`.

Differential helper + tests in `parser.rs`:
```rust
#[cfg(test)]
fn old_part(s: &str, quoted: bool) -> WordPart {
    use crate::lexer::{tokenize_with_opts, LexerOptions, TokenKind};
    let src = if quoted { format!("\"{s}\"") } else { s.to_string() };
    let toks = tokenize_with_opts(&src, LexerOptions::default()).expect("old lex");
    // dig out the single ParamExpansion WordPart (unwrap a Quoted wrapper if present)
    fn find(parts: &[WordPart]) -> Option<WordPart> {
        for p in parts {
            match p {
                WordPart::ParamExpansion { .. } => return Some(p.clone()),
                WordPart::Quoted { parts, .. } => if let Some(x) = find(parts) { return Some(x); },
                _ => {}
            }
        }
        None
    }
    match &toks[0].kind { TokenKind::Word(w) => find(&w.0).expect("param part"), _ => panic!() }
}
#[cfg(test)]
fn new_part(s: &str, quoted: bool) -> WordPart {
    use crate::lexer::{Lexer, LexerOptions};
    let mut lx = Lexer::new(s, LexerOptions::default(), true);
    parse_param_expansion(&mut lx, quoted).expect("new parse")
}
#[cfg(test)]
fn diff_ok(s: &str) { // unquoted and quoted
    assert_eq!(new_part(s, false), old_part(s, false), "unquoted {s:?}");
    assert_eq!(new_part(s, true),  old_part(s, true),  "quoted {s:?}");
}

#[test]
fn diff_core_forms() {
    for s in ["${x}", "${x:-d}", "${x-d}", "${x:=d}", "${x:?m}", "${x:+a}",
              "${x:-a b}", "${x:-${y}}", "${#x}", "${!x}",
              "${a[1]}", "${a[@]}", "${a[*]}", "${a[$i]}"] {
        diff_ok(s);
    }
}
```

- [ ] **Step 1: Write the failing test** — add `diff_core_forms` (above) + the helpers to `parser.rs` tests.
- [ ] **Step 2: Run to verify it fails** — `cargo test -p huck-syntax --lib diff_core_forms 2>&1 | tail -20` (panics at `unimplemented!`).
- [ ] **Step 3: Implement** `parse_word` + `parse_param_expansion` for the core+value forms as specified.
- [ ] **Step 4: Run to verify it passes** — `cargo test -p huck-syntax --lib diff_core_forms 2>&1 | grep "test result"` PASS. If a case mismatches, fix the mapping to match `old_part` (the production lexer is the oracle). Full lexer suite green.
- [ ] **Step 5: Commit**
```bash
git add crates/huck-syntax/src/parser.rs
git commit -m "v241 T4: parser.rs core + value-family ${} assembly + differential tests

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: `parser.rs` — all remaining forms + comprehensive differential corpus

**Files:**
- Modify: `crates/huck-syntax/src/parser.rs` (extend `parse_param_expansion` to all `ParamOpKind`s + the `!`/`#` head forms; add the full corpus)

**Interfaces consumed:** everything from Tasks 1–4.
**Produced:** the complete parser-driven `${…}` path, differentially equal to the production lexer across the comprehensive corpus.

Extend the `ParamOp(kind)` match in `parse_param_expansion` to ALL kinds (map to `ParamModifier` using the existing variants; the production `dispatch_braced_modifier` lexer.rs:3963–4174 is the row-by-row reference):
- `RemovePrefix(longest)`/`RemoveSuffix(longest)` → push `ParamWordOperand`, parse pattern, → `RemovePrefix/RemoveSuffix { pattern, longest }`.
- `Substitute(k)` → push `ParamSubstPatternOperand`, `parse_word` → pattern, expect `ParamSep`, pop+push `ParamWordOperand`, `parse_word` → replacement (if next is `ParamClose` with no `ParamSep`, replacement = empty `Word`), pop → `Substitute { pattern, replacement, anchor: k→{First/All→None, Prefix, Suffix}, all: k==All }`. (Mirror the `//` anchor-suppression note, lexer.rs:4086.)
- `Case(dir, all)` → push `ParamWordOperand`, parse optional pattern (empty body → `None`) → `Case { direction: dir, all, pattern }`.
- `Transform(op)` → expect `ParamClose` → `Transform { op }`.
- `Substring` → push `ParamSubstringOffsetOperand`, `parse_word` → offset, then if `ParamSep`: pop+push `ParamWordOperand`, `parse_word` → length=`Some`, else length=`None`; → `Substring { offset, length }`.

Head `!`/`#` forms (extend steps 2–5 of `parse_param_expansion`):
- `ParamIndirect` + name + `[@]`/`[*]` + `ParamClose` (no modifier) → `IndirectKeys`, `indirect: false`, keep `subscript`.
- `ParamIndirect` + `ParamName` ending `*`/`@`... (the `${!prefix*}`/`${!prefix@}` form): per lexer.rs:3394–3460, when the indirect name is followed by `*`/`@` then `}` → `PrefixNames { at: <@> }`, `indirect: false`. (The head mode must emit this so the parser can tell — recommended: head mode emits `ParamName(prefix)` then a `ParamOp` is NOT right; instead emit the trailing `*`/`@` as part of detecting prefix-names. PIN in implementation: head mode, in indirect context, if the name run is followed by `*`/`@` then `}`, emit a distinct atom or fold into `ParamName` + a marker. Simplest: head mode emits `ParamName(prefix)` and then `ParamName("*"/"@")`-as-marker before `ParamClose`; parser recognizes the two-name indirect-prefix shape.) Match `old_part` for `${!pre*}`/`${!pre@}` exactly.
- `ParamLengthPrefix` + name + `ParamClose` → `Length` (already in Task 4 core).
- Unrecognized operator / empty name / `${}` → `BadSubst { raw }` where `raw` = the exact `${…}` source slice. The head mode must surface enough for the parser to reconstruct `raw` (use `peek_span`/the lexer's `slice_from`, or have the head mode emit the raw on the bad path). Match `old_part("${x@}")`, `old_part("${}")`.

Comprehensive corpus (extend `diff_*` tests — each runs unquoted + quoted via `diff_ok`):
```rust
#[test]
fn diff_removal_and_case() {
    for s in ["${x#p}","${x##p}","${x%p}","${x%%p}",
              "${x^p}","${x^^}","${x,p}","${x,,}","${x#$a}","${x##${p}}"] { diff_ok(s); }
}
#[test]
fn diff_substitute() {
    for s in ["${x/p/r}","${x//p/r}","${x/#p/r}","${x/%p/r}",
              "${x/p}","${x//p}","${x/$a/$b}","${x/p/}"] { diff_ok(s); }
}
#[test]
fn diff_substring() {
    for s in ["${x:1}","${x:1:2}","${x:$o}","${x:$o:$l}","${x: -1}"] { diff_ok(s); }
}
#[test]
fn diff_transform() {
    for s in ["${x@Q}","${x@P}","${x@U}","${x@L}","${x@u}","${x@E}","${x@A}","${x@K}","${x@k}","${x@a}"] { diff_ok(s); }
}
#[test]
fn diff_indirect_and_special() {
    for s in ["${!x}","${!x[@]}","${!x[*]}","${!pre*}","${!pre@}",
              "${@}","${*}","${#}","${?}","${$}","${!}","${-}"] { diff_ok(s); }
}
#[test]
fn diff_badsubst() {
    for s in ["${x@}","${}","${x:}"] {
        assert_eq!(new_part(s, false), old_part(s, false), "badsubst {s:?}");
    }
}
#[test]
fn diff_dquote_operands() {
    // T3 fix: double-quoted operands tokenize FLAT (per-frame in_dquote). A simple
    // "…" is one quoted Lit (} stays literal); a "…" with a nested ${} recurses.
    // These MUST match the production lexer's flat WordPart::Literal{quoted:true}
    // (no Quoted wrapper — verified at parse_braced_operand_opts lexer.rs:3735).
    for s in ["${x:-\"a}b\"}", "${x:-\"a${y}b\"}", "${x:-\"$v\"}",
              "${x:-pre\"mid\"post}", "${x#\"$p\"}", "${x/\"a/b\"/c}"] { diff_ok(s); }
}
#[test]
fn diff_deferred_returns_unsupported() {
    use crate::lexer::{Lexer, LexerOptions};
    // $(…)/arith/backtick remain deferred even INSIDE a double-quoted operand.
    for s in ["${x:-$(cmd)}","${x:-$((1+1))}","${x:-`cmd`}","${x:-\"$(cmd)\"}"] {
        let mut lx = Lexer::new(s, LexerOptions::default(), true);
        assert!(matches!(parse_param_expansion(&mut lx, false),
                         Err(crate::command::ParseError::UnsupportedExpansion)), "for {s}");
    }
}
```

NOTE for `parse_word` (Task 4): it threads the enclosing `quoted` and must set each
operand part's `quoted` to `atom.quoted || enclosing_quoted` (so a bare operand inside an
outer-dquoted `${…}` is quoted, while a literal `"…"` span is always quoted). The
`diff_ok` quoted variant (which passes `quoted=true`) enforces this; the production
`parse_braced_operand_opts` (`q = enclosing_dquote`) is the oracle.

- [ ] **Step 1: Write the failing tests** — add all `diff_*` tests above.
- [ ] **Step 2: Run to verify they fail** — `cargo test -p huck-syntax --lib diff_ 2>&1 | tail -30` (unimplemented arms panic / mismatches).
- [ ] **Step 3: Implement** the full `ParamOp` mapping + the `!`/`#`/bad-subst head forms. For any mismatch, the production lexer (`old_part`) is the oracle — adjust the new path to match it exactly.
- [ ] **Step 4: Run the full proof:**
  - `cargo test -p huck-syntax --lib diff_ 2>&1 | grep "test result"` — all PASS.
  - `cargo test --workspace 2>&1 | grep -E "test result|FAILED|warning:" | tail -3` — green, 0 warnings.
  - Release harness sweep (production unaffected, but confirm):
    ```bash
    cargo build --release 2>&1 | tail -1
    H="$(pwd)/target/release/huck"; n=0; f=0
    for s in tests/scripts/*_diff_check.sh; do n=$((n+1)); HUCK_BIN="$H" timeout 60 bash "$s" >/dev/null 2>&1 || { echo "FAIL $(basename $s)"; f=$((f+1)); }; done
    echo "harness $n scripts $f fail"
    ```
    Expected: 0 fail (or only the known `kill_signals` 30s-budget flake — re-run it at 90s to confirm).
- [ ] **Step 5: Commit**
```bash
git add crates/huck-syntax/src/parser.rs
git commit -m "v241 T5: parser.rs full ${} grammar + comprehensive differential corpus

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Notes for the implementer

- Line numbers are approximate (~9,700-line `lexer.rs`); locate by symbol (`enum TokenKind`, `enum Mode`, `fn scan_step`, `scan_braced_param_expansion`, `dispatch_braced_modifier`).
- The **production lexer is the oracle**: when a differential case mismatches, change the NEW path to match `old_part`, never weaken the old path or the comparison.
- The lexer modes must **never scan ahead for a matching `}`/`]`**; they emit `ParamClose`/`RBracket`/`ParamSep` on the first unquoted occurrence and the parser handles matching via `push_mode`/`pop_mode`.
- Do NOT change `Command` mode, `command.rs` parsing, or any engine crate. If a form seems to require it, it is out of v241 scope — stop and flag.
- If distinguishing `:-` from substring `:` (or `${!pre*}` from `${!name}`) cleanly needs backtracking, use the v240 `mark`/`rewind` (it exists and is tested) rather than adding look-ahead to a scanner.
