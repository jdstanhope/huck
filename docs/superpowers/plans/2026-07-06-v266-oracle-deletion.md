# v266 — Sever the two atom→oracle bridges, then delete the oracle — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Port the last two atom→oracle bridges (subscript-assignment lvalue, alias body) onto the atom path, then delete the now-unreachable oracle (`command.rs` recursive-descent parser + the Word lexer + forward-scanners), leaving one parser and one lexing discipline.

**Architecture:** (1) Subscript — the lexer keeps *detecting* `name[…]=`/`+=` (bounded bracket scan) but stops assembling the subscript `Word` via the oracle; a new parser helper `parse_fragment_word` assembles it from atoms. (2) Alias — replace history-token-splicing with an input-source stack in the lexer so an alias body is lexed inline as atoms the parser consumes. (3) With both bridges gone the oracle is unreachable; delete it compiler-assisted (grep-verified, since `dead_code` under-reports mutually-recursive cycles) and tidy `command.rs` down to AST types.

**Tech Stack:** Rust (huck-syntax + huck-engine crates), single-threaded per-crate test runner, bash-diff harnesses.

> **DEVIATION FROM SPEC (subscript, Task 1):** The spec's Bridge 2 proposed "lexer carries raw text, parser assembles, no lexer→parser dependency." Code analysis showed the subscript `Word` has **no single parser choke point** — it is consumed by `command::try_split_assignment` (×2), `generate.rs`, and `expand.rs` directly, plus the final AST. The only single-point-correct assembly site is where the lexer builds it. This plan therefore assembles by calling `crate::parser::parse_fragment_word` from the two lexer subscript sites (a contained lexer→parser call), which is a 2-call-site change with zero type/AST/engine churn. The spec's approach would require touching every consumer or a whole-AST post-pass (higher risk). The AST type `AssignTarget::Indexed { subscript: Word }` and all engine code stay **unchanged**, exactly as the spec required.
>
> **FOLLOW-UP (deferred, user-agreed):** the introduced lexer→parser call (`lexer.rs` → `crate::parser::parse_fragment_word`) is an accepted architectural wart for this iteration. A subsequent iteration will remove the cycle — most likely by having the lexer carry the raw subscript text on the atom and assembling it at the (then-refactored) single parser choke point. Do NOT expand this iteration to do it.

## Global Constraints

- **Test runner (box is 1 core / 1.9 GB):** ONLY `cargo test -p <crate> --jobs 1 --lib -- --test-threads 1`. NEVER `--workspace` or multi-threaded (OOM-kills the session). Crates: `huck-syntax`, `huck-engine`.
- **Build the binary** with `cargo build -p huck` (root package). `huck-cli` is a lib and does NOT build the binary.
- **Guard every bash-diff harness / binary run** with `ulimit -v 1500000` + `timeout` in a subshell.
- **THE RULE:** the lexer emits small atoms and NEVER forward-scans for a matching delimiter across nesting; the parser owns delimiter-matching, recursion, and word assembly. (The retained subscript *detection* scans only a bracket-balanced `[…]` to confirm the assignment shape — bounded, non-recursive — flagged as a follow-up for full RULE-compliance.)
- **0 warnings** from `cargo build -p huck-syntax` and `-p huck-engine` at each task end. Trust `cargo`, not rust-analyzer (phantom `dead_code` recurs — verify with a real build).
- **Baseline to preserve:** huck-syntax 1059 pass, huck-engine 1739 pass, bash-diff sweep 1688 pass / 1 fail (`funcnest`, pre-existing intentional L-63). Counts shift as noted per task; **the bash-diff 1688/1 must not regress**.
- **Commit trailer, verbatim on every commit:** `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- **Do not change AST types or shell behavior** except as a task specifies. v266 is bridge-port + deletion + relocation — behavior-preserving, verified by the bash-diff sweep.

## File Structure

- `crates/huck-syntax/src/parser.rs` — NEW `parse_fragment_word` helper (Task 1); alias call sites unchanged.
- `crates/huck-syntax/src/lexer.rs` — subscript sites repointed (Task 1); `CharCursor` input-source stack + `maybe_expand_command_alias` rewrite (Task 2); oracle deletions (Task 4).
- `crates/huck-syntax/src/command.rs` — unchanged in Tasks 1-2; loses the recursive-descent parser, keeps AST types (Tasks 4-5).
- Tests live inline in each `*.rs` `#[cfg(test)]` module and in `tests/scripts/*_diff_check.sh`.

**Task order:** Task 1 (subscript) and Task 2 (alias) are independent; subscript is first because it is lower-risk and builds the `parse_fragment_word` primitive. Tasks 3→4→5 are a hard chain requiring both bridges landed. Task 6 is the final gate.

---

### Task 1: Subscript bridge — `parse_fragment_word` assembles the subscript `Word`

Replace the two lexer calls to the oracle `parse_subscript_body` with a parser-driven `parse_fragment_word`. The bounded `[…]` detection scan stays; only the `Word` assembly moves off the oracle.

**Files:**
- Modify: `crates/huck-syntax/src/parser.rs` (add `parse_fragment_word` near the other `pub fn` entry points, e.g. beside `parse_sequence`)
- Modify: `crates/huck-syntax/src/lexer.rs:4231` (in `try_scan_assign_prefix`) and `crates/huck-syntax/src/lexer.rs:3276` (in `scan_step_command_atoms_core`)
- Test: inline `#[cfg(test)] mod tests` in `parser.rs`

**Interfaces:**
- Produces: `pub fn parse_fragment_word(raw: &str, opts: crate::lexer::LexerOptions) -> Result<crate::lexer::Word, ParseError>` — atom-lexes `raw` and returns a single `Word`; on an empty or multi-word fragment, returns one unquoted `Literal` carrying `raw` verbatim (matches the old `parse_subscript_body` fallback so arithmetic subscripts like `1 + 2` see joined text).
- Consumes: `crate::lexer::Lexer::new_live_atoms`, `parse_sequence`, the AST `Command`/`SimpleCommand`/`Sequence` shapes.

- [ ] **Step 1: Write failing tests for `parse_fragment_word`**

In `crates/huck-syntax/src/parser.rs` test module add:

```rust
#[test]
fn fragment_word_single_expansion() {
    use crate::lexer::{WordPart, LexerOptions};
    let w = parse_fragment_word("$i", LexerOptions::default()).unwrap();
    // A single `$i` becomes a one-part Word with a variable expansion (not a raw literal).
    assert!(w.0.iter().any(|p| !matches!(p, WordPart::Literal { .. })), "got {:?}", w);
}

#[test]
fn fragment_word_command_sub() {
    use crate::lexer::{WordPart, LexerOptions};
    let w = parse_fragment_word("$(echo 2)", LexerOptions::default()).unwrap();
    assert!(w.0.iter().any(|p| matches!(p, WordPart::CommandSub { .. })), "got {:?}", w);
}

#[test]
fn fragment_word_multiword_collapses_to_raw_literal() {
    use crate::lexer::{WordPart, LexerOptions};
    let w = parse_fragment_word("1 + 2", LexerOptions::default()).unwrap();
    assert_eq!(w.0, vec![WordPart::Literal { text: "1 + 2".to_string(), quoted: false }]);
}

#[test]
fn fragment_word_plain_index() {
    use crate::lexer::{WordPart, LexerOptions};
    let w = parse_fragment_word("i+1", LexerOptions::default()).unwrap();
    assert_eq!(w.0, vec![WordPart::Literal { text: "i+1".to_string(), quoted: false }]);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `( ulimit -v 2500000; cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1 fragment_word 2>&1 | tail -5 )`
Expected: FAIL — `cannot find function parse_fragment_word`.

- [ ] **Step 3: Implement `parse_fragment_word`**

Add to `parser.rs` (module scope, `pub`):

```rust
/// Assemble a bounded fragment (a `[…]` subscript body) into a single `Word`
/// using the atom parser. Replaces the oracle `parse_subscript_body`. The
/// fragment is balanced by construction (the lexer captured a bracket-matched
/// interior), so `$i` / `${j}` / `$((n))` / `$(…)` / quotes all assemble
/// correctly here — the parser drives the zero-width opener signals a standalone
/// lexer could not. If the fragment is empty or lexes to more than one word
/// (e.g. `1 + 2`), collapse to a single unquoted `Literal` carrying the raw
/// text, matching `parse_subscript_body`'s historical fallback (arithmetic
/// evaluation tolerates the joined text; literal keys see it verbatim).
pub fn parse_fragment_word(
    raw: &str,
    opts: crate::lexer::LexerOptions,
) -> Result<crate::lexer::Word, ParseError> {
    use crate::command::{Command, SimpleCommand};
    let empty = std::collections::HashMap::new();
    let mut lx = crate::lexer::Lexer::new_live_atoms(raw, &empty, opts);
    let parsed = parse_sequence(&mut lx)?;
    // Single-word fragment: exactly one simple Exec command whose program is the
    // whole word, with no args / redirects / inline-assignments / list tail.
    if let Some(seq) = &parsed
        && seq.rest.is_empty()
        && !seq.background
        && let Command::Pipeline(p) = &seq.first
        && !p.negate
        && p.commands.len() == 1
        && let Command::Simple(SimpleCommand::Exec(e)) = &p.commands[0]
        && e.args.is_empty()
        && e.redirects.is_empty()
        && e.inline_assignments.is_empty()
    {
        return Ok(e.program.clone());
    }
    Ok(crate::lexer::Word(vec![crate::lexer::WordPart::Literal {
        text: raw.to_string(),
        quoted: false,
    }]))
}
```

(If the `let`-chain `&&` form does not compile on the pinned toolchain, rewrite as nested `if let` — behavior identical. Verify against a real build.)

- [ ] **Step 4: Run tests to verify they pass**

Run: `( ulimit -v 2500000; cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1 fragment_word 2>&1 | tail -5 )`
Expected: PASS (4 tests).

- [ ] **Step 5: Repoint the two lexer subscript sites**

In `crates/huck-syntax/src/lexer.rs`, the assignment-lvalue site (currently ~4231, inside `try_scan_assign_prefix`):

```rust
let subscript = parse_subscript_body(&raw, self.opts)?;
```

becomes

```rust
let subscript = crate::parser::parse_fragment_word(&raw, self.opts)?;
```

And the command-word in-word site (currently ~3276, inside `scan_step_command_atoms_core`):

```rust
let subscript = parse_subscript_body(&raw_subscript, self.opts)?;
```

becomes

```rust
let subscript = crate::parser::parse_fragment_word(&raw_subscript, self.opts)?;
```

Note both call sites are inside atom-path scanners. `parse_fragment_word` returns `Result<Word, ParseError>`; these sites currently return `Result<_, LexError>`. Map the error: wrap as the lexer already maps parser errors, or (simplest) convert `ParseError` → `LexError` via the existing `From`/`?` path. If no `From<ParseError> for LexError` exists, add `.map_err(|e| LexError::…)` using the same variant the atom path uses elsewhere for embedded parse failures (grep `ParseError` handling in `lexer.rs`; if none, add a `LexError::Parse(Box<ParseError>)` variant mirroring `ParseError::Lex`). Keep the error path behavior-preserving.

- [ ] **Step 6: Verify the two sites still compile and the subscript oracle call is gone from the atom path**

Run: `( ulimit -v 2500000; cargo build -p huck-syntax 2>&1 | tail -8 )`
Expected: builds, 0 warnings. Then confirm the only remaining `parse_subscript_body` caller is the oracle-internal `scan_subscript`:

Run: `grep -n "parse_subscript_body(" crates/huck-syntax/src/lexer.rs | grep -v "fn parse_subscript_body"`
Expected: exactly one line — inside `scan_subscript` (~7230). The two atom sites (3276, 4231) no longer appear.

- [ ] **Step 7: Add subscript behavior tests (engine-level, exercise the full path)**

In `crates/huck-engine` there are existing subscript/array tests; add focused cases. Create/extend an inline test that runs source through the shell and checks output. Example fragments to assert byte-for-byte vs. expectation (use the crate's existing run-and-capture helper; grep for one, e.g. `run_script`/`eval_to_string`):

```
a=(); a[$(echo 2)]=hi;      echo "${a[2]}"     # -> hi
i=3;  a[$i]=x;              echo "${a[3]}"     # -> x
a[1+1]=y;                   echo "${a[2]}"     # -> y
declare -A m; m["a b"]=z;   echo "${m[a b]}"   # -> z
a[2]=p; a[2]+=q;            echo "${a[2]}"     # -> pq
```

Write them as one `#[test]` using the existing harness pattern in that file. If unsure of the helper, model on the nearest existing `a[` subscript test in `huck-engine`.

- [ ] **Step 8: Run the engine subscript tests + the array/subscript bash-diff harnesses**

Run: `( ulimit -v 2500000; cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 subscript 2>&1 | tail -5 )`
Expected: PASS.
Run the relevant harnesses:
```bash
cd /home/john/projects/huck; export HUCK_BIN=$(pwd)/target/debug/huck
( ulimit -v 2500000; cargo build -p huck 2>&1 | tail -1 )
for f in $(ls tests/scripts/*_diff_check.sh | grep -iE "array|subscript|assign|arith"); do
  ( ulimit -v 1500000; timeout 90 bash "$f" ) >/dev/null 2>&1 && echo "PASS $(basename $f)" || echo "FAIL $(basename $f)"
done
```
Expected: all PASS.

- [ ] **Step 9: Full huck-syntax + huck-engine suites green, 0 warnings**

Run:
```bash
( ulimit -v 2500000; cargo test -p huck-syntax --jobs 1 --lib 2>&1 | grep "test result:" )
( ulimit -v 2500000; cargo test -p huck-engine --jobs 1 --lib 2>&1 | grep "test result:" )
```
Expected: huck-syntax ok (1059 + 4 new = 1063), huck-engine ok (1739 + new).

- [ ] **Step 10: Commit**

```bash
git add crates/huck-syntax/src/parser.rs crates/huck-syntax/src/lexer.rs crates/huck-engine
git commit -m "v266 T1: subscript bridge — parse_fragment_word assembles the subscript Word

The two atom-path subscript sites (a[i]=v lvalue + in-word) stop calling the
oracle parse_subscript_body; the parser's new parse_fragment_word assembles the
Word from atoms (single-word, else collapse-to-raw-literal fallback preserved).
AST AssignTarget::Indexed{subscript: Word} + engine unchanged.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Alias bridge — input-source stack

Replace the `tokenize(&body)` + history-splice in `maybe_expand_command_alias` with an input-source stack: firing an alias pushes its body text as a nested char source that the lexer reads inline (emitting atoms + opener signals the parser consumes), popping back to the parent source at the body's end.

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` — `CharCursor` (~49-138): add a source stack; `Lexer` fields + `maybe_expand_command_alias` (~5026-5086).
- Test: inline `#[cfg(test)] mod tests` in `lexer.rs` (token-stream / behavior) + engine-level alias tests + `tests/scripts/alias_*_diff_check.sh`.

**Interfaces:**
- Produces: unchanged public API — `expand_command_alias`, `take_trailing_eligible`, `new_live`, `new_live_atoms`. The parser call sites (`parser.rs` ~2189, ~2417) are unchanged.
- Invariant: **every token produced while an alias body is the active source carries the span of the alias-name invocation** (the base-input position), exactly as today's splice sets `name_span` on each spliced token. Achieve this by freezing `CharCursor::offset()/line()/column()` at the injection point while an injected source is active (the injected body has no base-input offset space of its own).

- [ ] **Step 1: Add the input-source stack to `CharCursor`**

Give `CharCursor` an injection stack. Each frame owns its body text and its own read position, but reports the *frozen* base offset/line/column captured at push time. Sketch (adapt field-for-field to the real struct):

```rust
struct Injected {
    body: String,       // owned alias replacement text
    pos: usize,         // read position within `body`
    peeked: Option<char>,
    peeked_len: usize,
    // Frozen base coordinates to report while this frame is active:
    anchor_off: usize,
    anchor_line: u32,
    anchor_col: u32,
}

pub struct CharCursor<'a> {
    s: &'a str,
    pos: usize,
    line: u32,
    column: u32,
    peeked: Option<char>,
    peeked_len: usize,
    injected: Vec<Injected>,   // NEW: top of stack is the active source; empty = read base `s`
}
```

Update construction sites (`CharCursor { s, pos: 0, … }` at ~60 and any `#[derive(Clone)]` use — `injected: Vec::new()`). `Clone` derive still works (Vec/String are Clone).

- [ ] **Step 2: Route `next`/`peek`/`peek_nth`/`offset`/`line`/`column` through the stack**

- `next()`: if `injected` non-empty, read from `injected.last_mut()` (its `body[pos..]`); when that frame is exhausted, `injected.pop()` **and remove its alias name from `Lexer::active`** (see Step 5 — the pop hook), then retry (read from the new top, or base). Otherwise read base `s` as today.
- `peek()`/`peek_nth()`: same source selection; peeking never pops.
- `offset()`/`line()`/`column()`: if `injected` non-empty, return the **top frame's anchor** (`anchor_off`/`anchor_line`/`anchor_col`) — frozen — so token spans pin to the alias-name site. If empty, return base `pos`/`line`/`column`.
- `seek()`/`slice_from()`: operate on the base only; assert/`debug_assert!(self.injected.is_empty())` — mark/rewind and raw-slice reconstruction are not expected to straddle an injected alias body. (If a future case needs it, that is a separate change; document the assumption.)

Because `next()` needs to remove the popped frame's name from `Lexer::active`, and `CharCursor` does not hold `active`, do the pop in the `Lexer` layer instead: add `CharCursor::injection_depth() -> usize` and have the `Lexer`'s pull loop pop+clear when depth drops. Simpler alternative (recommended): store the alias name **in the `Injected` frame** and expose `CharCursor::take_exhausted_alias() -> Option<String>` that the `Lexer` drains after each `next`, removing it from `active`. Choose whichever keeps `active` correct; the test in Step 7 (recursion) is the gate.

- [ ] **Step 3: Add a push entry on `CharCursor` (or `Lexer`)**

```rust
impl<'a> CharCursor<'a> {
    /// Inject `body` as the active input source, to be read fully before
    /// resuming the current source. Tokens produced while it is active report
    /// the frozen (`anchor_*`) coordinates.
    fn push_injection(&mut self, body: String, alias_name: String) {
        let (anchor_off, anchor_line, anchor_col) = (self.offset(), self.line(), self.column());
        self.injected.push(Injected {
            body, pos: 0, peeked: None, peeked_len: 0,
            anchor_off, anchor_line, anchor_col,
            // stash alias_name for the active-set pop (Step 2)
        });
    }
}
```

- [ ] **Step 4: Write the alias behavior tests FIRST (they will fail until Step 5)**

Add to `lexer.rs` test module — drive the full atom pipeline via the existing `new_seq`-style helper if present in `lexer.rs` tests, else via `parser::parse_sequence`. Prefer engine-level assertions in `huck-engine` for behavior; here assert structural/lex outcomes. Minimum set (put the runnable ones in `huck-engine`):

```
alias ll='ls -l';           type-check ll expands to `ls -l`
alias now='echo $(echo 2)'; now                 # -> 2   (body has $() — the spin case)
alias g='grep --color';     echo x | g x        # body flows into a pipeline stage
alias e='echo ';            e hi                 # trailing space -> next word eligible; -> hi
alias ls='ls --color';      (recursion) ls expands ONCE, not infinitely
alias a='b x'; alias b='a y'; a                  # a->b->a chain terminates
```

Write the behavioral ones as `huck-engine` `#[test]`s using that crate's run-and-capture helper (model on an existing `alias` test — grep `alias` in `huck-engine` tests).

- [ ] **Step 5: Rewrite `maybe_expand_command_alias` to push a source instead of splicing**

Replace the body-tokenize + history-splice (~5071-5085) with a push. Keep the name-extraction and boundary check (~5029-5069) and the `active` guard unchanged in spirit; re-anchor `active` to the source frame:

```rust
if self.active.contains(&name) { return Ok(()); }
let Some(body) = self.aliases.get(&name).cloned() else { return Ok(()); };
// Remove the already-produced command-word token(s) for `name` from history so
// the pushed body re-lexes in its place. (Today's code did `history.remove(self.pos)`;
// preserve the equivalent removal of the name token at `self.pos`.)
self.history.remove(self.pos);
// Trailing-blank eligibility: bash re-checks the next word when the body ends in blank.
self.alias_trailing_eligible = body.chars().last().is_some_and(|c| c.is_whitespace());
// Mark active for the lifetime of the injected source; the cursor pop clears it.
self.active.insert(name.clone());
self.cursor.push_injection(body, name.clone());
// Re-drive: the next pull lexes the body's first word at command position, so a
// DIFFERENT leading alias still expands (name is active, so it cannot re-expand).
self.maybe_expand_command_alias()?;
Ok(())
```

Key differences from today: (a) no `tokenize(&body)`; (b) `active.remove(&name)` no longer happens here — it happens when the injected source is exhausted (Step 2's pop hook), so the guard spans the whole body lex, not just the synchronous recursion. Confirm the recursion test (Step 4) passes: `alias ls='ls --color'` must expand exactly once.

Note on re-driving: `maybe_expand_command_alias` here re-checks the body's *first* command word. Because pushing an injection does not itself produce a token, you may need to `fill_to(self.pos)` (as the current code does at entry) so the body's first token exists before the boundary check. Preserve the existing `fill_to`/`get(self.pos)` entry logic.

- [ ] **Step 6: Build + verify the `tokenize(&body)` alias call is gone**

Run: `( ulimit -v 2500000; cargo build -p huck-syntax 2>&1 | tail -8 )`
Expected: 0 warnings.
Run: `grep -n "tokenize(" crates/huck-syntax/src/lexer.rs | grep -v "fn tokenize" | grep -v "arith::tokenize"`
Expected: the `maybe_expand_command_alias` line (~5072) is gone; remaining hits are oracle-internal only (to be deleted in Task 4).

- [ ] **Step 7: Run alias tests + harnesses**

```bash
( ulimit -v 2500000; cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 alias 2>&1 | tail -6 )
cd /home/john/projects/huck; export HUCK_BIN=$(pwd)/target/debug/huck
( ulimit -v 2500000; cargo build -p huck 2>&1 | tail -1 )
for f in $(ls tests/scripts/alias_*_diff_check.sh); do
  ( ulimit -v 1500000; timeout 90 bash "$f" ) >/dev/null 2>&1 && echo "PASS $(basename $f)" || echo "FAIL $(basename $f)"
done
```
Expected: all PASS.

- [ ] **Step 8: Full suites + broad bash-diff sweep (alias interacts widely)**

```bash
( ulimit -v 2500000; cargo test -p huck-syntax --jobs 1 --lib 2>&1 | grep "test result:" )
( ulimit -v 2500000; cargo test -p huck-engine --jobs 1 --lib 2>&1 | grep "test result:" )
pass=0; fail=0
for f in tests/scripts/*_diff_check.sh; do
  ( ulimit -v 1500000; timeout 90 bash "$f" ) >/dev/null 2>&1 && pass=$((pass+1)) || { fail=$((fail+1)); echo "FAIL $(basename $f)"; }
done
echo "sweep: pass=$pass fail=$fail"
```
Expected: both suites ok; sweep 1688 pass / 1 fail (`funcnest` only).

- [ ] **Step 9: Commit**

```bash
git add crates/huck-syntax/src/lexer.rs crates/huck-engine
git commit -m "v266 T2: alias bridge — input-source stack replaces tokenize+splice

maybe_expand_command_alias pushes the alias body as a nested CharCursor source
read inline as atoms (opener signals flow to the parser); recursion guard
(active set) re-anchored to source-stack frames; trailing-blank eligibility
preserved. Body tokens pin to the alias-name span. No more tokenize(&body).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Confirm the oracle is a test-only island; convert/delete oracle-referencing tests

With both bridges ported, no production code reaches the oracle. Prove it, then remove the tests that keep it referenced so Task 4's deletion compiles.

**Files:**
- Modify/delete tests in: `crates/huck-syntax/src/lexer.rs`, `crates/huck-syntax/src/command.rs`, `crates/huck-syntax/src/parser.rs` (any remaining `atoms_*_matches_oracle` / `tokenize`-based tests).

- [ ] **Step 1: Audit — every oracle entry point has zero PRODUCTION callers**

```bash
cd /home/john/projects/huck
testline=8134   # main lexer test module; re-confirm with: grep -n '^mod tests' crates/huck-syntax/src/lexer.rs
for pat in 'command::parse(' 'from_tokens(' '\btokenize(' 'tokenize_with_opts(' 'tokenize_no_brace(' 'tokenize_partial_inner('; do
  echo "== $pat (production only) =="
  grep -nE "$pat" crates/huck-syntax/src/*.rs crates/huck-engine/src/*.rs | grep -v "arith::tokenize" | grep -v "fn tokenize" || echo "  (none)"
done
```
Expected: the only hits are (a) oracle-internal (inside functions slated for deletion in Task 4) and (b) inside `#[cfg(test)]`. NO hit in `huck-engine` production, NO hit in an atom-path function. If any atom-path production hit remains, STOP — a bridge was missed; return to Task 1/2.

- [ ] **Step 2: List oracle-referencing tests**

```bash
grep -rn "matches_oracle\|old_seq\|old_part\|command::parse\|from_tokens\|= tokenize" crates/huck-syntax/src/*.rs | grep -iE "test|fn .*oracle|old_" | head -60
```
These are differential/oracle tests. v265 T3 already converted most; whatever remains gets converted to atom-only assertions or deleted (they compared atom-vs-oracle; with the oracle gone the comparison is meaningless).

- [ ] **Step 3: Convert or delete each**

For each remaining oracle-referencing test: if it asserts a real behavior, rewrite it to assert that behavior on `new_seq`/`parse_sequence` alone (drop the oracle side). If it exists ONLY to compare atom-vs-oracle, delete it. Do not weaken coverage of a real behavior — port the expectation to an explicit value.

- [ ] **Step 4: Suites green (counts drop as oracle-only tests go)**

```bash
( ulimit -v 2500000; cargo test -p huck-syntax --jobs 1 --lib 2>&1 | grep "test result:" )
```
Expected: ok, count ≤ prior (oracle-only tests removed). 0 failures.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "v266 T3: oracle is now a test-only island — convert/delete oracle-referencing tests

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Delete the oracle (compiler-assisted, grep-verified)

Remove the oracle. `dead_code` under-reports mutually-recursive cycles (the v265 post-mortem), so after each removal grep-assert zero remaining references rather than trusting the lint.

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` (delete the Word scanner, forward-scanners, `tokenize` family, old word-part scanners, `Mode::Command` non-atom branch, `command_atoms` flag).
- Modify: `crates/huck-syntax/src/command.rs` (delete `parse`/`parse_one_unit` + recursive-descent body; keep AST types).

- [ ] **Step 1: Delete the entry points**

Remove `command::parse` + `parse_one_unit` (and their private helpers) from `command.rs`; remove `tokenize`, `tokenize_with_opts`, `tokenize_no_brace`, `tokenize_partial_inner`, `from_tokens`, and the test-only `pub fn remaining` from `lexer.rs`.

Run: `( ulimit -v 2500000; cargo build -p huck-syntax 2>&1 | grep -E "^error" | head -40 )`
Work the error list: each names a now-uncompilable caller — all should be oracle-internal (below). Do NOT patch a caller to keep the oracle alive; delete the caller if it is oracle-internal.

- [ ] **Step 2: Delete outward, one cluster at a time, grep-verifying each**

Delete, in this order, confirming zero references after each (`grep -n "<name>(" crates/huck-syntax/src/*.rs | grep -v "fn <name>"` → empty, ignoring `#[cfg(test)]`):
`scan_step_command` (non-atom branch), the 6 forward-scanners, `parse_substitution_body`, `scan_paren_substitution`, `scan_dollar_expansion`, `scan_braced_param_expansion`, `scan_param_subscript`, `scan_array_literal`, `scan_array_element_word`, `scan_subscript`, the old `parse_subscript_body`, `with_in_dquote`, `fd_prefix_of_text`, `scan_dquote_expansion_body`, `scan_regex_operand`, `scan_extglob_group`, `scan_expanding_body_line`, `scan_arith_body`, and their private helpers.

After each cluster: `( ulimit -v 2500000; cargo build -p huck-syntax 2>&1 | grep -cE "^error" )` → drive toward 0.

- [ ] **Step 3: Retire the `command_atoms` flag and `Mode::Command` non-atom branch**

`command_atoms` is now always true. Remove the field and the dead non-atom branch in `Mode::Command`'s scan step; `new_live` and `new_live_atoms` collapse (keep both names as thin constructors if the parser references them, or unify — grep call sites first).

Run: `( ulimit -v 2500000; cargo build -p huck-syntax 2>&1 | tail -5 )`
Expected: builds, 0 warnings.

- [ ] **Step 4: Full verification — suites + entire bash-diff sweep**

```bash
( ulimit -v 2500000; cargo test -p huck-syntax --jobs 1 --lib 2>&1 | grep "test result:" )
( ulimit -v 2500000; cargo test -p huck-engine --jobs 1 --lib 2>&1 | grep "test result:" )
cd /home/john/projects/huck; export HUCK_BIN=$(pwd)/target/debug/huck
( ulimit -v 2500000; cargo build -p huck 2>&1 | tail -1 )
pass=0; fail=0
for f in tests/scripts/*_diff_check.sh; do
  ( ulimit -v 1500000; timeout 90 bash "$f" ) >/dev/null 2>&1 && pass=$((pass+1)) || { fail=$((fail+1)); echo "FAIL $(basename $f)"; }
done
echo "sweep: pass=$pass fail=$fail"
```
Expected: both suites ok; sweep **1688 pass / 1 fail** (`funcnest` only). Any other failure is a regression — bisect before proceeding.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "v266 T4: delete the oracle — command::parse + Word lexer + forward-scanners gone

The atom parser is the sole front-end. Removed command::parse/parse_one_unit,
the tokenize family, from_tokens, the Word scanner, the 6 forward-scanners, the
old word-part scanners, and the command_atoms flag / Mode::Command non-atom
branch. bash-diff sweep unchanged (1688/1).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Module tidy — `command.rs` = AST, `lexer.rs` = tokens, `parser.rs` = parsing

**Files:**
- Modify: `crates/huck-syntax/src/command.rs` (AST types only now); `lexer.rs`/`parser.rs` doc headers.

- [ ] **Step 1: Confirm `command.rs` holds only AST types + helpers**

Run: `grep -nE "pub fn |fn " crates/huck-syntax/src/command.rs | grep -v "test" | head -40`
Expected: only AST constructors/accessors + `try_split_assignment`/`try_split_assignment_ref` (assignment helpers on the AST) remain — no `parse`/`parse_one_unit`. If a parse-ish fn survives, move or delete it.

- [ ] **Step 2: Update module doc headers**

Update the top-of-file doc comments: `command.rs` = "AST types for the shell grammar"; `lexer.rs` = "token production (atoms)"; `parser.rs` = "the parser — all parsing and word/structure assembly". Remove stale references to the oracle / differential path / `tokenize` in doc comments across the three files (grep `oracle`, `differential`, `tokenize` in comments).

- [ ] **Step 3: Build + suites green, 0 warnings**

```bash
( ulimit -v 2500000; cargo build -p huck-syntax 2>&1 | tail -3 )
( ulimit -v 2500000; cargo test -p huck-syntax --jobs 1 --lib 2>&1 | grep "test result:" )
```
Expected: 0 warnings; ok.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "v266 T5: module tidy — command.rs = AST, lexer.rs = tokens, parser.rs = parsing

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: Final gate — full sweep, coverage backfill, warning scrub

**Files:** any (final polish).

- [ ] **Step 1: Whole-branch build + both suites + full sweep + binary**

```bash
cd /home/john/projects/huck
( ulimit -v 2500000; cargo build -p huck-syntax 2>&1 | tail -2 )
( ulimit -v 2500000; cargo build -p huck-engine 2>&1 | tail -2 )
( ulimit -v 2500000; cargo build -p huck 2>&1 | tail -1 )
( ulimit -v 2500000; cargo test -p huck-syntax --jobs 1 --lib 2>&1 | grep "test result:" )
( ulimit -v 2500000; cargo test -p huck-engine --jobs 1 --lib 2>&1 | grep "test result:" )
export HUCK_BIN=$(pwd)/target/debug/huck
pass=0; fail=0
for f in tests/scripts/*_diff_check.sh; do
  ( ulimit -v 1500000; timeout 90 bash "$f" ) >/dev/null 2>&1 && pass=$((pass+1)) || { fail=$((fail+1)); echo "FAIL $(basename $f)"; }
done
echo "sweep: pass=$pass fail=$fail"
```
Expected: 0 warnings all three crates; both suites ok; sweep 1688/1.

- [ ] **Step 2: Coverage backfill**

Confirm focused tests exist for both ported bridges (from Tasks 1-2). If any gap: add explicit-value tests (subscript `$()`/`$i`/`+=`/multiword-collapse/assoc-key; alias `$()`-body/trailing-space/recursion/chain). Re-run the two suites.

- [ ] **Step 3: Final line-count + reference check (the oracle is truly gone)**

```bash
for s in "command::parse" "fn tokenize" "from_tokens" "scan_step_command\b" "parse_substitution_body" "scan_dollar_expansion"; do
  echo "== $s =="; grep -rn "$s" crates/huck-syntax/src crates/huck-engine/src | grep -v "arith::tokenize" || echo "  (gone)"
done
git diff --stat main..HEAD | tail -1
```
Expected: each oracle symbol `(gone)` (or only a historical comment); the diffstat shows the large net deletion.

- [ ] **Step 4: Commit any backfill**

```bash
git add -A
git commit -m "v266 T6: final gate — coverage backfill + oracle-gone verification

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Self-review notes (for the executor)

- **Behavior gate is the bash-diff sweep (1688/1).** Every task that changes lexing/parsing re-runs the relevant harnesses; Tasks 2, 4, 6 run the full sweep. A new failure = regression; bisect, do not rebase expectations.
- **Do not keep the oracle alive to fix a Task-4 error.** Every uncompilable caller after the entry points go should be oracle-internal or a test; delete it. If a *production atom-path* caller appears, a bridge was missed — return to Task 1/2.
- **`parse_fragment_word` error mapping** (Task 1 Step 5) is the one fiddly integration point — verify the `ParseError`→`LexError` path against a real build.
- **Alias `active` lifetime** (Task 2) is the correctness crux — the recursion test (`alias ls='ls --color'` expands once; `a→b→a` terminates) is the gate that it's re-anchored correctly to source-stack pop.
