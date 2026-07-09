# Backtick capture-unescape-relex — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Replace huck's streaming backtick lexer mode (`Mode::Backtick` + `2^D−1`
depth formula + inline unescape + quote-aware delegation) with bash's three-phase
model — quote-blind raw capture → one-level unescape → recursive re-parse — fixing
the backslash-run collapse (L-70) and the quote-blind close divergence, and
flipping the `iquote` bash-suite category.

**Architecture:** `parse_backtick_sub` drives three phases. Phase 1: a new dumb
lexer mode `Mode::BacktickRaw` streams the body verbatim as `BacktickRawText`
atoms + `EndBacktick`, the parser concatenates them. Phase 2: a pure function
`unescape_backtick_body` applies `\\`→`\`, `\$`→`$`, `` \` ``→`` ` `` (all else
verbatim). Phase 3: the parser re-parses the cooked string with a fresh `Lexer`
via `parse_sequence`; nesting and `$()` fall out of recursion.

**Tech Stack:** Rust; `huck-syntax` crate (lexer.rs ~6600 lines, parser.rs ~3500
lines); bash-diff harnesses under `tests/scripts/`.

**Design doc:** `docs/superpowers/specs/2026-07-09-backtick-relex-design.md` — read
it first; §2 (phases), §3 (interaction), §8 (verified reference facts) are the
contract.

## Global Constraints

- **Binding rule:** the lexer emits small atoms and NEVER forward-scans for a
  matching delimiter; the parser owns delimiter-matching/recursion. Phase 1 emits
  small `BacktickRawText` chunks and recognizes the close LOCALLY (a bare backtick
  under 1-char `\` lookahead); the parser owns the capture loop and the re-parse.
- **Commit trailer:** every commit ends with
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- **Tests per-crate only** (this box OOM-kills `--workspace`):
  `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` (and
  `-p huck-engine` likewise). Build the binary with `cargo build -p huck`.
- **Never copy GPL bash test bytes** into committed files; harness fragments are
  hand-authored.
- Phase-2 unescape rule is EXACTLY three de-escape pairs (`\\`,`\$`,`` \` ``);
  `\<newline>`, `\"`, `\'`, `\n`, `\ ` etc. are kept verbatim and handled by the
  phase-3 re-lex (verified against bash 5.2 — spec §8).
- Bash-diff harnesses run FILE MODE (`bash "$f"` vs `huck "$f"`) so the harness's
  own shell never double-escapes the backslashes under test.

---

## File Structure

- `crates/huck-syntax/src/lexer.rs`
  - ADD `TokenKind::BacktickRawText(String)`; ADD `Mode::BacktickRaw`; ADD
    `scan_step_backtick_raw`; ADD `unescape_backtick_body` (pure fn) + accessors
    `aliases()`/`opts()`.
  - DELETE (Task 4) `scan_step_backtick`, `emit_backtick_delim`, `Mode::Backtick`
    (the depth-carrying variant), and its `look_past_backslash_run` helper if now
    unused.
- `crates/huck-syntax/src/parser.rs`
  - REWRITE `parse_backtick_sub` (capture → unescape → re-parse); DELETE
    `parse_backtick_body_sequence`.
- `tests/scripts/backtick_escape_diff_check.sh` — promote the 3 excluded
  divergences to live cases; add the parity matrix.
- `docs/bash-divergences.md` — delete L-70 (Task 6).

---

### Task 1: Phase-2 unescape as an isolated pure function

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` (add a free fn + `#[cfg(test)]` tests)

**Interfaces:**
- Produces: `pub(crate) fn unescape_backtick_body(raw: &str) -> String`

- [ ] **Step 1: Write the failing tests.** Add to lexer.rs's test module:

```rust
#[test]
fn unescape_backtick_body_rules() {
    // The ONLY three de-escape pairs remove the backslash:
    assert_eq!(unescape_backtick_body(r"\\"), r"\");        // \\ -> \
    assert_eq!(unescape_backtick_body(r"\$x"), "$x");       // \$ -> $
    assert_eq!(unescape_backtick_body("\\`"), "`");         // \` -> `
    // Everything else keeps the backslash verbatim:
    assert_eq!(unescape_backtick_body(r#"\""#), r#"\""#);   // \" kept
    assert_eq!(unescape_backtick_body(r"\'"), r"\'");       // \' kept
    assert_eq!(unescape_backtick_body(r"\n"), r"\n");       // \n kept (literal)
    assert_eq!(unescape_backtick_body("a\\\nb"), "a\\\nb"); // \<newline> kept
    // Runs collapse pairwise, left to right (the L-70 case):
    assert_eq!(unescape_backtick_body(r"\\\X"), r"\\X");    // \\\X -> \\X  (was mis-ordered)
    assert_eq!(unescape_backtick_body(r"\\\\"), r"\\");     // \\\\ -> \\
    assert_eq!(unescape_backtick_body(r"\\\\\x"), r"\\\x"); // 5 bslashes -> 3 + x
    // Trailing lone backslash kept:
    assert_eq!(unescape_backtick_body("ab\\"), "ab\\");
    // No backslashes: identity.
    assert_eq!(unescape_backtick_body("echo hi"), "echo hi");
}
```

- [ ] **Step 2: Run to confirm it fails** (`unescape_backtick_body` undefined):
  `cargo test -p huck-syntax --jobs 1 --lib unescape_backtick_body 2>&1 | tail`

- [ ] **Step 3: Implement.** A single left-to-right pass:

```rust
/// bash's one-level backtick unescape (spec §2 phase 2). Removes the backslash
/// for exactly `\\`, `\$`, and `` \` ``; every other `\c` (and a trailing lone
/// `\`) is copied verbatim for the phase-3 re-lex to handle.
pub(crate) fn unescape_backtick_body(raw: &str) -> String {
    let b = raw.as_bytes();
    let mut out = String::with_capacity(raw.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 1 < b.len() && matches!(b[i + 1], b'\\' | b'$' | b'`') {
            out.push(b[i + 1] as char); // drop the backslash, keep the escaped byte
            i += 2;
        } else {
            // Copy the next whole UTF-8 char verbatim (may be the lone `\`).
            let ch = raw[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}
```

- [ ] **Step 4: Run to confirm pass.** `cargo test -p huck-syntax --jobs 1 --lib unescape_backtick_body`

- [ ] **Step 5: Commit.** `git add -A && git commit` — "backtick: phase-2 unescape_backtick_body pure fn + tests".

---

### Task 2: `Mode::BacktickRaw` raw-capture lexer mode

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` (TokenKind, Mode, dispatch, new scan fn, tests)

**Interfaces:**
- Produces: `TokenKind::BacktickRawText(String)`; `Mode::BacktickRaw`;
  `fn scan_step_backtick_raw(&mut self) -> Result<Step, LexError>`.
- Consumes: entered by pushing `Mode::BacktickRaw` with the cursor parked on the
  opening `` ` `` (same entry point as `Mode::Backtick` today — the command-mode
  `BeginBacktick` signal at lexer.rs:~1854, unchanged).

**Contract for `scan_step_backtick_raw`** (spec §2 phase 1):
1. FIRST call (cursor on the opening `` ` ``): consume it, emit `TokenKind::BeginBacktick`
   (keep the existing handshake so `parse_backtick_sub`'s opening pull is unchanged).
   Use a per-mode "started" flag OR detect via `self.cursor.peek()==Some('`')` on entry
   — mirror how `scan_step_backtick(depth==0)` distinguished entry.
2. Then, per step on the body:
   - `None` (EOF before close) → `self.finish()` (unterminated; parser maps to error).
   - bare `` ` `` → consume it, emit `TokenKind::EndBacktick`.
   - `\` → consume the `\` AND the next char (if any); emit
     `TokenKind::BacktickRawText` containing both bytes verbatim. (A `` \` ``
     therefore never closes; the backslashes survive for phase 2.)
   - otherwise → consume the maximal run of chars that are neither `\` nor `` ` ``;
     emit it as one `TokenKind::BacktickRawText`.
   Quote-blind and `$()`-blind: `'`, `"`, `(`, `#` are ordinary run bytes.

- [ ] **Step 1: Write the failing test.** Add a lexer unit test that manually
  drives the mode. Model it on the existing `scan_backtick_body_*` tests (lexer.rs
  ~6620). Assert the atom sequence for a representative body:

```rust
#[test]
fn backtick_raw_streams_verbatim_atoms() {
    // Body:  `a\`b\\c`   (escaped backtick + escaped backslash are raw content)
    // Expect: BeginBacktick, "a", "\`", "b", "\\", "c", EndBacktick
    let mut lx = Lexer::new_live_atoms("`a\\`b\\\\c`", &Default::default(), LexerOptions::default());
    lx.push_mode(Mode::BacktickRaw);
    let mut kinds = Vec::new();
    loop {
        match lx.next_kind().unwrap() {
            Some(TokenKind::EndBacktick) => { kinds.push("END".to_string()); break; }
            Some(TokenKind::BeginBacktick) => kinds.push("BEGIN".to_string()),
            Some(TokenKind::BacktickRawText(s)) => kinds.push(s),
            Some(other) => panic!("unexpected atom: {other:?}"),
            None => panic!("EOF before EndBacktick"),
        }
    }
    let joined: String = kinds.iter().filter(|k| !matches!(k.as_str(), "BEGIN"|"END")).cloned().collect();
    assert_eq!(joined, "a\\`b\\\\c"); // raw body reassembles verbatim
    assert_eq!(kinds.first().unwrap(), "BEGIN");
    assert_eq!(kinds.last().unwrap(), "END");
}
```
(Adjust the exact `next_kind`/push_mode API names to the real ones seen in the
neighboring tests; the ASSERTION — verbatim reassembly + Begin/End framing — is
the contract.)

- [ ] **Step 2: Run to confirm it fails** (mode/variant undefined):
  `cargo test -p huck-syntax --jobs 1 --lib backtick_raw_streams_verbatim`

- [ ] **Step 3: Implement.** Add `TokenKind::BacktickRawText(String)` to the enum
  (lexer.rs:561). Add `Mode::BacktickRaw` to the Mode enum (lexer.rs:844). Add the
  dispatch arm next to line 1371: `Mode::BacktickRaw => self.scan_step_backtick_raw()`.
  Implement `scan_step_backtick_raw` per the contract above. Leave the old
  `Mode::Backtick`/`scan_step_backtick` in place (still used) — both coexist and
  compile.

- [ ] **Step 4: Run to confirm pass.** `cargo test -p huck-syntax --jobs 1 --lib backtick_raw_streams_verbatim`

- [ ] **Step 5: Commit.** "backtick: add Mode::BacktickRaw raw-capture mode + BacktickRawText atom".

---

### Task 3: Rewrite `parse_backtick_sub` (capture → unescape → re-parse)

**Files:**
- Modify: `crates/huck-syntax/src/parser.rs` (`parse_backtick_sub`; delete
  `parse_backtick_body_sequence`)
- Modify: `crates/huck-syntax/src/lexer.rs` (add `aliases()`/`opts()` accessors)

**Interfaces:**
- Consumes: `Mode::BacktickRaw`, `TokenKind::BacktickRawText`, `unescape_backtick_body`,
  `parse_sequence`, `Lexer::new_live_atoms`.
- Produces (unchanged public shape): `WordPart::CommandSub { sequence, quoted }`.

**Accessors (lexer.rs):**
```rust
pub(crate) fn aliases(&self) -> &std::collections::HashMap<String, String> { &self.aliases }
pub(crate) fn opts(&self) -> LexerOptions { self.opts }
```

**New `parse_backtick_sub` body** (replaces the push/pull-sequence version at
parser.rs:1545; no more nested-recursion-without-push — nesting is via phase 3):
```rust
pub(crate) fn parse_backtick_sub(iter: &mut Lexer, quoted: bool) -> Result<WordPart, ParseError> {
    // Phase 1 — capture the raw body under the dumb BacktickRaw mode.
    iter.push_mode(Mode::BacktickRaw);
    let raw = (|| -> Result<String, ParseError> {
        match iter.next_kind()? { Some(TokenKind::BeginBacktick) => {}, _ => return Err(ParseError::UnsupportedExpansion) }
        let mut raw = String::new();
        loop {
            match iter.next_kind()? {
                Some(TokenKind::BacktickRawText(s)) => raw.push_str(&s),
                Some(TokenKind::EndBacktick) => return Ok(raw),
                None => return Err(ParseError::UnexpectedEof), // unterminated `...`
                _ => return Err(ParseError::UnsupportedExpansion),
            }
        }
    })();
    iter.pop_mode();
    let raw = raw?;

    // Phase 2 — one-level unescape.
    let cooked = crate::lexer::unescape_backtick_body(&raw);

    // Phase 3 — re-parse the cooked body as a command Sequence with a FRESH lexer.
    // in_dquote is cleared: the body is its own context even inside "`...`".
    let mut sub_opts = iter.opts();
    sub_opts.in_dquote = false;
    let mut sub = Lexer::new_live_atoms(&cooked, iter.aliases(), sub_opts);
    let sequence = match parse_sequence(&mut sub)? {
        Some(mut seq) => { zero_lines_in_sequence(&mut seq); seq }
        None => empty_sequence(), // `` `` `` — same empty Sequence as before (see 1570-1574)
    };
    Ok(WordPart::CommandSub { sequence, quoted })
}
```
- Extract the empty-Sequence literal (parser.rs:1570-1574) into a small
  `fn empty_sequence() -> Sequence` (or inline it).
- Confirm `ParseError::UnexpectedEof` is the right unterminated-substitution
  variant (grep the existing `parse_backtick_body_sequence` EOF arm for what it
  used — reuse the SAME variant so error behavior is unchanged).
- DELETE `parse_backtick_body_sequence` (parser.rs:1465).

- [ ] **Step 1: Write failing tests.** Add end-to-end parse assertions that
  currently fail (the L-70 cases). Prefer the diff harness for behavior, but add
  at least one Rust round-trip test if the parser test module supports lex→parse→
  generate; otherwise rely on Task 5's harness and make this step "extend the
  harness with the 3 promoted divergence cases and run it red". Concretely, run:
  `HUCK_BIN=$(pwd)/target/debug/huck bash tests/scripts/backtick_escape_diff_check.sh`
  with the 3 divergences promoted (Task 5 authors them) — expect FAIL before, PASS
  after. If sequencing this before Task 5, add a temporary 3-case check here.

- [ ] **Step 2: Confirm red** (build the binary first:
  `cargo build -p huck`, then run the harness / tests).

- [ ] **Step 3: Implement** the rewrite + accessors + deletion above.

- [ ] **Step 4: Confirm green.** `cargo build -p huck` then the harness; plus
  `cargo test -p huck-syntax --jobs 1 --lib` (existing backtick parser tests must
  stay green — nested/escaped/quoted/dq cases from `backtick_escape_diff_check.sh`
  and any `parse_backtick*` unit tests).

- [ ] **Step 5: Commit.** "backtick: parse_backtick_sub captures raw, unescapes, re-parses".

---

### Task 4: Delete the old streaming machinery

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs`

- [ ] **Step 1:** Delete `scan_step_backtick` (lexer.rs:2460), `emit_backtick_delim`
  (lexer.rs:2619), and the `look_past_backslash_run` helper (lexer.rs:~232) if now
  unreferenced. Remove the `Mode::Backtick { depth }` variant and its dispatch arm
  (line 1371); update the two `matches!(… Mode::Backtick …)` sites (lexer.rs:1193,
  3417) to `Mode::BacktickRaw`. Delete the `bt_malformed_divergence_deferred` test
  and any `scan_backtick_body`/`consume_backtick_verbatim` helpers made dead
  (lexer.rs:5606/5630 — verify no other caller first with grep).
- [ ] **Step 2:** `cargo build -p huck-syntax 2>&1 | grep -E 'error|warning'` —
  resolve any dead-code warnings by deleting the dead item (not `#[allow]`).
- [ ] **Step 3:** `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1`
  and `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1` — all green.
- [ ] **Step 4: Commit.** "backtick: delete the 2^D-1 depth-formula machinery".

---

### Task 5: Parity harness, promote divergences, full verification

**Files:**
- Modify: `tests/scripts/backtick_escape_diff_check.sh`

- [ ] **Step 1: Promote the 3 documented divergences** (currently comments at
  lines 50-63) to live `checkf` cases, and add the parity matrix + quote-blind +
  `$()` cases:

```bash
# --- L-70 fixes: previously-divergent cases now pass ---
checkf 'run3 before X'     'echo `echo \\\X`'      # -> \X
checkf 'run4 pure'         'echo `echo \\\\`'      # -> \
checkf 'esc-bt dbl-bslash' 'echo `echo \\\`lit\\\``' # -> `lit`
checkf 'run2 before dollar' 'X=1; echo `echo \\\$X`' # -> $X (literal)

# --- backslash-run parity: N backslashes before X (file mode, exact bytes) ---
for n in 0 1 2 3 4 5 6 7 8; do
  bs=$(printf '%*s' "$n" '' | tr ' ' '\\')
  checkf "run$n-X" "echo \`echo ${bs}X\`"
  checkf "run$n-close" "echo \`echo a${bs}\`"       # run before the closing backtick
done

# --- quote-blind close: bash closes at a backtick inside quotes (may ERROR) ---
checkf 'sq hides nothing'  "echo \`echo '\`' hi\`"  # bash errors; huck must too
checkf 'literal bt in sq'  "x=\`printf '%s' 'a\`b'\`; echo \"[\$x]\""

# --- $() inside a backtick body ---
checkf 'dollarparen inbt'  'echo `echo $(echo hi)`'
checkf 'dollarparen tail'  'echo `echo $(echo X)Y`'

# --- nesting depth 2-3 (must remain correct) ---
checkf 'nest depth2'       'echo `echo \`echo inner\``'
checkf 'nest depth3'       'echo `echo \`echo \\\`echo deep\\\`\``'
```
(Delete the now-obsolete `# DIVERGENCE (reported)` comment block.)

- [ ] **Step 2:** Build release + run the harness guarded:
  `cargo build -p huck && ( ulimit -v 1500000; HUCK_BIN=$(pwd)/target/debug/huck timeout 120 bash tests/scripts/backtick_escape_diff_check.sh )`
  — expect all PASS. Investigate any `checkf` that diverges; if it's a genuinely
  undefined POSIX corner where matching bash isn't feasible, EXCLUDE it with a
  documented `# DIVERGENCE` comment and log it for Task 6 (do NOT silently drop).

- [ ] **Step 3: Full suites + official runner.**
  `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1`;
  `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`;
  build release and run the official runner
  (`env -u TMPDIR BASH_SOURCE_DIR=/tmp/bash-5.2.21 bash tests/bash-test-suite/runner.sh`).
  **Confirm `iquote` flips FAIL→PASS and NO category regresses PASS→not-PASS.**
- [ ] **Step 4: Commit.** "backtick: parity harness + promote L-70 divergences; iquote flips".

---

### Task 6: Docs + divergences

**Files:**
- Modify: `docs/bash-divergences.md`

- [ ] **Step 1:** DELETE the L-70 entry (it's resolved). If Task 5 surfaced a
  residual undefined-corner divergence, add a new `[deferred]` entry for it
  instead. Adjust the Tier-4 count if the doc tracks one.
- [ ] **Step 2:** Commit. "docs: retire L-70 (backtick capture-unescape-relex shipped)".

---

## Self-review notes

- **Spec coverage:** phase 1 → Task 2; phase 2 → Task 1; phase 3 + interaction →
  Task 3; deletions → Task 4; success criteria (harness/iquote/parity) → Task 5;
  L-70 retirement → Task 6. All spec §2–§6 items mapped.
- **Ordering:** Task 3 depends on Tasks 1+2 (uses the fn + mode). Task 4 depends
  on Task 3 (old machinery goes dead only after the rewrite). Task 5 verifies the
  whole. The Task 3 red/green anchor leans on Task 5's harness cases — if run
  standalone, add the 3 promoted cases temporarily in Task 3 Step 1.
- **Risk watch (spec §7):** span/line zeroing (`zero_lines_in_sequence` preserved
  in phase 3); entry from `"…"`/operand-dquote (the `quoted` flag still threads;
  the BeginBacktick entry signal is unchanged); `in_dquote` cleared for the sub-lexer.
- **Non-goal reminder:** exact malformed-body error TEXT is out of scope — the
  harness `checkf` compares stdout+stderr+exit, so a quote-blind-close case whose
  bash stderr differs only in the program-name prologue may need its assertion
  narrowed to stdout+exit (note it in the harness comment) rather than forcing
  byte-identical stderr.
