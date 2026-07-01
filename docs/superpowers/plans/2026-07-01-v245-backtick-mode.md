# v245 — Backtick `` `…` `` Command Substitution Lexer Mode Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Invert backtick command substitution (`` `…` ``), including arbitrary-depth escaping-based nesting, into the parser-driven front-end — a lexer-owned `Mode::Backtick { depth }` whose depth *renames the emitted token* (`BeginBacktick`/`EndBacktick`/body) per character, with the parser assembling the nested `WordPart::CommandSub` — dormant and differential-tested against the production lexer.

**Architecture:** The LEXER owns a nesting `depth` (in `Mode::Backtick { depth }`, mutated by the lexer, never read from the parser). `scan_step_backtick` reads one char at a time; on a backtick it decodes the escaping level from the contiguous preceding backslash run (a small LOCAL `CharCursor` peek, bounded by depth — NOT a scan for a matching `` ` ``) and, comparing to `depth`, emits `BeginBacktick` (opens a child, depth++) or `EndBacktick` (closes, depth--); `\$`/`\\` are unescaped depth-aware; other body content tokenizes as ordinary Command tokens (so a `$()` in the body fat-builds). `parse_backtick_sub` enters/exits the mode and matches Begin/End to build the AST. Built incrementally by depth (0 → 1 → 2), each pinned by the differential vs the recursive production oracle.

**Tech Stack:** Rust, `crates/huck-syntax/src/{lexer.rs, parser.rs}`.

## Global Constraints

- **Byte-identical / dormant:** the PRODUCTION backtick path (`scan_backtick_body`, `unescape_backtick`, `scan_backtick_substitution`, `parse_substitution_body`, and `scan_step_command`'s existing `` ` `` / `\` arms for NON-`Backtick`-mode) is UNCHANGED; nothing in production pushes `Mode::Backtick`. The new mode + atoms + `scan_step_backtick` + `parse_backtick_sub` + the operand dispatch are reached ONLY by tests and the dormant parser path. `cargo test --workspace` green, 0 warnings; release `*_diff_check.sh` harness byte-identical.
- **The LEXER owns `depth`** — it lives in `Mode::Backtick { depth }` and is mutated by the lexer (increment on `BeginBacktick`, decrement on `EndBacktick`). `scan_step_backtick` reads ONLY lexer state (its own depth); it NEVER reads parser state or calls the parser (no lexer→parser dependency). The parser→lexer mode signal (enter/exit `Mode::Backtick`) is the allowed direction.
- **No unbounded scan-ahead** — one atom per step; nesting resolved by the owned depth state renaming the token + the parser's Begin/End matching. A SMALL, LOCAL, bounded `CharCursor` peek over the contiguous backslash run before a backtick is explicitly allowed.
- **`command.rs` untouched.** Reuse `WordPart::CommandSub { sequence, quoted }` (no AST change), `ParseError::UnsupportedExpansion`, and v244's differential harness (`find_command_sub`, `zero_lines_in_sequence`). Changes live in `lexer.rs` (mode, atoms, `scan_step_backtick`, dispatch arm, `CharCursor` peek, the operand-scanner backtick branch) and `parser.rs` (`parse_backtick_sub`, operand dispatch, corpus).
- **The production lexer is the ORACLE** — on any differential mismatch, fix the NEW path to match; never weaken the comparison. Do NOT hand-derive the escaping decode as a closed formula and skip the differential; derive it per depth level, validated by the corpus.
- Commit trailer on every commit: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

## Deferred (out of v245)

The non-incremental char-scanner backtick sites (`scan_regex_operand`, `scan_extglob_group`, expanding-heredoc body, `arith_string_to_word`, `parse_braced_operand_opts` — lexer.rs 2490/2550/2906/2945/2987/3921/3960) stay on the production fat-lexer path. Body-word atomization (a nested `$(inner)`/`${x}` in a body word stays fat-built). `$(( ))` arith stays deferred.

## Reference (read these)

Production ORACLE (`lexer.rs`): `scan_backtick_body` (~3666, raw collect to first unescaped `` ` ``), `unescape_backtick` (~3699, `` \` ``→`` ` ``/`\\`→`\`/`\$`→`$`), `scan_backtick_substitution` (~4019), `parse_substitution_body` (~4000, tokenize + `command::parse` + line-zeroing + `empty_sequence`). The `scan_step_command` `` ` `` arm (~1705) and `\` arm (~1654). v244 template: `Mode::CommandSub { body_started }` (~530), `scan_step_command_sub` (~1447), `TokenKind::CmdSubOpen` (~404), `scan_step` dispatch (~708); `parser.rs` `parse_command_sub` (~500), `old_cs`/`new_cs`/`diff_cs`/`diff_cs_deferred`/`find_command_sub` (~2040), `zero_lines_in_sequence` (~441), v243 `parse_subshell_sequence`. `CharCursor` (peek/next/offset — check its current lookahead depth).

---

### Task 1: Scaffolding — `CharCursor` bounded peek + `Mode::Backtick` + atoms + `parse_backtick_sub` skeleton + differential harness

**Files:** Modify `crates/huck-syntax/src/lexer.rs`, `crates/huck-syntax/src/parser.rs`.

**Interfaces produced (Tasks 2–6 depend on these):**
- `Mode::Backtick { depth: u32 }` (new `Mode` variant).
- `TokenKind::BeginBacktick`, `TokenKind::EndBacktick`.
- `CharCursor` bounded multi-char peek — confirm the existing peek suffices, else add `fn peek_nth(&self, n: usize) -> Option<char>` (or equivalent) that peeks up to a small bounded N without consuming.
- `pub(crate) fn parser::parse_backtick_sub(iter: &mut Lexer, quoted: bool) -> Result<WordPart, ParseError>` (skeleton: pushes `Mode::Backtick { depth: 0 }`, then `unimplemented!()` — Task 2 fills it).
- test helpers `old_bt`/`new_bt`/`diff_bt`/`diff_bt_deferred` (reuse `find_command_sub`).

**What to build:**
1. `lexer.rs`: add `Mode::Backtick { depth: u32 }`; add `TokenKind::BeginBacktick`/`EndBacktick`; add the `scan_step` dispatch arm `Mode::Backtick { depth } => self.scan_step_backtick(depth)` with `scan_step_backtick` a stub that `unreachable!("v245 Task 2")` for now (so the arm compiles). Fix any exhaustive `Mode`/`TokenKind` match.
2. `CharCursor`: verify it can peek at least the char after the immediate `peek()` (needed later for `\`-run decoding). If not, add a bounded `peek_nth`/`peek2`. Add a unit test proving the peek does NOT consume.
3. `parser.rs`: `parse_backtick_sub` skeleton — `iter.push_mode(Mode::Backtick { depth: 0 }); unimplemented!(...)`. Add the differential harness:
```rust
fn old_bt(s: &str, quoted: bool) -> WordPart {
    let src = if quoted { format!("\"{s}\"") } else { s.to_string() };
    let toks = tokenize_with_opts(&src, LexerOptions::default()).expect("old lex");
    match &toks[0].kind {
        TokenKind::Word(w) => find_command_sub(&w.0).expect("no comsub part in production token"),
        _ => panic!("production token is not a Word for {src:?}"),
    }
}
fn new_bt(s: &str, quoted: bool) -> Result<WordPart, ParseError> {
    let mut lx = Lexer::new_live(s, &Default::default(), LexerOptions::default());
    parse_backtick_sub(&mut lx, quoted)
}
fn diff_bt(s: &str) {
    assert_eq!(new_bt(s, false).unwrap(), old_bt(s, false), "unquoted {s:?}");
    assert_eq!(new_bt(s, true).unwrap(),  old_bt(s, true),  "quoted   {s:?}");
}
fn diff_bt_deferred(s: &str) {
    assert!(matches!(new_bt(s, false), Err(ParseError::UnsupportedExpansion)),
            "expected deferred for {s:?}, got {:?}", new_bt(s, false));
}
```

- [ ] **Step 1: Write the failing test:**
```rust
#[test]
fn bt_scaffolding_exists() {
    let _ = Mode::Backtick { depth: 0 };
    let _ = TokenKind::BeginBacktick;
    let _ = TokenKind::EndBacktick;
    let _ = old_bt("`echo hi`", false);   // production oracle callable
}
```
- [ ] **Step 2: Run to verify it fails** — `cargo test -p huck-syntax --lib bt_scaffolding 2>&1 | tail` (unknown variants / fn).
- [ ] **Step 3: Implement** items 1–3 (skeletons; no body tokenization yet).
- [ ] **Step 4: Run to verify it passes** + production untouched — `cargo test -p huck-syntax --lib bt_scaffolding 2>&1 | grep "test result"` PASS; `cargo build --workspace 2>&1 | grep -c "^warning"` = 0; `cargo test -p huck-syntax 2>&1 | grep "test result" | tail`.
- [ ] **Step 5: Commit**
```bash
git add crates/huck-syntax/src/lexer.rs crates/huck-syntax/src/parser.rs
git commit -m "v245 T1: Mode::Backtick + Begin/EndBacktick atoms + CharCursor peek + parse_backtick_sub skeleton + harness

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Depth-0 core — non-nested, no escapes (the mechanism)

**Files:** Modify `crates/huck-syntax/src/lexer.rs`, `crates/huck-syntax/src/parser.rs`.

**What to build — the core Begin/body/End mechanism for a single (unnested) backtick:**
- `scan_step_backtick(depth)`: reads forward one char at a time under `Mode::Backtick`.
  - On ENTRY (the opening `` ` `` — depth 0): consume it, set the mode's `depth = 1` (lexer-owned mutation of `Mode::Backtick`), emit `BeginBacktick`.
  - Body content (no `\`, no inner `` ` `` for Task 2): tokenize as ordinary Command tokens. **Reuse `scan_step_command`'s word/expansion/operator logic** — the recommended approach is a mode-guarded branch so the SAME word scanner runs, with only the `` ` `` handling changed under `Mode::Backtick` (see below); if that proves too invasive, a `scan_step_backtick` that shares helpers with `scan_step_command`. REPORT which you chose.
  - On an unescaped `` ` `` at depth 1 (the terminator): finish/flush the current token, set `depth = 0`, emit `EndBacktick`, and (next step) the mode is exited by the parser. The `` ` `` under `Mode::Backtick` must be treated as a TERMINATOR, NOT the production nested-backtick opener — this is the guarded difference from `scan_step_command`.
- `parse_backtick_sub`: replace the skeleton — pull `BeginBacktick`; parse the body as a `Sequence` terminated by `EndBacktick` (mirror `parse_subshell_sequence` but stop on `TokenKind::EndBacktick` instead of `Op(RParen)`); empty body (`EndBacktick` immediately) → an empty `Sequence` (same value as `empty_sequence`, like v244); consume `EndBacktick`; `zero_lines_in_sequence`; `iter.pop_mode()`; return `WordPart::CommandSub { sequence, quoted }`.

- [ ] **Step 1: Write failing tests** (backtick source; note Rust escaping — `` `echo hi` `` is the literal string `` `echo hi` ``):
```rust
#[test]
fn bt_depth0() {
    diff_bt("`echo hi`");
    diff_bt("`echo hi there`");
    diff_bt("`a | b`");
    diff_bt("`a && b || c`");
    diff_bt("`a; b`");
    diff_bt("`if x; then y; fi`");
    diff_bt("``");                 // empty -> empty Sequence
}
```
- [ ] **Step 2: Run to verify they fail** — `cargo test -p huck-syntax --lib bt_depth0 2>&1 | tail`.
- [ ] **Step 3: Implement** `scan_step_backtick` (depth-0) + the mode-guarded `` ` ``-terminator handling + `parse_backtick_sub`. Fix to `old_bt` on any mismatch. Confirm production byte-identical (the guard triggers ONLY under `Mode::Backtick`).
- [ ] **Step 4: Run to verify they pass** — PASS; `cargo test -p huck-syntax 2>&1 | grep "test result" | tail` (no regression); 0 warnings.
- [ ] **Step 5: Commit**
```bash
git add crates/huck-syntax/src/lexer.rs crates/huck-syntax/src/parser.rs
git commit -m "v245 T2: depth-0 backtick core (Begin/body/End, terminator, empty)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Body content — `\$`/`\\` unescaping, `$()`/`${}` in body, quoted

**Files:** Modify `crates/huck-syntax/src/lexer.rs`, `crates/huck-syntax/src/parser.rs`.

**What to build (still depth 1, no nesting):** the `\`-arm under `Mode::Backtick` — depth-aware unescape matching `unescape_backtick`: `\$` → `$` (an expandable `$`, so `\$x` tokenizes as the variable `$x`), `\\` → `\`, other `\c` → preserve the two chars. A body `$(…)`/`${…}` fat-builds via the reused `scan_step_command` `$`-expansion logic (pass through). `diff_bt` already checks quoted throughout.

- [ ] **Step 1: Write failing tests:**
```rust
#[test]
fn bt_body_content() {
    diff_bt("`echo \\$x`");        // \$ -> variable $x
    diff_bt("`echo \\\\`");         // \\ -> literal backslash
    diff_bt("`echo \\n`");          // \n -> preserved (backslash + n)
    diff_bt("`echo $(date)`");      // $() in body -> fat-built, passes through
    diff_bt("`echo ${x}`");         // ${} in body -> fat-built
    diff_bt("`echo $HOME`");        // bare $ expands
    diff_bt("`echo \"quoted\"`");  // dquotes in body
}
```
(Note the Rust `\\` = one backslash in the shell source; `` `echo \$x` `` in the shell is `"`echo \\$x`"` in Rust.)
- [ ] **Step 2: Run to verify they fail/pass** — `cargo test -p huck-syntax --lib bt_body_content 2>&1 | tail`.
- [ ] **Step 3: Implement** the `\`-arm depth-aware unescape under `Mode::Backtick`. Fix to `old_bt` on any mismatch (esp. `\$`→var vs literal, and the `$()` pass-through).
- [ ] **Step 4: Run to verify they pass** — PASS; no regression; 0 warnings.
- [ ] **Step 5: Commit**
```bash
git add crates/huck-syntax/src/lexer.rs crates/huck-syntax/src/parser.rs
git commit -m "v245 T3: backtick body content (\\\$/\\\\ unescape, \$()/\${} in body, quoted)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Depth-1 nesting — `` \` `` opens/closes a child backtick

**Files:** Modify `crates/huck-syntax/src/lexer.rs`, `crates/huck-syntax/src/parser.rs`.

**What to build — the FIRST nesting level (the escaping decode begins):** under `Mode::Backtick { depth }`, a backtick preceded by the level-appropriate backslash run is a nested delimiter. At depth 1: a `` \` `` (escaping level 1) → `BeginBacktick` (open child, depth → 2); the matching `` \` `` at depth 2 → `EndBacktick` (close, depth → 1); a bare `` ` `` at depth 1 still closes the outer (depth → 0). Decode the escaping level from the contiguous preceding backslash run (bounded `CharCursor` peek). `parse_backtick_sub` recurses on a nested `BeginBacktick` (a body word contains a nested `WordPart::CommandSub`). **Derive the level-1 decode by matching `old_bt` (the recursive oracle) — do not assume a formula; the corpus is the authority.**

- [ ] **Step 1: Write failing tests:**
```rust
#[test]
fn bt_depth1_nesting() {
    diff_bt("`echo \\`date\\``");            // `echo `date`` (nested once)
    diff_bt("`a \\`b\\` c`");                // outer body: a `b` c
    diff_bt("`\\`inner\\``");                // nested at the start
    diff_bt("`echo \\`echo hi\\``");
    diff_bt("`x \\`y | z\\` w`");            // pipeline in the nested body
}
```
(Rust `\\`` = the shell's `` \` ``.)
- [ ] **Step 2: Run to verify they fail** — the level-1 delimiter isn't recognized yet.
- [ ] **Step 3: Implement** the level-1 escaping decode + the depth increment/decrement + the `parse_backtick_sub` recursion. Fix to `old_bt` until the nested `Sequence` (and the outer `Sequence` containing it) match the oracle byte-for-byte.
- [ ] **Step 4: Run to verify they pass** — PASS; no regression; 0 warnings.
- [ ] **Step 5: Commit**
```bash
git add crates/huck-syntax/src/lexer.rs crates/huck-syntax/src/parser.rs
git commit -m "v245 T4: depth-1 backtick nesting (\\\` open/close child; parser recursion)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Depth-2 nesting — `` \\\` `` (proves the escaping-compounding decode)

**Files:** Modify `crates/huck-syntax/src/lexer.rs`, `crates/huck-syntax/src/parser.rs`.

**What to build — prove the decode GENERALIZES:** at depth 2, the child-open delimiter is `` \\\` `` (level 2), the close is `` \` `` (level 1); body escapes compound one more level too. Extend the escaping decode so the level is computed from the backslash-run length for ARBITRARY depth (the mechanism is uniform), pinned by the depth-2 corpus against the oracle. If the decode from Task 4 was already written generally (level = f(backslash run, depth)), this task mostly ADDS the depth-2 corpus and fixes any generalization gap.

- [ ] **Step 1: Write failing/validating tests:**
```rust
#[test]
fn bt_depth2_nesting() {
    diff_bt("`a \\`b \\\\\\`c\\\\\\` d\\` e`");   // depth-2: \\\` around c
    diff_bt("`\\`\\\\\\`x\\\\\\`\\``");             // depth-2 at the start
    diff_bt("`echo \\`echo \\\\\\`echo hi\\\\\\`\\``");
}
```
(Rust `\\\\\\`` = the shell's `` \\\` `` — three backslashes + backtick, the level-2 delimiter. Double-check each literal by running `old_bt` on it first if unsure.)
- [ ] **Step 2: Run to verify** — `cargo test -p huck-syntax --lib bt_depth2 2>&1 | tail`.
- [ ] **Step 3: Implement/generalize** the escaping-level decode for arbitrary depth. Fix to `old_bt`; verify the depth-1 tests STILL pass (the generalization must not break level 1).
- [ ] **Step 4: Run to verify** — all `bt_*` pass; no regression; 0 warnings.
- [ ] **Step 5: Commit**
```bash
git add crates/huck-syntax/src/lexer.rs crates/huck-syntax/src/parser.rs
git commit -m "v245 T5: depth-2 backtick nesting (\\\\\\` — escaping-compounding decode generalizes)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: Operand wiring + deferred/error corpus + full proof

**Files:** Modify `crates/huck-syntax/src/lexer.rs`, `crates/huck-syntax/src/parser.rs`.

**What to build:**
1. **Operand wiring** (parallel to v244 T4): where v241's `${…}` operand scanner emits `DeferredExpansion` for a backtick, hand off to `parse_backtick_sub` (reuse v244's zero-width-signal pattern — the operand scanner signals a backtick without consuming it; `parse_word` calls `parse_backtick_sub`). Keep `$((` deferred. Confirm NO v241/v244 regression.
2. **Error parity:** unterminated `` `echo `` → the new path Errs like the oracle.

- [ ] **Step 1: Write the tests:**
```rust
#[test]
fn bt_in_param_operand() {
    diff_ok("${x:-`echo d`}");        // uses v241's diff_ok (whole ${…} vs oracle)
    diff_ok("${x:+`cmd`}");
    diff_ok("${x:-a`b`c}");
}
#[test]
fn bt_error_parity() {
    let new = new_bt("`echo", false);
    assert!(new.is_err(), "unterminated backtick must Err, got {new:?}");
}
```
- [ ] **Step 2: Run to verify they fail** — operand backticks were deferred by v244.
- [ ] **Step 3: Implement** the operand hand-off + confirm error parity. Fix to `old_part`/`old_bt` on mismatch; run the FULL `-p huck-syntax --lib` suite to confirm no v241/v244 regression.
- [ ] **Step 4: Full proof:**
  - `cargo test -p huck-syntax --lib bt_ 2>&1 | grep "test result"` — all PASS.
  - `cargo test --workspace 2>&1 | grep -E "test result:" | awk '{p+=$4;f+=$6} END{print "passed="p" failed="f}'` — 0 failed.
  - `cargo build --workspace 2>&1 | grep -c "^warning"` — 0.
  - Release harness: `cargo build --release 2>&1 | tail -1`, then for `tests/scripts/*_diff_check.sh` run each with `HUCK_BIN=$(pwd)/target/release/huck` + 90s timeout; report `N scripts, F failures`. Expected 0 (production unchanged).
- [ ] **Step 5: Commit**
```bash
git add crates/huck-syntax/src/lexer.rs crates/huck-syntax/src/parser.rs
git commit -m "v245 T6: backtick in \${…} operands + error parity; full workspace/harness proof

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Notes for the implementer

- **The production lexer is the ORACLE.** On any `diff_bt`/`diff_ok` mismatch, fix the NEW path to match `old_bt`/`old_part` — never weaken the assertion. The escaping-level decode (Tasks 4–5) is the hard core: derive it per depth level by matching the oracle; do NOT hand-derive a closed formula and skip the differential. When unsure whether a Rust string literal is the shell source you intend, run `old_bt(s, false)` on it first and inspect.
- **The lexer owns `depth`** (in `Mode::Backtick`), mutates it itself, and reads only its own state — never parser state. The parser only enters/exits the mode and matches Begin/End.
- **No unbounded scan-ahead** — one atom per step; a small bounded `CharCursor` peek over the contiguous backslash run is the only lookahead, and it must not become a scan for a matching `` ` ``.
- **Byte-identical production** — the `Mode::Backtick` branches in the word scanner must trigger ONLY under `Mode::Backtick` (never pushed in production); confirm `scan_step_command`'s behavior is unchanged for `Mode::Command`.
- Do NOT touch `command.rs`, the production backtick scanners, or any engine crate. Reuse `WordPart::CommandSub` + the v244 harness. Line numbers are approximate — locate by symbol name.
