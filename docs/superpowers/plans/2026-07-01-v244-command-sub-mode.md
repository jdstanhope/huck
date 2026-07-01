# v244 — Command Substitution `$( … )` Lexer Mode Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Invert `$( … )` command substitution into the parser-driven front-end — a `CommandSub` lexer mode emits flat atoms and `parser::parse_command_sub` assembles `WordPart::CommandSub`, reusing v243's `parse_subshell_sequence` for the body — dormant and differential-tested against the production lexer.

**Architecture:** Fill the already-declared `Mode::CommandSub` stub with `scan_step_command_sub` (emit a `CmdSubOpen` atom on `$(`, then delegate the body to the existing `scan_step_command` one token at a time — NO scan-ahead; the terminating `)` is a normal `Op(RParen)` the parser consumes). `parse_command_sub(iter, quoted)` pushes the mode, parses the body via `parse_subshell_sequence` (empty `$()` → an empty `Sequence`, special-cased), pops, and returns `WordPart::CommandSub`. Wire it into v241's operand `DeferredExpansion` so `${x:-$(cmd)}` parses end-to-end. The differential harness (`new` = new path vs `old` = production lexer oracle) is the proof.

**Tech Stack:** Rust, `crates/huck-syntax/src/{lexer.rs, parser.rs}`.

## Global Constraints

- **Byte-identical / dormant:** the PRODUCTION word-scanning path (`scan_step_command` / `scan_dollar_expansion` / `scan_paren_substitution`) is UNCHANGED; nothing in production pushes `Mode::CommandSub`. The new mode + `parse_command_sub` + the operand-atom change are reached ONLY by tests and the dormant `parser.rs` path. `cargo test --workspace` green, 0 warnings; release `*_diff_check.sh` harness sweep byte-identical.
- **`command.rs` untouched** — reuse `parse_subshell_sequence` and the command parser as-is. Changes live in `lexer.rs` (new mode + atom + `scan_step` arm + the operand-atom change) and `parser.rs` (`parse_command_sub`, operand wiring, corpus).
- **`command.rs` is the differential ORACLE** — `WordPart::CommandSub { sequence, quoted }` from the production lexer. On mismatch, fix the NEW path; never weaken the comparison.
- **The lexer NEVER scans ahead for the matching `)`** — `CommandSub` mode emits the open atom then one Command-mode token at a time; the PARSER (`parse_subshell_sequence`) owns the `)` matching. Per-frame state lives IN the `Mode::CommandSub` variant (the v241 `ParamExpansion { seen_name }` pattern) so it is `mark`/`rewind`-safe.
- Reuse `WordPart::CommandSub` (no AST change), `ParseError::UnsupportedExpansion` (v241, for deferred cases), and v241's `DeferredExpansion` atom (kept for `$((`/backtick). No new `ParseError` variant.
- Commit trailer on every commit: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

## Deferred (corpus asserts the boundary in Task 5)

Backticks `` `…` `` (own iteration), `$(( ))` arith expansion, `$((`-adjacent comsub (`$((` no space — corpus writes the spaced `$( (…) )`), comsub bodies containing a command-parser-deferred construct (arith command, `[[ ]]`, function-def, coproc → the comsub defers), and atom-izing the body's words.

## Reference (read these)

Production oracle path in `lexer.rs`: `scan_dollar_expansion` (~2939, the `$(` arm + `$((` disambiguation ~2946), `scan_paren_substitution` (~3926), `scan_cmdsub_body` (~3416, the scan-ahead being replaced), `parse_substitution_body` (~3938, `command::parse` on the body + `empty_sequence` for `$()`). The `Mode` enum (~526), `scan_step` dispatch (~708), `push_mode`/`pop_mode` (~625). v241 atoms `TokenKind::{DeferredExpansion, …}` (~396); `scan_step_param_operand` (the operand scanner that emits `DeferredExpansion` for `$(`). `WordPart::CommandSub` (~307). In `parser.rs`: v241's `parse_param_expansion`/`parse_word` + the `old_part`/`new_part`/`diff_ok` harness; v243's `parse_subshell_sequence` (stops on `Op(RParen)`).

---

### Task 1: `CommandSub` mode + `parse_command_sub` + differential harness (simple + empty body)

**Files:** Modify `crates/huck-syntax/src/lexer.rs`, `crates/huck-syntax/src/parser.rs`.

**Interfaces produced (Tasks 2–5 depend on these):**
- `TokenKind::CmdSubOpen` (new atom).
- `Mode::CommandSub { body_started: bool }` (replaces the fieldless `CommandSub` stub).
- `fn Lexer::scan_step_command_sub(&mut self, body_started: bool) -> Result<Step, LexError>`.
- `pub(crate) fn parser::parse_command_sub(iter: &mut Lexer, quoted: bool) -> Result<WordPart, ParseError>`.
- test helpers `old_cs`/`new_cs`/`diff_cs`/`diff_cs_deferred`.

**What to build (mirror v241's ParamExpansion mechanism + the production oracle):**
1. `lexer.rs`: add `TokenKind::CmdSubOpen`. Change `Mode::CommandSub` → `Mode::CommandSub { body_started: bool }`; update the `scan_step` dispatch arm from `unreachable!` to `Mode::CommandSub { body_started } => self.scan_step_command_sub(body_started)`. Update any exhaustive `Mode` match (e.g. in `mark`/`rewind` clone — the `Vec<Mode>` clone is fine; check `Debug`).
2. `scan_step_command_sub(body_started)`:
   - `body_started == false`: consume `$` then `(` (assert they are there). Peek the next char: if `(` → emit `TokenKind::DeferredExpansion` (defer `$((`; leave the state as-is — `parse_command_sub` will pop+defer). Else → set the mode to `Mode::CommandSub { body_started: true }` (mutate the top-of-stack, as v241's head mode flips `seen_name`) and emit `TokenKind::CmdSubOpen`.
   - `body_started == true`: delegate to `self.scan_step_command()` (the body is Command-mode tokens; the terminating `)` comes out as `Op(RParen)`).
3. `parser.rs` `parse_command_sub(iter, quoted)`:
   - `iter.push_mode(Mode::CommandSub { body_started: false })`.
   - pull the first atom: `DeferredExpansion` → `iter.pop_mode(); return Err(ParseError::UnsupportedExpansion)`. `CmdSubOpen` → continue. (anything else → the same `UnsupportedExpansion`/an internal error, matching the oracle.)
   - **empty body:** if `iter.peek_kind()?` is `Some(Op(RParen))` immediately → consume it, `sequence = <empty Sequence>` (reuse `command`'s `empty_sequence` if `pub(crate)`, else construct the SAME value the oracle's `parse_substitution_body` yields for `$()` — the differential pins it).
   - else `sequence = parse_subshell_sequence(iter)?` (it consumes the terminating `)`).
   - `iter.pop_mode(); Ok(WordPart::CommandSub { sequence, quoted })`.
4. Differential harness in `parser.rs` tests:
```rust
fn old_cs(s: &str, quoted: bool) -> WordPart {
    let src = if quoted { format!("\"{s}\"") } else { s.to_string() };
    let toks = tokenize_with_opts(&src, LexerOptions::default()).expect("old lex");
    match &toks[0].kind {
        TokenKind::Word(w) => w.0.iter().find(|p| matches!(p, WordPart::CommandSub { .. }))
            .expect("no comsub part in production token").clone(),
        _ => panic!("production token is not a Word for {src:?}"),
    }
}
fn new_cs(s: &str, quoted: bool) -> Result<WordPart, ParseError> {
    let mut lx = Lexer::new_live(s, &Default::default(), LexerOptions::default());
    parse_command_sub(&mut lx, quoted)
}
fn diff_cs(s: &str) {
    assert_eq!(new_cs(s, false).unwrap(), old_cs(s, false), "unquoted {s:?}");
    assert_eq!(new_cs(s, true).unwrap(),  old_cs(s, true),  "quoted   {s:?}");
}
fn diff_cs_deferred(s: &str) {
    assert!(matches!(new_cs(s, false), Err(ParseError::UnsupportedExpansion)),
            "expected deferred for {s:?}, got {:?}", new_cs(s, false));
}
```

- [ ] **Step 1: Write the failing tests:**
```rust
#[test]
fn cs_simple() {
    diff_cs("$(echo hi)");
    diff_cs("$(echo hi there)");
    diff_cs("$(true)");
    diff_cs("$()");            // empty -> empty Sequence (NOT EmptySubshell)
}
```
- [ ] **Step 2: Run to verify they fail** — `cargo test -p huck-syntax --lib cs_simple 2>&1 | tail` (mode was `unreachable!` / fn undefined).
- [ ] **Step 3: Implement** items 1–4. If `cs_simple` mismatches, fix the new path to match `old_cs` (the oracle) — especially the empty-`$()` `Sequence` value.
- [ ] **Step 4: Run to verify they pass** — `cargo test -p huck-syntax --lib cs_simple 2>&1 | grep "test result"` PASS. Confirm production untouched: `cargo test -p huck-syntax 2>&1 | grep "test result" | tail`. `cargo build --workspace 2>&1 | grep -c "^warning"` = 0.
- [ ] **Step 5: Commit**
```bash
git add crates/huck-syntax/src/lexer.rs crates/huck-syntax/src/parser.rs
git commit -m "v244 T1: CommandSub mode + parse_command_sub + differential harness (simple + empty body)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Body grammar — multi-command, connectors, compound bodies

**Files:** Modify `crates/huck-syntax/src/parser.rs` (tests; fixes only if a case mismatches).

**What this covers:** the body is parsed by `parse_subshell_sequence` (v243), so multi-command / connector / compound bodies should already work — this task proves it against the oracle and fixes any divergence.

- [ ] **Step 1: Write the failing tests:**
```rust
#[test]
fn cs_body_grammar() {
    diff_cs("$(a; b)");
    diff_cs("$(a; b; c)");
    diff_cs("$(a | b)");
    diff_cs("$(a | b | c)");
    diff_cs("$(a && b || c)");
    diff_cs("$(a; b;)");                       // trailing ;
    diff_cs("$(a &)");                          // background in body
    diff_cs("$(if x; then y; fi)");             // compound body (v243)
    diff_cs("$(for i in a b; do echo $i; done)");
    diff_cs("$(while x; do y; done)");
    diff_cs("$(case $z in a) b;; esac)");
    diff_cs("$( (echo x) )");                   // comsub of a subshell (SPACED)
    diff_cs("$({ echo x; })");                  // comsub of a brace group
}
```
- [ ] **Step 2: Run to verify they fail/pass** — `cargo test -p huck-syntax --lib cs_body_grammar 2>&1 | tail`. (Many may pass immediately since `parse_subshell_sequence` handles them.)
- [ ] **Step 3: Fix to oracle** — for any mismatch, fix `parse_command_sub`/its body handling to match `old_cs`. Do NOT weaken a case.
- [ ] **Step 4: Run to verify they pass** — PASS; `cs_*` all green.
- [ ] **Step 5: Commit**
```bash
git add crates/huck-syntax/src/parser.rs
git commit -m "v244 T2: comsub body grammar (multi-command/connectors/compounds)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Nesting + quoted `"$(…)"` + redirect bodies

**Files:** Modify `crates/huck-syntax/src/parser.rs` (tests; fixes only if a case mismatches).

**What this covers:** nesting (the OUTER comsub is parser-driven; a nested `$(inner)` in a body word is fat-built by `scan_step_command` and passes through), the `quoted` flag, and bodies that are bare redirects.

- [ ] **Step 1: Write the failing tests:**
```rust
#[test]
fn cs_nesting_quoting() {
    diff_cs("$(echo $(date))");               // nested: inner fat-built, outer new-path
    diff_cs("$(echo ${x})");                  // ${…} in a body word (fat-built, passes through)
    diff_cs("$(a $(b) $(c))");                // two nested
    diff_cs("$(echo \"$(date)\")");           // nested inside dquotes in the body
    diff_cs("$(<file)");                       // body is a bare redirect
    diff_cs("$(cat < in > out)");
    diff_cs("$(echo hi\n)");                   // trailing newline in body
}
// diff_cs already checks BOTH unquoted and "…"-quoted for every case above,
// so the quoted `"$(…)"` path is exercised throughout.
```
- [ ] **Step 2: Run to verify** — `cargo test -p huck-syntax --lib cs_nesting_quoting 2>&1 | tail`.
- [ ] **Step 3: Fix to oracle** — any mismatch (esp. the nested-comsub `Sequence` or the `quoted` flag) → fix the new path to match `old_cs`.
- [ ] **Step 4: Run to verify they pass** — PASS; `cs_*` green.
- [ ] **Step 5: Commit**
```bash
git add crates/huck-syntax/src/parser.rs
git commit -m "v244 T3: comsub nesting + quoted + redirect bodies

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Wire into v241 operand `DeferredExpansion` (`${x:-$(cmd)}`)

**Files:** Modify `crates/huck-syntax/src/lexer.rs` (the operand scanner), `crates/huck-syntax/src/parser.rs` (`parse_word` dispatch + corpus).

**The contract to preserve:** `parse_command_sub(iter, quoted)` (Task 1) is entered with the lexer positioned so that pushing `Mode::CommandSub { body_started: false }` and pulling yields `CmdSubOpen` (i.e. it owns consuming `$(`). Keep that ONE entry point; the operand path must hand off at the same position.

**What to build:**
1. `lexer.rs` `scan_step_param_operand`: read how v241 emits `TokenKind::DeferredExpansion` for `$(` (what it consumes before emitting). Change it so that for `$(`-NOT-followed-by-`(` it stops at the SAME position `parse_command_sub` expects (lexer at `$(`), signalling the comsub via an atom `parse_word` can dispatch on — reuse `CmdSubOpen`, or a dedicated "operand comsub starts here" atom, WITHOUT consuming into the body. Keep emitting `DeferredExpansion` for `$((` and backtick (still deferred). If v241's current code consumes `$(` before emitting, either (a) stop emitting the marker BEFORE consuming `$(`, or (b) rewind one atom — choose the one that lets `parse_command_sub` run unchanged, and REPORT which. The `diff_ok` corpus is the check.
2. `parser.rs` `parse_word`: on the comsub-start atom mid-operand, call `parse_command_sub(iter, in_dquote_of_the_operand)` and push the returned `WordPart::CommandSub` onto the operand's parts; continue the operand loop. On `DeferredExpansion` (now only `$((`/backtick), keep the v241 behavior (`Err(UnsupportedExpansion)`).

Reconcile the mode stack: the operand mode is on the stack when `parse_command_sub` pushes `CommandSub`; after it pops, the operand mode resumes. Confirm the four operand modes (`ParamWordOperand`/`ParamSubstPatternOperand`/`ParamSubstringOffsetOperand`/`ParamSubscriptOperand`) all compose.

- [ ] **Step 1: Write the failing tests** (using v241's `diff_ok`, which compares the whole `${…}` `WordPart::ParamExpansion` against the production oracle):
```rust
#[test]
fn cs_in_param_operand() {
    diff_ok("${x:-$(echo d)}");
    diff_ok("${x:+$(cmd)}");
    diff_ok("${x=$(a b)}");
    diff_ok("${x:-a$(b)c}");                   // comsub between literals in an operand
    diff_ok("${x/$(a)/$(b)}");                 // pattern + replacement operands
    diff_ok("${x:-$(echo $(date))}");          // nested comsub inside an operand
}
```
- [ ] **Step 2: Run to verify they fail** — `cargo test -p huck-syntax --lib cs_in_param_operand 2>&1 | tail` (v241 deferred these with `UnsupportedExpansion`).
- [ ] **Step 3: Implement** the operand-atom change + the `parse_word` dispatch. Fix to `old_part` (the v241 oracle) on any mismatch.
- [ ] **Step 4: Run to verify they pass** — PASS; ALL prior tests green (`cargo test -p huck-syntax --lib 2>&1 | grep "test result"`) — the operand-atom change must not regress v241's `${…}` differential.
- [ ] **Step 5: Commit**
```bash
git add crates/huck-syntax/src/lexer.rs crates/huck-syntax/src/parser.rs
git commit -m "v244 T4: comsub inside \${…} operands (wire operand DeferredExpansion → parse_command_sub)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Deferred-boundary + error-parity corpus + full proof

**Files:** Modify `crates/huck-syntax/src/parser.rs` (tests only).

- [ ] **Step 1: Write the tests:**
```rust
#[test]
fn cs_deferred_boundary() {
    diff_cs_deferred("$((1+2))");              // arith expansion (WordPart::Arith, not comsub)
    diff_cs_deferred("$(( a + b ))");
    diff_cs_deferred("`echo hi`");             // backtick (own iteration)
    diff_cs_deferred("$([[ -n x ]])");         // body defers ([[ ]])
    diff_cs_deferred("$(f() { x; })");         // body defers (function-def)
    diff_cs_deferred("$(coproc x)");           // body defers (coproc)
}
#[test]
fn cs_error_parity() {
    // unterminated: the new path errors like the oracle. Compare the Err values.
    let s = "$(echo";
    let new = new_cs(s, false);
    // old path: tokenize errors (UnterminatedSubstitution). Assert the new path also Errs
    // (exact variant match if reachable; otherwise assert it is an Err, not a panic/wrong Ok).
    assert!(new.is_err(), "unterminated comsub must Err, got {new:?}");
}
```
(Note: `$((echo x))` — the bare `$((` no-space case — is covered by `$((1+2))`/`$(( a + b ))` deferring; if you want an explicit "comsub-of-subshell without space defers" case, add `diff_cs_deferred("$((echo);(echo))")` and confirm it Errs — but keep it only if it lexes.)
- [ ] **Step 2: Run to verify** — `cargo test -p huck-syntax --lib cs_deferred_boundary cs_error_parity 2>&1 | tail`. Fix `parse_command_sub` if a deferral doesn't fire (e.g. `$((` must emit `DeferredExpansion`; a body-deferred construct must propagate `UnsupportedExpansion` from `parse_subshell_sequence`).
- [ ] **Step 3: Full proof:**
  - `cargo test -p huck-syntax --lib cs_ 2>&1 | grep "test result"` — all PASS.
  - `cargo test --workspace 2>&1 | grep -E "test result:" | awk '{p+=$4;f+=$6} END{print "passed="p" failed="f}'` — 0 failed.
  - `cargo build --workspace 2>&1 | grep -c "^warning"` — 0.
  - Release harness sweep: `cargo build --release 2>&1 | tail -1`, then for `tests/scripts/*_diff_check.sh` run each with `HUCK_BIN=$(pwd)/target/release/huck` + a 90s timeout; report `N scripts, F failures`. Expected 0 fail (production path unchanged).
- [ ] **Step 4: Commit**
```bash
git add crates/huck-syntax/src/parser.rs
git commit -m "v244 T5: comsub deferred-boundary + error-parity corpus; full workspace/harness proof

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Notes for the implementer

- **The production lexer is the ORACLE**: on a `diff_cs`/`diff_ok` mismatch, fix the NEW path to match `old_cs`/`old_part` — never weaken the assertion.
- Do NOT touch the production word-scanning path (`scan_dollar_expansion`/`scan_paren_substitution`/`scan_cmdsub_body`) — it must stay byte-identical. `Mode::CommandSub` is reached only when `parse_command_sub` pushes it (tests + the dormant operand wiring).
- Mirror v241's `ParamExpansion` mechanism for the per-frame `body_started` state and the push/pop lifecycle; reuse v243's `parse_subshell_sequence` for the body verbatim.
- The empty-`$()` `Sequence` must equal the oracle's `empty_sequence` (NOT the subshell `EmptySubshell` error) — the differential pins it.
- Line numbers are approximate — locate by symbol name.
