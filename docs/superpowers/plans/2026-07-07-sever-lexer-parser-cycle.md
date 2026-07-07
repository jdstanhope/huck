# Sever the lexer→parser cycle (command-word subscript lvalue) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove the last lexer→parser production edge (`lexer.rs:3769` →
`parser::parse_fragment_word`) and the speculative `[…]` forward-scan that feeds
it, so the huck front-end dependency runs strictly one-way (parser → lexer). The
command-word indexed lvalue `a[i]=x` becomes parser-recognized like the
array-element lvalue already is. Two pre-existing divergences (D1, D2) resolve
toward bash as a designed side effect.

**Architecture:** The lexer stops intercepting `name[…]` at command-word-start. It
emits `Lit name` + a zero-width `LBracket` and sets a `pending_lvalue_subscript`
flag. The parser assembles the subscript `Word` under `Mode::ParamSubscriptOperand`
(reusing the `${a[i]}` / array-element machinery), then decides on a trailing
`AssignEq` token: assignment → `AssignTarget::Indexed` + `begin_assignment_value`;
no `AssignEq` → glob fold-back (the subscript parts flow into the word, expanded).

**Tech Stack:** Rust; `crates/huck-syntax` (lexer.rs, parser.rs, errors.rs);
`crates/huck-engine` comments only; a bash-diff harness.

**Spec:** `docs/superpowers/specs/2026-07-07-sever-lexer-parser-cycle-design.md`
(read it — especially §1-§5 and "Behavior changes (intended fixes)").

## Global Constraints

- **NEVER run `cargo test --workspace` or multi-threaded** — this box (1 core /
  1.9 GB) OOM-kills. Every test run: `( ulimit -v 2500000; cargo test -p
  huck-syntax --jobs 1 --lib -- --test-threads 1 <filter> )`. Same for
  `huck-engine`. Build the binary with `cargo build -p huck` (NOT huck-cli).
  Guard bash-diff harnesses with `( ulimit -v 1500000; timeout 60 … )`.
- **Commit trailer (verbatim, every commit):** `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- **Behavior:** byte-identical to bash everywhere EXCEPT the two intended fixes
  D1/D2 (spec "Behavior changes"). D1: `x=2; echo a[$x]` (no `=`) → `a[2]` (was
  `a[$x]`). D2: `a[0]=~/y` → tilde expands (was literal). Everything else — incl.
  `a[i]=v`, `a[$(..)]=v`, `a[i]=(x y)`, `a[bc]` literal glob — unchanged. Full
  huck-syntax + huck-engine suites and the bash-diff sweep are the regression net.
- **THE RULE:** the lexer emits atoms + a parser-pushed single-terminator
  subscript mode; the parser owns `[`…`]` matching and the `=` decision. No
  speculative delimiter scan, no lexer→parser call.

## File map

- `crates/huck-syntax/src/lexer.rs` — `TokenKind::AssignEq`; `pending_lvalue_subscript`
  field (+ Mark clone/restore); `begin_assignment_value`; replace
  `try_scan_assign_prefix` `Some('[')` branch; flag hook atop
  `scan_step_command_atoms_core`; delete `SubscriptParseError` (Task 2).
- `crates/huck-syntax/src/parser.rs` — `LBracket` arm in `parse_word_command`
  (subscript assembly + `AssignEq` decision + glob fold-back); delete
  `parse_fragment_word` + its tests (Task 2).
- `crates/huck-syntax/src/errors.rs` — remove the `SubscriptParseError` message
  arm (Task 2).
- `crates/huck-engine/src/executor.rs` — repoint two comments (Task 2).
- `tests/scripts/subscript_lvalue_diff_check.sh` — new (Task 3).

---

## Task 1: The atomic swap — lexer emits `LBracket`/`AssignEq`, parser assembles + decides

This task changes both sides together (a half-landed swap breaks every
command-word `a[i]=x`), so it is one task. It ends green with no new warnings.

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` — `enum TokenKind` (~577); `struct Lexer`
  fields (~864-872) + `Lexer::new` init (~1005-1017) + the Mark struct/clone/restore
  (~860, 1140-1181); `scan_step_command_atoms_core` top (~2876); `try_scan_assign_prefix`
  `Some('[')` branch (3733-3822); a new `begin_assignment_value` method.
- Modify: `crates/huck-syntax/src/parser.rs` — `parse_word_command` match (~172-320).
- Test: both files' `mod tests`.

**Interfaces produced (used by Task 2 / later):**
- `TokenKind::AssignEq { append: bool }` — zero-width token the parser consumes to
  confirm an indexed assignment.
- `Lexer::begin_assignment_value(&mut self, append: bool)` — parser→lexer; sets
  value-mode + the compound-array `(` probe.

- [ ] **Step 1: Write the failing tests (RED targets D1/D2 + regression guards)**

Add to `parser.rs` `mod tests` (use the crate's existing test helpers — assemble a
`Lexer::new_live_atoms(src, &empty, LexerOptions::default())` and
`parse_sequence`, then inspect the AST; mirror the shape of the existing
`parse_fragment_word` tests being replaced and the array-literal tests):
```rust
#[test]
fn cmdword_indexed_assignment_builds_indexed_target() {
    // Regression: a[i]=v and a[$(echo 2)]=v still produce AssignTarget::Indexed
    // with the right subscript Word (was the lexer bridge; now parser-assembled).
    use crate::command::{Command, SimpleCommand, AssignTarget};
    for (src, want_sub_lit) in [("a[0]=v", Some("0")), ("a[$(echo 2)]=v", None)] {
        let empty = std::collections::HashMap::new();
        let mut lx = crate::lexer::Lexer::new_live_atoms(src, &empty, crate::lexer::LexerOptions::default());
        let seq = parse_sequence(&mut lx).unwrap().unwrap();
        // Bare assignment → Command::Simple(SimpleCommand::Assign([a], _))
        let Command::Pipeline(p) = &seq.first else { panic!("pipeline") };
        let Command::Simple(SimpleCommand::Assign(items, _)) = &p.commands[0] else { panic!("assign, got {:?}", p.commands[0]) };
        assert_eq!(items.len(), 1);
        let AssignTarget::Indexed { name, subscript } = &items[0].target else { panic!("indexed") };
        assert_eq!(name, "a");
        if let Some(lit) = want_sub_lit {
            assert_eq!(subscript.0, vec![crate::lexer::WordPart::Literal { text: lit.into(), quoted: false }]);
        }
    }
}

#[test]
fn cmdword_bracket_no_eq_is_a_glob_word_not_assignment() {
    // D1: a[bc] and a[$x] with NO '=' are ordinary (glob) words — NOT assignments.
    // a[$x] now EXPANDS (fold-back), so its parts include a Var (was literal-swallowed).
    use crate::command::{Command, SimpleCommand};
    use crate::lexer::WordPart;
    let empty = std::collections::HashMap::new();
    // a[bc]: single literal word, program "a[bc]", no inline assignment.
    let mut lx = crate::lexer::Lexer::new_live_atoms("a[bc]", &empty, crate::lexer::LexerOptions::default());
    let seq = parse_sequence(&mut lx).unwrap().unwrap();
    let Command::Pipeline(p) = &seq.first else { panic!() };
    let Command::Simple(SimpleCommand::Exec(e)) = &p.commands[0] else { panic!("exec, got {:?}", p.commands[0]) };
    assert!(e.inline_assignments.is_empty(), "a[bc] must NOT be an assignment");
    assert_eq!(e.program.0, vec![WordPart::Literal { text: "a[bc]".into(), quoted: false }]);
    // a[$x]: program parts must contain a Var (proves expansion, D1 fix), not a literal "$x".
    let mut lx2 = crate::lexer::Lexer::new_live_atoms("a[$x]", &empty, crate::lexer::LexerOptions::default());
    let seq2 = parse_sequence(&mut lx2).unwrap().unwrap();
    let Command::Pipeline(p2) = &seq2.first else { panic!() };
    let Command::Simple(SimpleCommand::Exec(e2)) = &p2.commands[0] else { panic!() };
    assert!(e2.program.0.iter().any(|p| matches!(p, WordPart::Var { name, .. } if name == "x")),
        "a[$x] subscript must EXPAND (D1): {:?}", e2.program.0);
}

#[test]
fn cmdword_indexed_value_leading_tilde_expands() {
    // D2: a[0]=~/y — the value's leading ~ becomes a Tilde part (was literal).
    use crate::command::{Command, SimpleCommand, AssignTarget};
    use crate::lexer::WordPart;
    let empty = std::collections::HashMap::new();
    let mut lx = crate::lexer::Lexer::new_live_atoms("a[0]=~/y", &empty, crate::lexer::LexerOptions::default());
    let seq = parse_sequence(&mut lx).unwrap().unwrap();
    let Command::Pipeline(p) = &seq.first else { panic!() };
    let Command::Simple(SimpleCommand::Assign(items, _)) = &p.commands[0] else { panic!() };
    let AssignTarget::Indexed { .. } = &items[0].target else { panic!("indexed") };
    assert!(matches!(items[0].value.0.first(), Some(WordPart::Tilde(_))),
        "D2: leading ~ must be a Tilde part, got {:?}", items[0].value.0);
}
```
Add to `lexer.rs` `mod tests`:
```rust
#[test]
fn begin_assignment_value_sets_value_mode_and_tilde() {
    let empty = std::collections::HashMap::new();
    let mut lx = Lexer::new_live_atoms("x", &empty, LexerOptions::default());
    lx.begin_assignment_value(false);
    assert!(lx.in_assignment_value);
    assert!(lx.assign_val_tilde_ok, "D2: tilde enabled for indexed value");
    assert!(!lx.cmd_at_word_start);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `( ulimit -v 2500000; cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1 cmdword_ begin_assignment_value )`
Expected: `begin_assignment_value_…` fails to compile (no such method);
`cmdword_bracket_no_eq…` fails on the `a[$x]` Var assertion (today literal-swallowed);
`cmdword_indexed_value_leading_tilde…` fails (today literal `~`);
`cmdword_indexed_assignment_builds_indexed_target` likely PASSES already (old path
also builds Indexed) — that's fine, it is a regression guard. Note which are RED.

- [ ] **Step 3: Lexer — add `AssignEq`, the flag, and `begin_assignment_value`**

In `enum TokenKind` (near `LBracket, RBracket` at ~577), add:
```rust
    /// v268: the `=` / `+=` that follows a command-word-start subscript
    /// (`name[sub]=`). Emitted by the command scanner ONLY when
    /// `pending_lvalue_subscript` is set; the parser consumes it to confirm an
    /// indexed assignment and then calls `begin_assignment_value`.
    AssignEq { append: bool },
```
In `struct Lexer` (near `assign_val_tilde_ok` ~872) add:
```rust
    /// v268: set when the command scanner emitted a word-start `name[` `LBracket`;
    /// checked once, on the first command scan step after the parser assembles the
    /// subscript and consumes `RBracket`, to emit `AssignEq` (→ indexed assignment)
    /// or clear (→ ordinary glob word).
    pending_lvalue_subscript: bool,
```
In `Lexer::new` struct literal (~1005-1017) add `pending_lvalue_subscript: false,`.
Add `pending_lvalue_subscript` to the Mark struct (~860 region) and its
clone/restore (~1140-1181) so arith mark/rewind cannot desync it (mirror an
existing bool field like `in_assignment_value` at 864/1140/1173).

Add the method (in `impl Lexer`, near `try_scan_assign_prefix`):
```rust
    /// v268: enter assignment-value lexing for an indexed lvalue whose `AssignEq`
    /// the parser just consumed. Mirrors the value-mode state the old Indexed arm
    /// of `try_scan_assign_prefix` set, with `assign_val_tilde_ok = true` (D2 fix:
    /// a leading `~` in `a[i]=~/x` now expands, matching the scalar `name=` arm).
    /// Also runs the compound-array `(` probe so `a[i]=(…)` pushes `Mode::ArrayLiteral`.
    pub(crate) fn begin_assignment_value(&mut self, _append: bool) {
        self.cmd_at_word_start = false;
        self.in_assignment_value = true;
        self.assign_val_tilde_ok = true;
        skip_line_continuations(&mut self.cursor);
        if self.cursor.peek() == Some(&'(') {
            let (ao, al, ac) = (self.cursor.offset(), self.cursor.line(), self.cursor.column());
            self.history.push(Token::new(TokenKind::ArrayOpen, Span::new(ao, al, ac)));
        }
    }
```

- [ ] **Step 4: Lexer — replace the `Some('[')` branch + add the flag hook**

Replace the entire `Some('[')` arm of `try_scan_assign_prefix` (lexer.rs:3733-3822,
from `Some('[') => {` through its closing `}`) with: emit `Lit name` + `LBracket`,
set the flag, consume `[`, and return `Produced` — no scan, no parser call:
```rust
            // `name[` at command-word-start — a possible indexed lvalue OR an
            // ordinary glob word (`a[bc]`). The lexer no longer decides: emit the
            // name and a zero-width `LBracket`, set `pending_lvalue_subscript`, and
            // let the PARSER assemble the subscript and decide by the trailing
            // `AssignEq` (v268 — severs the old forward-scan + parse_fragment_word).
            Some('[') => {
                for _ in 0..name.len() { self.cursor.next(); }
                self.cursor.next(); // consume `[`
                self.cmd_at_word_start = false;
                self.pending_lvalue_subscript = true;
                self.history.push(Token::new(TokenKind::Lit { text: name, quoted: false }, Span::new(off, l, c)));
                let (bo, bl, bc) = (self.cursor.offset(), self.cursor.line(), self.cursor.column());
                self.history.push(Token::new(TokenKind::LBracket, Span::new(bo, bl, bc)));
                Ok(Some(Step::Produced))
            }
```
Add the flag hook at the TOP of `scan_step_command_atoms_core` (lexer.rs:2876,
BEFORE the blank-skip `if`, so a space after `]` correctly means glob):
```rust
        // v268: first command scan step after the parser assembled a word-start
        // `name[sub]` subscript. If `=`/`+=` immediately follows `]`, emit AssignEq
        // (→ indexed assignment); otherwise it was a glob word — clear and fall
        // through to ordinary scanning. Checked BEFORE the blank-skip so `a[i] =v`
        // (space) is NOT an assignment.
        if self.pending_lvalue_subscript {
            self.pending_lvalue_subscript = false;
            let off = self.cursor.offset(); let l = self.cursor.line(); let c = self.cursor.column();
            match self.cursor.peek().copied() {
                Some('=') => {
                    self.cursor.next();
                    self.history.push(Token::new(TokenKind::AssignEq { append: false }, Span::new(off, l, c)));
                    return Ok(Step::Produced);
                }
                Some('+') if { let mut p = self.cursor.clone(); p.next(); p.peek() == Some(&'=') } => {
                    self.cursor.next(); self.cursor.next(); // `+` `=`
                    self.history.push(Token::new(TokenKind::AssignEq { append: true }, Span::new(off, l, c)));
                    return Ok(Step::Produced);
                }
                _ => { /* glob: fall through to normal scanning below */ }
            }
        }
```
(The `p.peek()` one-char probe is a single-token disambiguation, not a
delimiter forward-scan — it never crosses `]`.)

- [ ] **Step 5: Parser — add the `LBracket` arm to `parse_word_command`**

In `parse_word_command`'s match (parser.rs ~172-320), add an arm for `LBracket`.
The name is the coalesced literal accumulated so far (`acc`); assemble the
subscript under `ParamSubscriptOperand` (mirror `parse_array_literal:1259-1271`),
then branch on `AssignEq`:
```rust
            Some(TokenKind::LBracket) => {
                iter.next_kind()?; // consume LBracket
                // The name is whatever literal was accumulated immediately before
                // `[` (the lexer emits `Lit name` then `LBracket`). Take it out of
                // the coalescing buffer; if there is none, treat `[` literally.
                let name = match acc.take() {
                    Some((n, false)) => n,
                    other => { acc = other; push_lit(&mut acc, &mut parts, "[".into(), quoted); continue; }
                };
                iter.push_mode(Mode::ParamSubscriptOperand { in_dquote: false, enclosing_dquote: false });
                let sub_word = match parse_word(iter, false) {
                    Ok(w) => w,
                    Err(e) => { iter.pop_mode(); return Err(e); }
                };
                match iter.next_kind() {
                    Ok(Some(TokenKind::RBracket)) => {}
                    Ok(_) => { iter.pop_mode(); return Err(ParseError::UnsupportedExpansion); }
                    Err(e) => { iter.pop_mode(); return Err(ParseError::Lex(Box::new(e))); }
                }
                iter.pop_mode();
                match iter.peek_kind()? {
                    Some(TokenKind::AssignEq { append }) => {
                        let append = *append;
                        iter.next_kind()?;                    // consume AssignEq
                        iter.begin_assignment_value(append);  // value-mode BEFORE value pull
                        parts.push(WordPart::AssignPrefix {
                            target: crate::command::AssignTarget::Indexed { name, subscript: sub_word },
                            append,
                        });
                        // value flows into this same word; try_split_assignment splits it.
                    }
                    _ => {
                        // Glob fold-back: name + `[` + subscript parts + `]` → word.
                        push_lit(&mut acc, &mut parts, name, quoted);
                        push_lit(&mut acc, &mut parts, "[".into(), quoted);
                        flush_lit(&mut acc, &mut parts);
                        parts.extend(sub_word.0);
                        push_lit(&mut acc, &mut parts, "]".into(), quoted);
                    }
                }
            }
```
Notes for the implementer: match the real signatures of `push_lit`/`flush_lit`
(they may take `&str`/`String`; adapt). Confirm `parse_word_command` is the sole
command-word assembler that must handle this (the array-element `LBracket` is
consumed inside `parse_array_literal`, a different loop — do NOT double-handle).
`parse_word` (the non-command variant) does NOT get this arm.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `( ulimit -v 2500000; cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1 cmdword_ begin_assignment_value )`
Expected: all PASS (D1/D2 now fixed; the regression guard still passes).

- [ ] **Step 7: Full huck-syntax crate (regression + warnings)**

Run: `( ulimit -v 2500000; cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1 )`
Expected: all pass, 0 warnings. If any existing subscript/array/assignment test
fails, reconcile against the spec's checklist before proceeding — a real behavior
change outside D1/D2 is a defect.

- [ ] **Step 8: Commit**

```bash
git add crates/huck-syntax/src/lexer.rs crates/huck-syntax/src/parser.rs
git commit -m "$(cat <<'EOF'
v268 T1: parser-recognize command-word indexed lvalue (sever the forward-scan)

The lexer no longer forward-scans name[...] or calls the parser: it emits Lit name
+ LBracket + a pending flag, then AssignEq after the subscript's ]. The parser
assembles the subscript under ParamSubscriptOperand and decides assignment-vs-glob
by the trailing AssignEq, calling begin_assignment_value for the value. Fixes D1
(a[$x] no-= now expands) and D2 (a[0]=~/y tilde expands).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Delete the bridge — `parse_fragment_word`, `SubscriptParseError`, comments

**Files:**
- Modify: `crates/huck-syntax/src/parser.rs` — delete `parse_fragment_word`
  (3289-3316) and its 4 tests (7440-7473).
- Modify: `crates/huck-syntax/src/lexer.rs` — delete `LexError::SubscriptParseError`
  (~18) and the doc mention at line 14.
- Modify: `crates/huck-syntax/src/errors.rs` — delete the `SubscriptParseError`
  message arm.
- Modify: `crates/huck-engine/src/executor.rs` — repoint the two comments
  (9225, 9245) to describe the parser-side assembly.
- Test: an invariant test in `lexer.rs` `mod tests`.

- [ ] **Step 1: Write the invariant test**
```rust
#[test]
fn lexer_has_no_production_parser_dependency() {
    // The one-way front-end invariant: no `crate::parser` reference in non-test
    // lexer code. Reads this source file and checks every `crate::parser` line is
    // inside the `#[cfg(test)]` tests module.
    let src = include_str!("lexer.rs");
    let test_mod = src.find("mod tests").unwrap_or(src.len());
    let prod = &src[..test_mod];
    assert!(!prod.contains("crate::parser"),
        "lexer production code must not reference crate::parser (one-way invariant)");
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `( ulimit -v 2500000; cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1 lexer_has_no_production_parser_dependency )`
Expected: FAIL — `crate::parser` still appears (the doc mention at lexer.rs:14, if
not already removed in Task 1; and confirm the call at 3769 is gone). If Task 1
already removed both, this test passes now — then this step just documents the
invariant; note that and proceed.

- [ ] **Step 3: Delete the surface**

- `parser.rs`: delete `pub fn parse_fragment_word(…)` (3289-3316) and the 4 tests
  in the `v266 T1` block (7440-7473). Grep first: `grep -rn parse_fragment_word
  crates/` must show ONLY these definitions/tests before deleting (Task 1 removed
  the lexer call).
- `lexer.rs`: delete `LexError::SubscriptParseError(…)` variant (~18) and the
  line-14 doc mention. Confirm no remaining producer/consumer: `grep -rn
  SubscriptParseError crates/`.
- `errors.rs`: delete the `LexError::SubscriptParseError => …` arm in
  `lex_error_message_impl` (the match is exhaustive — removing the variant forces
  this).
- `executor.rs`: reword the two comments at 9225/9245 to say the atom parser
  assembles the subscript (no `parse_fragment_word`).

- [ ] **Step 4: Run the invariant test + full crate**

Run: `( ulimit -v 2500000; cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1 lexer_has_no_production_parser_dependency )` → PASS.
Run: `( ulimit -v 2500000; cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1 )` → all pass, 0 warnings.

- [ ] **Step 5: Commit**
```bash
git add crates/huck-syntax/src/lexer.rs crates/huck-syntax/src/parser.rs crates/huck-syntax/src/errors.rs crates/huck-engine/src/executor.rs
git commit -m "$(cat <<'EOF'
v268 T2: delete parse_fragment_word + SubscriptParseError (bridge gone)

The command-word lvalue no longer uses the lexer->parser bridge, so
parse_fragment_word and LexError::SubscriptParseError are dead — removed, with an
invariant test asserting no crate::parser reference in production lexer code.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: bash-parity diff harness

**Files:**
- Create: `tests/scripts/subscript_lvalue_diff_check.sh`

- [ ] **Step 1: Write the harness**

Model it on `tests/scripts/alias_expand_diff_check.sh` (a `-c`-based
`bash`↔`$HUCK_BIN` byte-comparator with a PASS/FAIL tally and `exit 1` on any
fail). Cover the spec's checklist AND the D1/D2 fixes, each run in a fresh temp
dir where file existence matters. Cases:
```
# assignment (unchanged behavior)
declare -A m; a[0]=v; echo "${a[0]}"
a[$(echo 2)]=w; echo "${a[2]}"
a[1]=x; a[1]+=y; echo "${a[1]}"
declare -A m; m[k]=v; m[$k2]=... ; echo "${m[k]}"
a[i]=(x y z); echo "${a[@]}"           # compound RHS
a=1 a[2]=3 : ; declare -p a            # command-prefixed / inline list (adapt to bash-parity)
# D1 (now expands, matches bash):
cd "$tmp"; x=2; echo a[$x]             # -> a[2]
echo a[$(echo 2)]                      # -> a[2]
echo a[bc]                             # literal glob, unchanged
touch a2; x=2; echo a[$x]              # -> a2 (glob match)
# D2 (now expands):
HOME=/h; a[0]=~/y; echo "${a[0]}"      # -> /h/y
a[0]=p:~/y; echo "${a[0]}"            # -> p:/h/y (already correct)
# glob mixed shapes:
echo a[b] c ; echo a[b]c ; echo "a[b]=c"
```
Guard the run: `( ulimit -v 1500000; timeout 60 bash tests/scripts/subscript_lvalue_diff_check.sh )`.

- [ ] **Step 2: Build the binary + run the harness**

Run:
```bash
( ulimit -v 2500000; cargo build -p huck --jobs 1 )
export HUCK_BIN=$(pwd)/target/debug/huck
( ulimit -v 1500000; timeout 60 bash tests/scripts/subscript_lvalue_diff_check.sh )
```
Expected: all cases PASS (byte-identical bash↔huck), exit 0. If a case fails,
it is either a real defect or a case that is genuinely bash-divergent for an
UNRELATED reason (e.g. `declare -p` formatting) — in that case narrow the case to
the subscript behavior under test, don't assert an unrelated divergence.

- [ ] **Step 3: Commit**
```bash
git add tests/scripts/subscript_lvalue_diff_check.sh
git commit -m "$(cat <<'EOF'
v268 T3: subscript_lvalue_diff_check.sh — bash-parity for command-word lvalues

Byte-identical bash<->huck harness for a[i]=v (+ compound/append/assoc), the D1
glob-expansion fix (a[$x] no-= -> a[2]), and the D2 value-tilde fix (a[0]=~/y).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Final gate

**Files:** none (verification only).

- [ ] **Step 1: huck-syntax + huck-engine suites**

Run: `( ulimit -v 2500000; cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1 )` → all pass, 0 warnings.
Run: `( ulimit -v 2500000; cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 )` → all pass (~1740), 0 warnings.

- [ ] **Step 2: Full bash-diff sweep (no regression)**

```bash
( ulimit -v 2500000; cargo build -p huck --jobs 1 )
export HUCK_BIN=$(pwd)/target/debug/huck
pass=0; fail=0; fails=""
for f in tests/scripts/*_diff_check.sh; do
  ( ulimit -v 1500000; timeout 60 bash "$f" ) >/dev/null 2>&1 && pass=$((pass+1)) || { fail=$((fail+1)); fails="$fails $(basename $f)"; }
done
echo "sweep: pass=$pass fail=$fail"; echo "FAILS:$fails"
```
Expected: the new `subscript_lvalue_diff_check.sh` passes; the only failures are
the known pre-existing `cmdsub_comment_diff_check.sh` + `funcnest_diff_check.sh`
(i.e. `fail=2`, or `fail` unchanged from the pre-branch baseline). Any other
failure is a regression — stop and investigate.

- [ ] **Step 3: (no commit — verification task)**

If all gates pass, the branch is ready for the whole-branch review + merge.

---

## Self-review notes (author)

- **Spec coverage:** §1 (lexer emit `LBracket`) → T1 Step 4; §2 (`AssignEq`/flag) →
  T1 Steps 3-4; §3 (parser assembly + decision + glob fold-back) → T1 Step 5; §4
  (`begin_assignment_value`, tilde=true D2) → T1 Step 3; §5 (deletions) → T2;
  "Behavior changes" D1/D2 → T1 tests + T3 harness; testing → T1/T3/T4.
- **Placeholder scan:** none — every code step carries code or an exact anchor +
  skeleton; integration points that depend on real signatures (`push_lit`,
  Mark fields) are flagged for the implementer to match, not left vague.
- **Type consistency:** `AssignEq { append: bool }` defined T1-S3, consumed T1-S5;
  `pending_lvalue_subscript: bool` defined/managed T1-S3, set T1-S4, read/cleared
  T1-S4 hook; `begin_assignment_value(append)` defined T1-S3, called T1-S5;
  `AssignTarget::Indexed { name, subscript }` matches `command.rs:332`;
  `WordPart::AssignPrefix { target, append }` matches the existing arm
  (parser.rs:302-305) so `try_split_assignment` is unchanged.
- **Atomicity:** T1 changes both lexer and parser in one commit because a
  half-landed swap breaks command-word assignments — deliberate, and it ends green
  with no dead-code window (the new token/field/method are all exercised by T1).
