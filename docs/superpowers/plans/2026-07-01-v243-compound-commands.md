# v243 — Compound Commands in the Parser-Driven Command Parser Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extend v242's dormant flat command-list parser in `crates/huck-syntax/src/parser.rs` to the command-list-body compound commands (subshell, brace group, if/elif/else, while/until, for, select, case), differential-tested against `command.rs`.

**Architecture:** Add the recursion enabler — a `stop_at: &[Keyword]` parameter on `parse_and_or` (break, without consuming, on a peeked stop-keyword), a `parser`-local `Keyword` enum + `keyword_kind`, a `parse_compound_section` helper, and a `maybe_wrap_redirects` helper (→ `Command::Redirected`). Then replace v242's `parse_command` compound-deferral seams with dispatch to per-compound parsers, each mirroring a `command.rs` function. Compounds-as-pipeline-stages come for free (v242's `parse_pipeline` already routes every stage through `parse_command`). Dormant; the differential corpus (`command::parse` = oracle) is the proof.

**Tech Stack:** Rust, `crates/huck-syntax/src/{parser.rs, command.rs}`.

## Global Constraints

- **Byte-identical / dormant:** `parser.rs`'s parser is reached ONLY by tests; `command.rs`'s parser, `Command` mode, and the engine are untouched. `cargo test --workspace` green, 0 warnings; the release `*_diff_check.sh` harness sweep byte-identical.
- **No lexer change** — consume the production `Command`-mode tokens via the existing pull API (`peek_kind`/`peek2_kind`/`next_kind`/`peek_span`/`next`).
- **`command.rs` is the ORACLE** — on any `diff_cmd` mismatch, fix `parser.rs` to match; NEVER weaken the comparison or edit `command.rs`'s parser logic.
- **Lexer never scans ahead** (the committed direction): the compounds assemble structure from the FLAT token stream — the parser matches `(`/`)`/`{`/`}`/keyword/`;;`/`|` delimiters and recurses. No parser-driven-lexer-word-building; every `Word` passes through opaquely (the v242 interim).
- Reuse the existing AST verbatim (`IfClause`/`WhileClause`/`ForClause`/`CaseClause`/`SelectClause`/`Command::*`) — NO AST change. Reuse the existing `ParseError` variants (`UnsupportedCommand` for deferred forms; `UnterminatedIf`/`UnterminatedLoop`/`UnterminatedBrace`/`EmptySubshell`/etc. to match the oracle's errors) — no new variant.
- Permitted `command.rs` change: VISIBILITY-ONLY `pub(crate)` bumps on helpers `parser.rs` reuses (behavior-neutral; protects against copy-drift) — e.g. the `Keyword`/`keyword_of`/`maybe_wrap_redirects`/`parse_trailing_redirects` internals if reuse is cleaner than a mirror. Report each.
- Commit trailer on every commit: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

## Deferred → `ParseError::UnsupportedCommand` (keep as-is; corpus asserts the boundary in Task 7)

Arith command `(( … ))` (the `ArithBlock` seam), `[[ … ]]`, function-def (`name()` / `function name`), `coproc`, C-style `for (( … ))` (ArithFor), heredocs/here-strings.

## Reference (the ORACLE to mirror — read these in `command.rs`)

`Keyword` enum (~4), `keyword_of` (~57); `parse_sequence`/`parse_sequence_opts` (~872, the `stop_at` mechanism); `parse_compound_section` (~1270); `maybe_wrap_redirects` (~2115) + `parse_trailing_redirects` (~2055); `parse_brace_group` (~1764); `parse_subshell`/`parse_subshell_sequence` (~1780/1807); `parse_if` (~1282) + `IfClause`/`ElifBranch`; `parse_while` (~1886) + `WhileClause`; `parse_for_command`/`parse_for_after_keyword`/`parse_do_body_done` (~1487/1537/1522) + `ForClause`; `parse_select_command` (~1583) + `SelectClause`; `parse_case`/`parse_case_item` (~1673/1702) + `CaseClause`/`CaseItem`/`CaseTerminator`. The AST clause types are `pub` in `command.rs` (~644–720).

The v242 seams to replace live in `parser.rs`'s `parse_command` (~631–671): the `Op(LParen)`, `ArithBlock`, keyword (`keyword_of_tok`), and function-def arms.

---

### Task 1: Recursion enabler + brace group

**Files:** Modify `crates/huck-syntax/src/parser.rs` (+ optional `pub(crate)` bumps in `command.rs`).

**Interfaces produced (Tasks 2–7 depend on these EXACT names):**
- `enum Keyword { If, Then, Elif, Else, Fi, While, Until, Do, Done, For, In, Case, Esac, LBrace, RBrace, DoubleBracketOpen, DoubleBracketClose, Function, Select, Coproc }` (mirror `command.rs`'s `Keyword`).
- `fn keyword_kind(token: &TokenKind) -> Option<Keyword>` (mirror `command.rs`'s `keyword_of`: a `Word` of exactly one unquoted `Literal` part whose text is a keyword).
- `fn parse_and_or(iter: &mut Lexer, stop_at: &[Keyword]) -> Result<Sequence, ParseError>` (v242's `parse_and_or` gains the `stop_at` param).
- `fn parse_compound_section(iter: &mut Lexer, stop_at: &[Keyword], unterminated: ParseError) -> Result<Sequence, ParseError>`.
- `fn maybe_wrap_redirects(cmd: Command, iter: &mut Lexer) -> Result<Command, ParseError>`.

**What to build (mirror the cited `command.rs` fns):**
1. Add the `Keyword` enum + `keyword_kind` (mirror `keyword_of` ~57). Rewrite `keyword_of_tok` (v242) as `keyword_kind(token).is_some()` so there is ONE keyword table.
2. Add `stop_at: &[Keyword]` to `parse_and_or`, mirroring `command.rs`'s `parse_sequence_opts`: at each point the loop is about to parse a pipeline (INCLUDING before the first, and after consuming a `;`/newline/`&`), peek — if `keyword_kind(tok)` is in `stop_at`, break WITHOUT consuming. Reproduce the exact placement of the three stop checks (`command.rs` ~890/917/958). Update `parse_sequence` (the entry) to call `parse_and_or(iter, &[])`.
3. Add `parse_compound_section` (mirror ~1270): call `parse_and_or(iter, stop_at)`; if it returns `Err(MissingCommand)` AND `iter.peek_kind()?.is_none()`, return `unterminated`; else return as-is.
4. Add `maybe_wrap_redirects` (mirror ~2115): parse trailing redirects (loop the v242 `parse_one_redirect` while `command::next_is_redirect` — both already `pub(crate)` from v242); if any, return `Command::Redirected { inner: Box::new(cmd), redirects }`, else `cmd`.
5. In `parse_command`, replace the keyword seam so `keyword_kind(tok) == Some(Keyword::LBrace)` dispatches to `parse_brace_group`; every OTHER keyword (and the `ArithBlock`/`LParen`/`Heredoc`/function-def seams) still returns `UnsupportedCommand` (later tasks tighten these).
6. Add `parse_brace_group` (mirror ~1764): expect the `{` keyword, `let body = parse_compound_section(iter, &[Keyword::RBrace], ParseError::UnterminatedBrace)?`, expect `}`; return `maybe_wrap_redirects(Command::BraceGroup(Box::new(body)), iter)`.

- [ ] **Step 1: Write failing tests** — extend `parser.rs`'s test module (reuse v242's `diff_cmd`/`diff_unsupported`, and add `diff_err` for error parity):
```rust
fn diff_err(s: &str) { assert_eq!(new_seq(s), old_seq(s), "error mismatch for {s:?}"); }

#[test]
fn cmd_brace_group() {
    diff_cmd("{ a; }");
    diff_cmd("{ a; b; }");
    diff_cmd("{ a; b; c; }");
    diff_cmd("{ echo hi; }");
    diff_cmd("{ { a; } }");            // nested
    diff_cmd("{ a; } >f");             // trailing redirect -> Command::Redirected
    diff_cmd("{ a; } >f 2>&1");
    diff_cmd("{ a; } | cat");          // brace as pipeline stage
    diff_cmd("a | { b; }");
    diff_cmd("{ a; }; { b; }");        // two brace groups in a sequence
    diff_err("{ a");                   // UnterminatedBrace parity
}
```

- [ ] **Step 2: Run to verify they fail** — `cargo test -p huck-syntax --lib cmd_brace_group 2>&1 | tail` (mismatch / `UnsupportedCommand` from the v242 seam).
- [ ] **Step 3: Implement** items 1–6 above, mirroring the cited `command.rs` fns.
- [ ] **Step 4: Run to verify they pass** — `cargo test -p huck-syntax --lib cmd_brace_group 2>&1 | grep "test result"` PASS. Existing v242 tests still pass: `cargo test -p huck-syntax --lib cmd_ 2>&1 | grep "test result"`. Fix `parser.rs` to match the oracle on any mismatch.
- [ ] **Step 5: Commit**
```bash
git add crates/huck-syntax/src/parser.rs crates/huck-syntax/src/command.rs
git commit -m "v243 T1: recursion enabler (stop_at/Keyword/parse_compound_section/maybe_wrap_redirects) + brace group

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Subshell `( … )`

**Files:** Modify `crates/huck-syntax/src/parser.rs`.

**What to build:** `parse_subshell` (mirror `parse_subshell`/`parse_subshell_sequence` ~1780/1807): consume `Op(LParen)`; if the next token is `Op(RParen)` → `Err(EmptySubshell)`; else parse a sequence loop that terminates on (and consumes) `Op(RParen)` — this is a BESPOKE loop (NOT `parse_and_or(stop_at)`; subshell stops on `)`, not a keyword), reproducing `parse_subshell_sequence`'s connector handling exactly. Return `maybe_wrap_redirects(Command::Subshell { body: Box::new(body) }, iter)`. Replace the `Op(LParen)` seam in `parse_command` with this dispatch.

- [ ] **Step 1: Write failing tests:**
```rust
#[test]
fn cmd_subshell() {
    diff_cmd("( a )");
    diff_cmd("( a; b )");
    diff_cmd("( a | b )");
    diff_cmd("( a && b || c )");
    diff_cmd("( a; b; )");             // trailing ;
    diff_cmd("( (a) )");               // nested subshell
    diff_cmd("( { a; } )");            // brace group inside subshell
    diff_cmd("{ ( a ); }");            // subshell inside brace group
    diff_cmd("( a ) >f");              // trailing redirect
    diff_cmd("( a ) | b");            // subshell as pipeline stage
    diff_err("()");                    // EmptySubshell parity
    diff_err("( a");                   // unterminated parity
}
```
- [ ] **Step 2: Run to verify they fail** — `UnsupportedCommand` from the v242 `Op(LParen)` seam.
- [ ] **Step 3: Implement** `parse_subshell` + the dispatch, mirroring `parse_subshell_sequence`.
- [ ] **Step 4: Run to verify they pass** — PASS; oracle-fix on mismatch; `cmd_` all green.
- [ ] **Step 5: Commit**
```bash
git add crates/huck-syntax/src/parser.rs
git commit -m "v243 T2: subshell ( … )

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: `if … then … [elif …] [else …] fi`

**Files:** Modify `crates/huck-syntax/src/parser.rs`.

**What to build:** `parse_if` (mirror `parse_if` ~1282) → `Command::If(Box::new(IfClause{condition, then_body, elif_branches, else_body}))`: `expect` `if`; condition = `parse_compound_section(&[Then])`; `expect` `then`; then_body = `parse_compound_section(&[Elif, Else, Fi])`; loop `elif` (condition `[Then]`, body `[Elif,Else,Fi]`, push `ElifBranch`); optional `else` (body `[Fi]`); `expect` `fi`. Wrap via `maybe_wrap_redirects`. Add an `expect_keyword(iter, kw, err)` helper (mirror `command.rs`) if not already present. Dispatch `Keyword::If`.

- [ ] **Step 1: Write failing tests:**
```rust
#[test]
fn cmd_if() {
    diff_cmd("if x; then y; fi");
    diff_cmd("if x; then y; else z; fi");
    diff_cmd("if a; then b; elif c; then d; fi");
    diff_cmd("if a; then b; elif c; then d; else e; fi");
    diff_cmd("if a; then b; elif c; then d; elif e; then f; fi");   // multi-elif
    diff_cmd("if x; then if y; then z; fi; fi");                    // nested if
    diff_cmd("if x; then a; b; c; fi");                             // multi-command body
    diff_cmd("if x | y; then z; fi");                               // pipeline condition
    diff_cmd("if x; then y; fi | cat");                            // if as pipeline stage
    diff_cmd("if x; then y; fi >f");                               // trailing redirect
    diff_err("if x; then y");                                       // UnterminatedIf parity
}
```
- [ ] **Step 2: Run to verify they fail** — `UnsupportedCommand` from the keyword seam.
- [ ] **Step 3: Implement** `parse_if` + `expect_keyword` + dispatch, mirroring `command.rs`.
- [ ] **Step 4: Run to verify they pass** — PASS; oracle-fix; `cmd_` green.
- [ ] **Step 5: Commit**
```bash
git add crates/huck-syntax/src/parser.rs
git commit -m "v243 T3: if/elif/else

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: `while`/`until … do … done`

**Files:** Modify `crates/huck-syntax/src/parser.rs`.

**What to build:** `parse_while` (mirror `parse_while` ~1886) → `Command::While(Box::new(WhileClause{condition, body, until}))`: consume `while`/`until` (set `until` from which); condition = `parse_compound_section(&[Do])`; `expect` `do`; body = `parse_compound_section(&[Done])`; `expect` `done`. Wrap via `maybe_wrap_redirects`. Dispatch `Keyword::While | Keyword::Until`.

- [ ] **Step 1: Write failing tests:**
```rust
#[test]
fn cmd_while_until() {
    diff_cmd("while x; do y; done");
    diff_cmd("until x; do y; done");
    diff_cmd("while x; do a; b; done");
    diff_cmd("while x | y; do z; done");                           // pipeline condition
    diff_cmd("while x; do if y; then z; fi; done");                // nested if in body
    diff_cmd("while x; do while y; do z; done; done");             // nested loop
    diff_cmd("until x; do ( a ); done");                           // subshell in body
    diff_cmd("while x; do y; done | cat");                        // as pipeline stage
    diff_cmd("while x; do y; done <f");                           // trailing redirect
    diff_err("while x; do y");                                     // UnterminatedLoop parity
}
```
- [ ] **Step 2: Run to verify they fail.**
- [ ] **Step 3: Implement** `parse_while` + dispatch.
- [ ] **Step 4: Run to verify they pass** — PASS; oracle-fix; `cmd_` green.
- [ ] **Step 5: Commit**
```bash
git add crates/huck-syntax/src/parser.rs
git commit -m "v243 T4: while/until loops

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: `for` (POSIX) + `select`

**Files:** Modify `crates/huck-syntax/src/parser.rs`.

**What to build:**
- A shared `parse_do_body_done(iter)` (mirror ~1522): skip `;`/newlines, `expect` `do`, `parse_compound_section(&[Done])`, `expect` `done`.
- `parse_for` (mirror `parse_for_command`/`parse_for_after_keyword` ~1487/1537) → `Command::For(Box::new(ForClause{var, words, has_in, body}))`: consume `for`; **if the token after `for` is `ArithBlock` → `UnsupportedCommand`** (C-style ArithFor deferred); else read the variable NAME word, optional `in` + word-list (consume `Word`s until `Newline`/`Op(Semi)`/the `do` keyword — bespoke, mirror `command.rs`), then `parse_do_body_done`. Wrap via `maybe_wrap_redirects`.
- `parse_select` (mirror `parse_select_command` ~1583) → `Command::Select(Box::new(SelectClause{var, words, body}))`: like `for` but `words: Option<Vec<Word>>` (`None` = no `in`). Wrap via `maybe_wrap_redirects`.
- Dispatch `Keyword::For` → `parse_for`, `Keyword::Select` → `parse_select`.

- [ ] **Step 1: Write failing tests:**
```rust
#[test]
fn cmd_for_select() {
    diff_cmd("for i in a b c; do echo $i; done");
    diff_cmd("for i; do x; done");               // no-`in`
    diff_cmd("for i in; do x; done");            // empty in-list
    diff_cmd("for i in a; do for j in b; do x; done; done");   // nested
    diff_cmd("for i in a b; do if x; then y; fi; done");
    diff_cmd("for i in a; do x; done | cat");    // as pipeline stage
    diff_cmd("for i in a; do x; done 2>&1");     // trailing redirect
    diff_cmd("select x in a b; do y; done");
    diff_cmd("select x; do y; done");            // no-`in`
    diff_cmd("select x in a b c; do echo $x; break; done");
    diff_unsupported("for ((i=0;i<3;i++)); do x; done");   // ArithFor deferred
    diff_err("for i in a; do x");                // unterminated parity
}
```
(Every construct in this test lands in Task 1–5; `case`-containing nesting is exercised in Task 6.)
- [ ] **Step 2: Run to verify they fail.**
- [ ] **Step 3: Implement** `parse_do_body_done` + `parse_for` + `parse_select` + dispatch.
- [ ] **Step 4: Run to verify they pass** — PASS; oracle-fix; `cmd_` green.
- [ ] **Step 5: Commit**
```bash
git add crates/huck-syntax/src/parser.rs
git commit -m "v243 T5: for (POSIX) + select; C-style for(()) deferred

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: `case … in … esac`

**Files:** Modify `crates/huck-syntax/src/parser.rs`.

**What to build:** `parse_case`/`parse_case_item` (mirror ~1673/1702) → `Command::Case(Box::new(CaseClause{subject, items}))`:
- `parse_case`: consume `case`; read subject `Word`; `expect` `in`; loop `parse_case_item` until the `esac` keyword; `expect` `esac`.
- `parse_case_item`: optional leading `Op(LParen)`; pattern list = `Word` then `(Op(Pipe) Word)*`; `expect` `Op(RParen)`; body = `parse_compound_section(&[Esac])` — which must ALSO break on the case terminators `Op(DoubleSemi)`/`Op(SemiAmp)`/`Op(DoubleSemiAmp)` (confirm `parse_and_or` breaks on these, mirroring `parse_sequence_opts`; add the break if v242 didn't — the differential pins it); terminator → `CaseTerminator::{Break, FallThrough, ContinueMatch}` (or implicit `Break` if the next token is `esac`); `body: Option<Sequence>` is `None` for an empty body.
- Wrap via `maybe_wrap_redirects`. Dispatch `Keyword::Case`.

- [ ] **Step 1: Write failing tests:**
```rust
#[test]
fn cmd_case() {
    diff_cmd("case $x in a) 1;; esac");
    diff_cmd("case $x in a) 1;; b) 2;; esac");
    diff_cmd("case $x in a|b|c) 1;; esac");       // pattern list
    diff_cmd("case $x in (a) 1;; esac");          // leading paren
    diff_cmd("case x in a) ;; esac");             // empty body
    diff_cmd("case x in a) 1;; *) 2;; esac");     // default
    diff_cmd("case $x in a) 1;& b) 2;; esac");    // ;& fallthrough
    diff_cmd("case $x in a) 1;;& b) 2;; esac");   // ;;& continue-match
    diff_cmd("case $x in a) if y; then z; fi;; esac");  // compound in body
    diff_cmd("case $x in a) case $y in b) c;; esac;; esac");  // nested case
    diff_cmd("case $x in a) 1;; esac | cat");    // case as pipeline stage
    diff_cmd("case $x in a) 1;; esac >f");        // trailing redirect
    diff_cmd("for i in a; do case $i in q) x;; esac; done");  // case in for body
    diff_err("case x in");                         // unterminated parity
}
```
- [ ] **Step 2: Run to verify they fail.**
- [ ] **Step 3: Implement** `parse_case`/`parse_case_item` + the terminator breaks in `parse_and_or` + dispatch.
- [ ] **Step 4: Run to verify they pass** — PASS; oracle-fix; `cmd_` green.
- [ ] **Step 5: Commit**
```bash
git add crates/huck-syntax/src/parser.rs
git commit -m "v243 T6: case … in … esac (pattern lists + terminators)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 7: Deferred-boundary corpus + deep-nesting sweep + full proof

**Files:** Modify `crates/huck-syntax/src/parser.rs` (tests only).

**What to build:** No new parser code — the compound-as-pipeline-stage behavior already works (every stage routes through `parse_command`). Add the deferred-boundary corpus, a cross-compound deep-nesting sweep, and run the full verification.

- [ ] **Step 1: Write the tests:**
```rust
#[test]
fn cmd_compound_deferred_still() {
    diff_unsupported("(( 1+2 ))");            // arith command (ArithBlock seam)
    diff_unsupported("(( x + $y ))");
    diff_unsupported("[[ -n x ]]");           // test grammar
    diff_unsupported("f() { x; }");           // function def (name())
    diff_unsupported("function f { x; }");    // function def (keyword)
    diff_unsupported("coproc x");
    diff_unsupported("for ((i=0;i<3;i++)); do x; done");   // ArithFor
    diff_unsupported("cat <<<w");             // here-string
}

#[test]
fn cmd_deep_nesting() {
    diff_cmd("if x; then while y; do case $z in a) ( b );; esac; done; fi");
    diff_cmd("{ for i in a b; do if $i; then echo $i; fi; done; }");
    diff_cmd("while x; do { a; ( b ); }; done");
    diff_cmd("case $x in a) for i in 1 2; do echo $i; done;; b) { y; };; esac");
    diff_cmd("( if x; then y; else z; fi ) | { cat; }");
}
```
- [ ] **Step 2: Run to verify** — `cargo test -p huck-syntax --lib cmd_ 2>&1 | grep "test result"` — all PASS (oracle-fix any deep-nesting mismatch in `parser.rs`).
- [ ] **Step 3: Full proof:**
  - `cargo test --workspace 2>&1 | grep -E "test result:" | awk '{p+=$4;f+=$6} END{print "passed="p" failed="f}'` — 0 failed.
  - `cargo build --workspace 2>&1 | grep -c "^warning"` — 0.
  - Release harness sweep:
    ```bash
    cargo build --release 2>&1 | tail -1
    H="$(pwd)/target/release/huck"; n=0; f=0
    for s in tests/scripts/*_diff_check.sh; do n=$((n+1)); HUCK_BIN="$H" timeout 90 bash "$s" >/dev/null 2>&1 || { echo "FAIL $(basename $s)"; f=$((f+1)); }; done
    echo "harness $n scripts $f fail"
    ```
    Expected: 0 fail.
- [ ] **Step 4: Commit**
```bash
git add crates/huck-syntax/src/parser.rs
git commit -m "v243 T7: deferred-boundary + deep-nesting corpus; full workspace/harness proof

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Notes for the implementer

- **`command.rs` is the oracle**: on a `diff_cmd`/`diff_err` mismatch, change `parser.rs` to match `old_seq`/`command::parse` — never weaken the assertion, never edit `command.rs`'s parser logic. Mirror the cited functions for exact behavior (section stop sets, the subshell `)`-terminator loop, `for`/`select` word-list termination, the `case` pattern grammar + terminators, `maybe_wrap_redirects` placement).
- Do NOT touch `Command` mode, `command.rs`'s parser logic, or any engine crate. `command.rs` changes are limited to VISIBILITY-ONLY `pub(crate)` bumps for reuse.
- Keep every deferred construct (arith command, `[[ ]]`, function-def, coproc, ArithFor, heredocs) returning `UnsupportedCommand`; Task 7's corpus asserts the boundary.
- Line numbers are approximate (`command.rs` is ~6000 lines) — locate by symbol name.
