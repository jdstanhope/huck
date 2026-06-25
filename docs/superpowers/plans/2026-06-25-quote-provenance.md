# `Word` quote-span provenance (`WordPart::Quoted`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a `WordPart::Quoted` wrapper that preserves each quoted run's style + span, making `declare -f` / `type` reconstruction byte-identical to bash 5.2.21 for quoted words, and flipping the `cprint` bash-test category to PASS.

**Architecture:** New `QuoteStyle` enum + `WordPart::Quoted { style, parts }` variant. Inner parts keep `quoted: true` (decision B1), so expansion is unchanged except for a recursion arm. The lexer wraps quoted runs; `generate.rs` renders them by style. Landed in three tasks: (1) introduce the variant + handle it in every consumer (inert, behavior unchanged) + render it; (2) make the lexer produce it (behavior changes, regression-guarded); (3) verify the cprint flip + bookkeeping.

**Tech Stack:** Rust workspace (`huck-syntax` lexer/generate, `huck-engine` expand/builtins); bash diff harnesses under `tests/scripts/*_diff_check.sh`.

## Global Constraints

- Commit trailer on EVERY commit, verbatim: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- Run the FULL suite with `cargo test --workspace` (~3672 baseline). A plain `cargo test` skips most crates.
- **No expansion-semantics change.** Decision B1: the wrapper's inner parts keep `quoted: true`; the existing splitting/glob/quoted-null logic is unchanged. The full suite + the quoting/splitting diff harnesses (`ifs_diff_check.sh`, `operand_dquote_context_diff_check.sh`, `alternate_word_quoting_diff_check.sh`, `dollar_quote_forms_diff_check.sh`, `pattern_operand_quoting_diff_check.sh`) are the regression gate; any harness regression blocks the change.
- Byte-faithfulness oracle is bash **5.2.21** (system `bash`), non-interactive `declare -f`/`type`. Capture exact strings with `bash -c '<frag>; declare -f f' | cat -A`.
- **Catch-all hazard:** several `match WordPart` sites end in `_ => …` (forward-compat) and will NOT compile-error on the new variant — they must be audited and given an explicit `Quoted` arm, or they silently mishandle quoted content once the lexer produces it (Task 1 must fix these BEFORE Task 2 activates production).
- GPL posture: read bash behavior from system bash / `$BASH_SOURCE_DIR`; never vendor source or paste bash `.right`/output into committed files.
- Do NOT push to main or merge without explicit user confirmation.

### The model (reference for all tasks)

```rust
pub enum QuoteStyle { Single, Double, AnsiC, Backslash }   // bareword = not wrapped
WordPart::Quoted { style: QuoteStyle, parts: Vec<WordPart> }
```

Worked forms (source → AST → bash-exact reconstruction):
- `ab'cd'ef` → `[Literal{"ab",false}, Quoted{Single,[Literal{"cd",true}]}, Literal{"ef",false}]` → `ab'cd'ef`
- `"$a $b"` → `[Quoted{Double,[Var{a,true},Literal{" ",true},Var{b,true}]}]` → `"$a $b"`
- `\$PWD` → `[Quoted{Backslash,[Literal{"$",true}]}, Literal{"PWD",false}]` → `\$PWD`
- `\&\|'()'` → `[Quoted{Backslash,["&"]}, Quoted{Backslash,["|"]}, Quoted{Single,["()"]}]` → `\&\|'()'`
- `"a b""c d"` → two adjacent `Quoted{Double,…}` → `"a b""c d"`

---

### Task 1: Introduce `WordPart::Quoted`, handle it in every consumer, render it

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` (add `QuoteStyle` + `Quoted` variant; do NOT yet produce it)
- Modify: `crates/huck-syntax/src/generate.rs` (`part_to_source`, `pattern_word_to_source` — render the variant; add unit tests)
- Modify: `crates/huck-engine/src/expand.rs` (`expand` central loop, `expand_assignment`, `reconstruct_part`, `word_part_is_quoted` — handle the variant)
- Modify: any other file with an exhaustive or catch-all `WordPart` match the compiler/audit flags: `crates/huck-engine/src/param_expansion.rs`, `executor.rs`, `builtins.rs`, `alias_expand.rs`, `crates/huck-syntax/src/command.rs`, `crates/huck-syntax/examples/*.rs`
- Test: `crates/huck-syntax/src/generate.rs` (tests module)

**Interfaces:**
- Produces: `QuoteStyle`, `WordPart::Quoted { style: QuoteStyle, parts: Vec<WordPart> }`. Inner `parts` carry `quoted: true`. Task 2 (lexer) constructs these; Task 1 only declares + handles + renders them.

**Method:** The variant is added but NOT produced by the lexer, so the whole change is inert — every existing test must still pass. The new tests construct `Quoted` ASTs directly to prove rendering.

- [ ] **Step 1: Add the enum + variant**

In `lexer.rs`, near the `WordPart` definition:

```rust
/// The original source quoting style of a `WordPart::Quoted` run, preserved
/// so `declare -f` / `type` reconstruction reproduces bash's exact bytes.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum QuoteStyle {
    Single,    // '…'
    Double,    // "…"
    AnsiC,     // $'…'
    Backslash, // \c
}
```

Add to `enum WordPart` (keep all existing variants):

```rust
    /// One contiguous quoted run, preserving source `style` and span. Inner
    /// `parts` keep their own `quoted: true` flag (decision B1) so the
    /// expansion path is unchanged; the wrapper exists for reconstruction.
    Quoted { style: QuoteStyle, parts: Vec<WordPart> },
```

- [ ] **Step 2: Build — let the compiler list the exhaustive-match sites**

Run: `cargo build --workspace 2>&1 | grep -A2 'non-exhaustive\|E0004' | head -40`
Expected: compile errors at every EXHAUSTIVE `match WordPart` (no `_` arm) — notably the central `expand()` loop (`expand.rs:1019`) and `generate.rs` `part_to_source`. These are the compiler-found sites. Record the list.

- [ ] **Step 3: Render the variant in `generate.rs` (the reconstruction payoff)**

In `part_to_source` (the `match part`), add this single arm (the `Double` style
double-quote-escapes literal content and keeps expansions; the other three
styles take the inner text/expansion verbatim under their own delimiters):

```rust
        WordPart::Quoted { style, parts } => {
            use crate::lexer::QuoteStyle;
            match style {
                QuoteStyle::Double => {
                    let inner: String = parts.iter().map(|p| match p {
                        WordPart::Literal { text, .. } =>
                            crate::escape_double_quote_value(text),
                        other => part_to_source(other),
                    }).collect();
                    format!("\"{inner}\"")
                }
                QuoteStyle::Single => {
                    let inner: String = parts.iter().map(quoted_inner_to_source).collect();
                    format!("'{inner}'")
                }
                QuoteStyle::Backslash => {
                    let inner: String = parts.iter().map(quoted_inner_to_source).collect();
                    format!("\\{inner}")
                }
                QuoteStyle::AnsiC => {
                    let inner: String = parts.iter().map(quoted_inner_to_source).collect();
                    format!("$'{}'", ansi_c_escape(&inner))
                }
            }
        }
```

Add two helpers in `generate.rs`:

```rust
/// Render one part inside a Single/Backslash/AnsiC quoted run: a `Literal`
/// contributes its text verbatim (the run owns the quotes); an expansion renders
/// via its normal source form WITHOUT re-quoting. (The `Double` style escapes
/// literals itself, so it does not use this helper.)
fn quoted_inner_to_source(part: &WordPart) -> String {
    match part {
        WordPart::Literal { text, .. } => text.clone(),
        other => part_to_source(other),
    }
}

/// ANSI-C re-escape for `$'…'`: turn control chars back into their escapes.
fn ansi_c_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}
```

Also add a `Quoted` arm to `pattern_word_to_source` (case patterns) that renders
the same way (reuse: call `part_to_source(part)` for the `Quoted` case, since a
quoted run in a pattern is a literal span):

```rust
            other => part_to_source(other),  // already the fallback — confirm Quoted hits it
```
(If `pattern_word_to_source` matches only `Literal{quoted:false}` specially and
falls through `other => part_to_source(other)`, no change is needed — verify.)

- [ ] **Step 4: Write generate unit tests (constructed ASTs)**

In the `generate.rs` tests module:

```rust
#[test]
fn render_quoted_single() {
    use crate::lexer::{QuoteStyle, Word, WordPart};
    let w = Word(vec![WordPart::Quoted {
        style: QuoteStyle::Single,
        parts: vec![WordPart::Literal { text: "what a window".into(), quoted: true }],
    }]);
    assert_eq!(word_to_source(&w), "'what a window'");
}
#[test]
fn render_quoted_double_span() {
    use crate::lexer::{QuoteStyle, Word, WordPart};
    let w = Word(vec![WordPart::Quoted {
        style: QuoteStyle::Double,
        parts: vec![
            WordPart::Var { name: "a".into(), quoted: true },
            WordPart::Literal { text: " ".into(), quoted: true },
            WordPart::Var { name: "b".into(), quoted: true },
        ],
    }]);
    assert_eq!(word_to_source(&w), "\"$a $b\"");
}
#[test]
fn render_quoted_backslash() {
    use crate::lexer::{QuoteStyle, Word, WordPart};
    let w = Word(vec![
        WordPart::Quoted { style: QuoteStyle::Backslash,
            parts: vec![WordPart::Literal { text: "$".into(), quoted: true }] },
        WordPart::Literal { text: "PWD".into(), quoted: false },
    ]);
    assert_eq!(word_to_source(&w), "\\$PWD");
}
#[test]
fn render_quoted_adjacent_double() {
    use crate::lexer::{QuoteStyle, Word, WordPart};
    let run = |t: &str| WordPart::Quoted { style: QuoteStyle::Double,
        parts: vec![WordPart::Literal { text: t.into(), quoted: true }] };
    let w = Word(vec![run("a b"), run("c d")]);
    assert_eq!(word_to_source(&w), "\"a b\"\"c d\"");
}
#[test]
fn render_quoted_double_escapes_specials() {
    use crate::lexer::{QuoteStyle, Word, WordPart};
    let w = Word(vec![WordPart::Quoted { style: QuoteStyle::Double,
        parts: vec![WordPart::Literal { text: "a\"b$c".into(), quoted: true }] }]);
    // inside "...", a literal " and $ must be backslash-escaped
    assert_eq!(word_to_source(&w), "\"a\\\"b\\$c\"");
}
#[test]
fn render_quoted_ansic_newline() {
    use crate::lexer::{QuoteStyle, Word, WordPart};
    let w = Word(vec![WordPart::Quoted { style: QuoteStyle::AnsiC,
        parts: vec![WordPart::Literal { text: "i\n".into(), quoted: true }] }]);
    assert_eq!(word_to_source(&w), "$'i\\n'");
}
```

Run: `cargo test -p huck-syntax render_quoted_`
Expected: FAIL before Step 3's code, PASS after. (Confirm `escape_double_quote_value` escapes `"` and `$` as the `render_quoted_double_escapes_specials` test asserts; if its escaping differs, adjust the test's expected string to match the actual bash 5.2.21 output of `f(){ echo "a\"b\$c"; }; declare -f f` and the implementation, not the other way round.)

- [ ] **Step 5: Handle the variant in expansion (`expand.rs`) — recurse, preserve semantics**

The central `expand()` loop (`expand.rs:1019`) is exhaustive → the compiler
flagged it. Refactor its loop body into a helper so the `Quoted` arm can recurse:

1. Extract the body of `for part in &word.0 { match part { … } }` into
   `fn expand_part(part: &WordPart, current: &mut Field, result: &mut Vec<Field>, has_emitted: &mut bool, shell: &mut Shell, snapshot_status: i32) -> std::ops::ControlFlow<()>`.
   Every existing `return result;` inside the body becomes `return ControlFlow::Break(());`
   (the nounset-error early exits). Arms that fall through become `ControlFlow::Continue(())`.
2. The loop becomes:
   ```rust
   for part in &word.0 {
       if expand_part(part, &mut current, &mut result, &mut has_emitted, shell, snapshot_status).is_break() {
           return result;
       }
   }
   ```
3. Add the `Quoted` arm inside `expand_part`:
   ```rust
   WordPart::Quoted { parts, .. } => {
       for inner in parts {
           expand_part(inner, current, result, has_emitted, shell, snapshot_status)?;
       }
   }
   ```
   (`?` on `ControlFlow` propagates `Break` — `expand_part` returns `ControlFlow<()>`.)

This is a **behavior-preserving extraction**: with the lexer not yet producing
`Quoted`, every existing expansion test must still pass — that is the correctness
gate for the refactor.

- [ ] **Step 6: Handle the catch-all sites (compiler will NOT flag these)**

Audit every `match WordPart` for a `_ =>` arm; add an explicit `Quoted` arm
BEFORE the catch-all. Known sites:

`word_part_is_quoted` (`expand.rs:1661`) — a quoted run IS quoted:
```rust
        WordPart::Quoted { .. } => true,
```

`expand_assignment` (`expand.rs:1552`, has `_ => {}`) — must contribute the inner
content. Mirror Step 5: extract its loop body into `expand_assignment_part(part, &mut result, shell, snapshot_status)` and add:
```rust
        WordPart::Quoted { parts, .. } => {
            for inner in parts { expand_assignment_part(inner, result, shell, snapshot_status); }
        }
```
(No early returns here, so no `ControlFlow` needed — a plain `for` loop.)

`reconstruct_part` (`expand.rs:1367`, xtrace, has `_ => {}`) — recurse so quoted
content still appears in xtrace (preserves current behavior):
```rust
        P::Quoted { parts, .. } => { for inner in parts { reconstruct_part(inner, out); } }
```

Then grep every other `WordPart`-matching file for `_ =>`:
`param_expansion.rs`, `executor.rs`, `builtins.rs`, `alias_expand.rs`,
`command.rs`, the `examples/*.rs`. For each catch-all match, add a `Quoted` arm
that recurses over `parts` applying the same per-part handling (so wrapped
content is treated exactly as the inner parts were before wrapping). For
exhaustive matches the compiler already forced the arm in Step 2.

Run: `cargo build --workspace` → clean. Then `cargo test --workspace`.
Expected: **all existing tests pass unchanged** (the variant is inert; the
refactors are behavior-preserving) PLUS the new `render_quoted_*` tests pass.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "$(printf 'v219 task 1: add WordPart::Quoted variant, handle + render it everywhere\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 2: Lexer produces `WordPart::Quoted`; end-to-end byte-exactness

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` (the single/double/backslash/ANSI-C scan blocks; `scan_ansi_c_quoted`)
- Modify: `tests/scripts/declare_f_diff_check.sh` (add quoting fragments)
- Test: `crates/huck-syntax/src/generate.rs` and/or `crates/huck-engine` (end-to-end `declf_*` via parse)

**Interfaces:**
- Consumes: `WordPart::Quoted` + the rendering/handling from Task 1.

**Method:** Restructure each quote-scan block to collect its run into a local
sub-vec and push one `WordPart::Quoted`. Inner parts keep `quoted: true`. This is
where behavior changes; the full suite + quoting harnesses guard regressions.

- [ ] **Step 1: Write the failing end-to-end tests**

Add to the `generate.rs` tests module a helper + exact-match cases (strings are
bash 5.2.21 captures — verify each with `bash -c '<frag>; declare -f f' | cat -A`):

```rust
fn declf_word(body_word_src: &str) -> String {
    // define `f(){ echo <body_word_src>; }`, return the function reconstruction
    use crate::{command, lexer};
    let src = format!("f(){{ echo {body_word_src}; }}");
    let seq = command::parse(lexer::tokenize(&src).unwrap()).unwrap().unwrap();
    let command::Command::FunctionDef { name, body } = seq.first else { panic!() };
    function_to_source(&name, &body)
}

#[test] fn rt_quote_single()   { assert!(declf_word("'what a window'").contains("echo 'what a window'")); }
#[test] fn rt_quote_dq_span()  { assert!(declf_word("\"$a $b\"").contains("echo \"$a $b\"")); }
#[test] fn rt_quote_backslash(){ assert!(declf_word("\\$PWD").contains("echo \\$PWD")); }
#[test] fn rt_quote_adjacent() { assert!(declf_word("\"a b\"\"c d\"").contains("echo \"a b\"\"c d\"")); }
#[test] fn rt_quote_mixed()    { assert!(declf_word("ab'cd'ef").contains("echo ab'cd'ef")); }
#[test] fn rt_quote_specials() { assert!(declf_word("\\&\\|'()'").contains("echo \\&\\|'()'")); }
```

Run: `cargo test -p huck-syntax rt_quote_`
Expected: FAIL (lexer still emits flat parts → huck renders `"$a"" ""$b"` etc.)

- [ ] **Step 2: Wrap the single-quote run**

In `lexer.rs` (~line 540, the `'\''` block), collect the run into a local
`Vec<WordPart>` and push one `Quoted`:
```rust
'\'' => {
    has_token = true;
    flush_literal(&mut parts, &mut current, false);
    let mut run: Vec<WordPart> = Vec::new();
    let mut buf = String::new();
    loop {
        match chars.next() {
            Some('\'') => break,
            Some(ch) => buf.push(ch),
            None => return Err(LexError::UnterminatedQuote),
        }
    }
    // empty '' still yields one empty quoted Literal (empty-token contract)
    run.push(WordPart::Literal { text: buf, quoted: true });
    parts.push(WordPart::Quoted { style: QuoteStyle::Single, parts: run });
}
```

- [ ] **Step 3: Wrap the double-quote run**

In the `'"'` block (~line 558), collect into a local `run` vec. `flush_literal`
and `scan_dollar_expansion` take `&mut Vec<WordPart>` — pass `&mut run`:
```rust
'"' => {
    has_token = true;
    flush_literal(&mut parts, &mut current, false);
    let mut run: Vec<WordPart> = Vec::new();
    let mut qbuf = String::new();
    loop {
        match chars.next() {
            Some('"') => break,
            Some('\\') => match chars.next() {
                Some(esc @ ('"' | '\\' | '$' | '`')) => qbuf.push(esc),
                Some('\n') => {}
                Some(other) => { qbuf.push('\\'); qbuf.push(other); }
                None => return Err(LexError::UnterminatedQuote),
            },
            Some('$') => { flush_literal(&mut run, &mut qbuf, true);
                           scan_dollar_expansion(&mut chars, &mut run, true, opts)?; }
            Some('`') => { flush_literal(&mut run, &mut qbuf, true);
                           let sequence = scan_backtick_substitution(&mut chars, opts)?;
                           run.push(WordPart::CommandSub { sequence, quoted: true }); }
            Some(ch) => qbuf.push(ch),
            None => return Err(LexError::UnterminatedQuote),
        }
    }
    flush_literal(&mut run, &mut qbuf, true);
    if run.is_empty() {
        run.push(WordPart::Literal { text: String::new(), quoted: true }); // "" contract
    }
    parts.push(WordPart::Quoted { style: QuoteStyle::Double, parts: run });
}
```

- [ ] **Step 4: Wrap the backslash escape**

In the `'\\'` block (~line 601), wrap the one-char escaped literal:
```rust
Some(ch) => {
    has_token = true;
    flush_literal(&mut parts, &mut current, false);
    parts.push(WordPart::Quoted {
        style: QuoteStyle::Backslash,
        parts: vec![WordPart::Literal { text: ch.to_string(), quoted: true }],
    });
}
```
(Leave the `\<newline>` line-continuation arm unchanged.)

- [ ] **Step 5: Wrap the ANSI-C run**

In `scan_ansi_c_quoted` (~line 1889): wrap the decoded value as
`WordPart::Quoted { style: QuoteStyle::AnsiC, parts: vec![Literal{decoded, quoted:true}] }`
instead of pushing a bare `Literal`. (Find where it currently pushes the decoded
`Literal` and wrap it.)

- [ ] **Step 6: Run the end-to-end + full suite + harnesses**

Run: `cargo test -p huck-syntax rt_quote_` → PASS.
Run: `cargo test --workspace` → all pass (~3672+). Investigate ANY new failure —
a lexer-test asserting the old flat-parts shape must be updated to the wrapped
shape (the AST genuinely changed); an EXPANSION test failure is a real
regression — fix the handling, do not weaken the test.
Run the quoting harnesses:
```bash
cargo build --release --bin huck   # long timeout (~5 min)
for h in ifs operand_dquote_context alternate_word_quoting dollar_quote_forms pattern_operand_quoting; do
  bash tests/scripts/${h}_diff_check.sh | tail -1
done
```
Expected: each `Fail: 0`.

- [ ] **Step 7: Extend the reconstruction harness**

Add to `tests/scripts/declare_f_diff_check.sh` `fragments`:
```bash
  'f(){ echo "$a $b"; }; declare -f f'
  "f(){ echo 'what a fabulous window treatment'; }; declare -f f"
  'f(){ echo \$PWD in \$PATH; }; declare -f f'
  "f(){ echo \\&\\|'()'; }; declare -f f"
  'f(){ echo "a b""c d"; }; declare -f f'
  "f(){ echo ab'cd'ef; }; declare -f f"
```
Run: `bash tests/scripts/declare_f_diff_check.sh | tail -1` → `Fail: 0`.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "$(printf 'v219 task 2: lexer wraps quoted runs in WordPart::Quoted\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 3: Verify the cprint flip + divergence/baseline bookkeeping

**Files:**
- Modify: `docs/bash-divergences.md` (resolve the provenance portion of L-57; scope its successor to herestr's remaining blockers; note L-21(a))
- Modify: `docs/bash-test-suite-baseline.md` (cprint → PASS; herestr note trimmed)

**Interfaces:** none (verification + docs).

- [ ] **Step 1: Confirm cprint flips**

Run:
```bash
BASH_SOURCE_DIR=/tmp/bash-5.2.21 HUCK_BASH_TEST_HELPERS=/tmp/bash-test-helpers \
  HUCK_BASH_TEST_CATEGORY=cprint bash tests/bash-test-suite/runner.sh
```
Expected: cprint **PASS** (0 diff). If a residual remains, capture it
(`huck cprint.tests` vs `cprint.right` under `$BASH_SOURCE_DIR/tests`) and report
DONE_WITH_CONCERNS with the exact diff — the flip is the success criterion.

- [ ] **Step 2: Re-measure herestr (should shrink, not flip)**

Run the same with `HUCK_BASH_TEST_CATEGORY=herestr`. Confirm the provenance hunks
(`"$a $b"`, `'what a fabulous…'`, `'double"quote'`) are gone; the residual should
be only the `declare -p` ANSI-C value (`$'i\n'`) + the runtime `command not
found:` lines. Record the measured residual for the baseline.

- [ ] **Step 3: Update `docs/bash-divergences.md`**

- L-57: DELETE the quote-provenance-in-reconstruction content (resolved in v219).
  Replace with a successor `[deferred]` entry scoped to herestr's remaining
  blockers: (1) `declare -p` ANSI-C control-char VALUE quoting (`$'…'`), and
  (2) the herestr runtime `command not found:` (empty command name) bug — each
  with the measured evidence from Step 2, in huck-authored prose (no bash output
  pasted).
- L-21(a): add a one-line note that `WordPart::Quoted` now carries source quote
  provenance, available to the xtrace `reconstruct_part` path if wired up later.

- [ ] **Step 4: Re-triage `docs/bash-test-suite-baseline.md`**

- cprint → **PASS**; update its note and the Summary counts.
- herestr → FAIL with the trimmed note (only the two remaining blockers).
- Update the header (huck commit / sweep date 2026-06-25).

- [ ] **Step 5: Commit**

```bash
git add docs/bash-divergences.md docs/bash-test-suite-baseline.md
git commit -m "$(printf 'v219 task 3: cprint flips to PASS; L-57 provenance resolved, herestr residual re-scoped\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Notes for the implementer

- **Oracle is system bash 5.2.21.** Capture exact bytes with
  `bash -c '<frag>; declare -f f' | cat -A` before trusting any expected string.
- **B1 is the safety rail:** inner parts keep `quoted: true`; expansion only
  GAINS a recursion arm. If you find yourself changing how `quoted` is READ in
  expansion, stop — that is out of scope and risks regressions.
- **Catch-all audit is mandatory (Task 1 Step 6):** sites ending in `_ =>` will
  not compile-error; a missed one silently drops quoted content once Task 2
  activates production. Grep every `WordPart`-matching file for `_ =>`.
- **The existing suite is the refactor gate:** Task 1's expansion extraction must
  leave every test green (the variant is inert). Task 2's lexer change is where
  behavior moves; an expansion-test failure there is a real regression to fix in
  the handling, never by weakening the test.
- **AnsiC** is included for model completeness; if no in-scope fragment exercises
  the `$'…'` render, the `render_quoted_ansic_newline` unit test is its coverage.
