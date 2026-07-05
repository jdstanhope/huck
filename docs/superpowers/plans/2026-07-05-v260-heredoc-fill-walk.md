# v260 Iteration B — Heredoc-in-Word Fill-Walk (CF1) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** On the dormant atom-command path, fill heredoc bodies whose openers sit inside a Word (command-sub, process-sub, arith, `${…}` operand, array-literal element, quoted span), so the atom AST is byte-identical to the `command.rs` oracle instead of dropping the body as `Word([])`.

**Architecture:** Add a `fill_word` (+ `fill_word_parts` + `fill_param_modifier`) recursion to the post-parse heredoc-body fill walk in `parser.rs`, and extend `fill_command`'s `Simple(Exec)` and `Simple(Assign)` arms to walk the command's own Words, filling from the shared FIFO body queue in source order (**words then redirects**). The single order-sensitive interleaving (a heredoc redirect *before* a heredoc-bearing Word on one command) is pinned — the AST carries no source positions to sort by.

**Tech Stack:** Rust; `huck-syntax` crate; `parser.rs mod tests` differential harness (`diff_cmd(s)` asserts `new_seq(s).unwrap() == old_seq(s).unwrap()`).

## Global Constraints

- Box is 1 core / 1.9 GB and OOM-kills on parallel test runs. Use EXACTLY: `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1`. NEVER `--workspace`, NEVER multiple threads. (Single test: append its name before `--`.)
- `cargo build -p huck-syntax` → 0 warnings.
- `command.rs` gets NO changes (EMPTY diff vs `main`). Confirm `git diff --stat main -- crates/huck-syntax/src/command.rs`.
- Both `command_atoms` sites in `lexer.rs` stay `false`.
- rust-analyzer shows PHANTOM diagnostics after edits; trust `cargo build`/`cargo test`, not the IDE.
- Commit trailer VERBATIM: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- Work on branch `v260-heredoc-fill-walk`. Do NOT touch `main`.
- `diff_cmd(s)` is the equality gate. Do NOT weaken a test; if a `diff_cmd` fails, fix the implementation. All corpus values were probed EQ=false today and must become byte-identical (the pinned case excepted).
- Heredoc test inputs use real newlines. In Rust test strings write them as `\n` (e.g. `"echo $(cat <<X\nhi\nX\n)"`).

---

## File Structure

- `crates/huck-syntax/src/parser.rs` — the fill machinery lives at ~2671–2810 (`fill_redirects`, `fill_command`, `fill_sequence`, `parse_sequence`). Add `fill_word`/`fill_word_parts`/`fill_param_modifier` near them; extend `fill_command`. Import `AssignTarget`. Tests in `mod tests`.
- `crates/huck-syntax/src/command.rs` — UNTOUCHED.

---

### Task 1: `fill_word` recursion + `fill_command` Word-walking

**Files:**
- Modify: `crates/huck-syntax/src/parser.rs` — add `use` for `AssignTarget`; add `fill_word`, `fill_word_parts`, `fill_param_modifier`; extend `fill_command` `Simple(Exec)`/`Simple(Assign)` arms.
- Test: `crates/huck-syntax/src/parser.rs` `mod tests` — new `atoms_heredoc_in_word_fill`.

**Interfaces:**
- Consumes: `fill_sequence(&mut Sequence, &mut impl Iterator<Item = Word>)` and `fill_redirects(&mut [Redirection], …)` (existing, parser.rs:2763 / 2671); AST types `WordPart`, `Word`, `SubscriptKind`, `ParamModifier`, `ArrayLiteralElement` (imported from `crate::lexer`), `AssignTarget`, `Assignment`, `ExecCommand`, `SimpleCommand` (from `crate::command`).
- Produces: `fill_word(&mut Word, &mut impl Iterator<Item = Word>)` (used by Task 2's tests only indirectly).

- [ ] **Step 1: Add the `AssignTarget` import**

In `parser.rs`, the `use crate::command::{ … }` block (line ~6) does not list `AssignTarget`. Add it:

```rust
use crate::command::{
    Command, Sequence, Pipeline, SimpleCommand, ExecCommand, Assignment, Connector, ParseError,
    AssignTarget,
    Redirection, RedirFd, RedirOp, FileMode, word_literal_text, valid_identifier_text, IfClause, ElifBranch, WhileClause,
    ForClause, SelectClause, CaseClause, CaseItem, CaseTerminator, ArithForClause,
    TestExpr, TestUnaryOp, TestBinaryOp, try_unary_op, skip_test_newlines, is_compound_opener,
};
```

- [ ] **Step 2: Write the failing test**

Add to `parser.rs mod tests` (these are all EQ=false today — the bodies are dropped):

```rust
    #[test]
    fn atoms_heredoc_in_word_fill() {
        // v260 CF1: a heredoc nested inside a Word is filled, not dropped.
        diff_cmd("echo $(cat <<X\nhi\nX\n)");                 // arg command-sub
        diff_cmd("x=$(cat <<X\nhi\nX\n)");                    // assignment RHS
        diff_cmd("a=($(cat <<X\nhi\nX\n))");                  // array-literal element value
        diff_cmd("echo ${y:-$(cat <<X\nhi\nX\n)}");           // param-expansion operand
        diff_cmd("echo $(( $(cat <<X\n1\nX\n) + 2 ))");       // arith body
        diff_cmd("echo `cat <<X\nhi\nX\n`");                  // backtick (→ CommandSub)
        diff_cmd("echo \"$(cat <<X\nhi\nX\n)\"");             // inside a quoted span
        diff_cmd("echo $(a <<X\nxx\nX\n)$(b <<Y\nyy\nY\n)");  // two Word-nested (queue order)
        diff_cmd("FOO=$(cat <<X\nhi\nX\n) echo hi");          // inline assignment value
    }
```

- [ ] **Step 3: Run to verify it FAILS**

Run: `cargo test -p huck-syntax --jobs 1 --lib atoms_heredoc_in_word_fill -- --test-threads 1`
Expected: FAIL on the first case (atom inner heredoc body is `Word([])`, oracle is filled).

- [ ] **Step 4: Add `fill_word` + `fill_word_parts` + `fill_param_modifier`**

Insert near `fill_redirects` (before `fill_command`) in `parser.rs`:

```rust
/// v260 CF1: fill heredoc bodies whose openers sit inside a `Word`. Recurses
/// into every nested `Sequence`/`Word` a `WordPart` can carry, in source order,
/// so the shared FIFO body queue attaches each body to its placeholder.
/// EXHAUSTIVE over `WordPart` — no `_ =>` wildcard.
fn fill_word(word: &mut Word, bodies: &mut impl Iterator<Item = Word>) {
    fill_word_parts(&mut word.0, bodies);
}

fn fill_word_parts(parts: &mut [WordPart], bodies: &mut impl Iterator<Item = Word>) {
    for part in parts.iter_mut() {
        match part {
            WordPart::CommandSub { sequence, .. } => fill_sequence(sequence, bodies),
            WordPart::ProcessSub { sequence, .. } => fill_sequence(sequence, bodies),
            WordPart::Arith { body, .. } => fill_word(body, bodies),
            WordPart::Quoted { parts, .. } => fill_word_parts(parts, bodies),
            WordPart::ParamExpansion { subscript, modifier, .. } => {
                // Source order: `${a[i]:-word}` — subscript before the modifier.
                if let Some(SubscriptKind::Index(w)) = subscript {
                    fill_word(w, bodies);
                }
                fill_param_modifier(modifier, bodies);
            }
            WordPart::ArrayLiteral(elems) => {
                for el in elems.iter_mut() {
                    if let Some(sub) = el.subscript.as_mut() {
                        fill_word(sub, bodies); // `[idx]=val` — subscript before value
                    }
                    fill_word(&mut el.value, bodies);
                }
            }
            // No nested Word — nothing to fill.
            WordPart::Literal { .. }
            | WordPart::Tilde(_)
            | WordPart::Var { .. }
            | WordPart::LastStatus { .. }
            | WordPart::AllArgs { .. }
            | WordPart::AssignPrefix { .. } => {}
        }
    }
}

/// EXHAUSTIVE over `ParamModifier`; recurses into each variant's Word(s) in
/// source order.
fn fill_param_modifier(modifier: &mut ParamModifier, bodies: &mut impl Iterator<Item = Word>) {
    match modifier {
        ParamModifier::UseDefault { word, .. }
        | ParamModifier::AssignDefault { word, .. }
        | ParamModifier::ErrorIfUnset { word, .. }
        | ParamModifier::UseAlternate { word, .. } => fill_word(word, bodies),
        ParamModifier::RemovePrefix { pattern, .. }
        | ParamModifier::RemoveSuffix { pattern, .. } => fill_word(pattern, bodies),
        ParamModifier::Substitute { pattern, replacement, .. } => {
            fill_word(pattern, bodies);      // `${x/pat/rep}` — pattern before replacement
            fill_word(replacement, bodies);
        }
        ParamModifier::Substring { offset, length } => {
            fill_word(offset, bodies);       // `${x:off:len}` — offset before length
            if let Some(l) = length.as_mut() {
                fill_word(l, bodies);
            }
        }
        ParamModifier::Case { pattern: Some(p), .. } => fill_word(p, bodies),
        // No Word to fill.
        ParamModifier::Case { pattern: None, .. }
        | ParamModifier::None
        | ParamModifier::Length
        | ParamModifier::IndirectKeys
        | ParamModifier::PrefixNames { .. }
        | ParamModifier::Transform { .. }
        | ParamModifier::BadSubst { .. } => {}
    }
}
```

Note: match the exact field names of the `ParamModifier` variants as defined in `lexer.rs:225` (verify: `UseDefault/AssignDefault/ErrorIfUnset/UseAlternate { word, colon }`, `RemovePrefix/RemoveSuffix { pattern, longest }`, `Substitute { pattern, replacement, anchor, all }`, `Substring { offset, length }`, `Case { direction, all, pattern }`, `PrefixNames { at }`, `Transform { op }`, `BadSubst { raw }`). Use `..` to ignore the non-Word fields as shown.

- [ ] **Step 5: Extend `fill_command`'s `Simple` arms**

In `fill_command` (parser.rs:2689), replace the two `Simple` arms:

```rust
        Command::Simple(SimpleCommand::Assign(items, _)) => {
            // v260 CF1: a bare assignment carries no redirects, but its value
            // Words can nest heredocs (`x=$(cat <<X)`, `a=($(cat <<X))`).
            for a in items.iter_mut() {
                if let AssignTarget::Indexed { subscript, .. } = &mut a.target {
                    fill_word(subscript, bodies);
                }
                fill_word(&mut a.value, bodies);
            }
        }
        Command::Simple(SimpleCommand::Exec(exec)) => {
            // v260 CF1: walk the command's own Words, then its redirects, in
            // source order (words-then-redirects). Inline assignments precede the
            // program, which precedes the args, which precede trailing redirects.
            for a in exec.inline_assignments.iter_mut() {
                if let AssignTarget::Indexed { subscript, .. } = &mut a.target {
                    fill_word(subscript, bodies);
                }
                fill_word(&mut a.value, bodies);
            }
            fill_word(&mut exec.program, bodies);
            for arg in exec.args.iter_mut() {
                fill_word(arg, bodies);
            }
            fill_redirects(&mut exec.redirects, bodies);
        }
```

- [ ] **Step 6: Run the test + full suite + build + gates**

Run: `cargo test -p huck-syntax --jobs 1 --lib atoms_heredoc_in_word_fill -- --test-threads 1` → PASS.
Run: `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` → all pass (the v250 pin `atoms_heredoc_in_cmdsub_body_drop_divergence` at parser.rs:4823 still asserts the OLD dropped behavior — it will now FAIL because the body is filled. That is EXPECTED and is flipped in Task 2. If it fails here, leave it; Task 2 fixes it. If you prefer green between tasks, you may temporarily `#[ignore]` it with a `// v260 T2: flip to diff_cmd` note — but do NOT delete it.)
Run: `cargo build -p huck-syntax` → 0 warnings.
Run: `git diff --stat main -- crates/huck-syntax/src/command.rs` → EMPTY.

- [ ] **Step 7: Commit**

```bash
git add crates/huck-syntax/src/parser.rs
git commit -m "v260 T1: fill heredoc bodies nested inside Words (CF1)

Add fill_word/fill_word_parts/fill_param_modifier — an exhaustive recursion into
every nested Sequence/Word a WordPart can carry (CommandSub/ProcessSub/Arith/
Quoted/ParamExpansion/ArrayLiteral) — and extend fill_command's Simple(Exec)/
Simple(Assign) arms to walk the command's own Words (inline-assign/program/args,
and bare-assignment values) then redirects, in source order. Fixes the dropped
heredoc body for \$(…)/backtick/<(…)/\$((…))/\${…}/array/quoted nestings.
Atom-path only; command.rs EMPTY-diff; command_atoms false.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Source-order interleaving corpus + the pin + flip the v250 pin

**Files:**
- Modify: `crates/huck-syntax/src/parser.rs` `mod tests` — new `atoms_heredoc_word_redirect_order`; new `atoms_heredoc_redirect_before_word_pin`; rewrite `atoms_heredoc_in_cmdsub_body_drop_divergence` (parser.rs:4823) into a `diff_cmd`.

**Interfaces:**
- Consumes: the fill walk from Task 1; the differential harness `diff_cmd`/`new_seq`/`old_seq`.

**Background:** With words-then-redirects, a command that has BOTH a heredoc redirect and a heredoc-bearing Word fills correctly only when the Word precedes the redirect in source (`cat $(sh <<B) <<A` — the idiomatic trailing redirect). The mirror (`cat <<A $(sh <<B)` — redirect first) mis-orders because the AST carries no source positions; that is the documented pin.

- [ ] **Step 1: Write the trailing-redirect (handled) test + regressions**

Add to `parser.rs mod tests`:

```rust
    #[test]
    fn atoms_heredoc_word_redirect_order() {
        // Word-nested heredoc BEFORE a trailing outer redirect heredoc:
        // emission order = arg (B) then redirect (A); words-then-redirects fills
        // them in that order → byte-identical to the oracle.
        diff_cmd("cat $(sh <<B\nbb\nB\n) <<A\naa\nA\n");
        // Regressions: plain / multiple outer-redirect heredocs still fill in order.
        diff_cmd("cat <<A\naa\nA\n");
        diff_cmd("cat <<A <<B\naa\nA\nbb\nB\n");
        diff_cmd("cat <<A\naa\nA\ncat <<B\nbb\nB\n");
    }
```

- [ ] **Step 2: Run — expect PASS**

Run: `cargo test -p huck-syntax --jobs 1 --lib atoms_heredoc_word_redirect_order -- --test-threads 1`
Expected: PASS (Task 1's words-then-redirects order already handles these). If the trailing-redirect case FAILS, the visit order is inverted — STOP and report (the pin and the handled case would be swapped, which contradicts the design; do not silently flip the pin).

- [ ] **Step 3: Determine the pin's actual atom output, then write the pin**

The redirect-before-Word case diverges. First probe the ACTUAL atom output (throwaway): temporarily add a test printing `new_seq("cat <<A $(sh <<B\nbb\nB\n)\naa\nA\n")` and `old_seq(...)` via `eprintln!("{:.400?}", …)`, run with `--nocapture`, read which body lands where, then DELETE the probe. Then write the pin asserting the real (mis-ordered) atom behavior and that it differs from the oracle:

```rust
    #[test]
    fn atoms_heredoc_redirect_before_word_pin() {
        // v260 PINNED live-flip carry-forward: a heredoc REDIRECT appearing in
        // source BEFORE a heredoc-bearing Word on the same command
        // (`cat <<A $(sh <<B)`). Emission order = redirect (A) then arg (B), but
        // the words-then-redirects fill visits the arg first, so the atom
        // MIS-ORDERS the two bodies. Matching the oracle needs per-node source
        // positions the AST does not carry (rejected Scope B). Both paths parse
        // Ok; the ASTs differ. Reconcile before flipping command_atoms live.
        let s = "cat <<A $(sh <<B\nbb\nB\n)\naa\nA\n";
        let n = new_seq(s);
        let o = old_seq(s);
        assert!(n.is_ok(), "expected atom Ok for {s:?}, got {n:?}");
        assert!(o.is_ok(), "expected oracle Ok for {s:?}, got {o:?}");
        assert_ne!(n.unwrap(), o.unwrap(), "expected the KNOWN mis-order divergence for {s:?}");
    }
```

- [ ] **Step 4: Flip the v250 pin to `diff_cmd`**

Replace the entire `atoms_heredoc_in_cmdsub_body_drop_divergence` test (parser.rs:4823 — the one with the `inner_heredoc_body` helper asserting `body == Word([])`) with a resolved-divergence regression guard:

```rust
    #[test]
    fn atoms_heredoc_in_cmdsub_body_drop_divergence() {
        // v250 pinned a KNOWN gap (heredoc inside a `$(…)`/`` `…` `` dropped its
        // body); v260 CF1 RESOLVED it via the fill_word recursion. Now a
        // resolved-divergence regression guard: the atom fills the inner body
        // byte-identically to the oracle.
        diff_cmd("echo $(cat <<X\nhi\nX\n)");
        diff_cmd("echo `cat <<X\nhi\nX\n`");
    }
```

(Keep the test NAME so history/greps for the pin still resolve; the body becomes a `diff_cmd`. Remove the now-unused `inner_heredoc_body` helper.)

- [ ] **Step 5: Run the new tests + full suite + build + gates**

Run: `cargo test -p huck-syntax --jobs 1 --lib atoms_heredoc -- --test-threads 1` → all `atoms_heredoc*` pass.
Run: `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` → all pass (0 failures; if Task 1 `#[ignore]`d the old pin, it is now rewritten and un-ignored).
Run: `cargo build -p huck-syntax` → 0 warnings.
Run: `git diff --stat main -- crates/huck-syntax/src/command.rs` → EMPTY.

- [ ] **Step 6: Commit**

```bash
git add crates/huck-syntax/src/parser.rs
git commit -m "v260 T2: heredoc word/redirect source-order corpus + pin; flip v250 pin

Adds the trailing-redirect interleaving diff_cmd (cat \$(sh <<B) <<A, handled by
words-then-redirects) + outer-redirect regressions; pins the mirror case
(cat <<A \$(sh <<B), redirect-before-Word — mis-orders without AST source
positions); and flips the v250 atoms_heredoc_in_cmdsub_body_drop_divergence pin
to a diff_cmd (CF1 resolved). command.rs EMPTY-diff; command_atoms false.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review

**Spec coverage:** the `fill_word`/`fill_param_modifier` recursion + `fill_command` Word-walking (spec §Architecture) → Task 1; the words-then-redirects visit order, the trailing-redirect handled case, the redirect-before-Word pin, the outer-redirect regressions, and flipping the v250 pin (spec §"Visit order & the pin", §Testing) → Task 2. The fixed corpus (spec §Differential corpus) is split: non-interleaving cases in Task 1, interleaving + pin in Task 2.

**Placeholder scan:** none — verbatim code, exact commands, expected outputs. The one deliberately-probed value (the pin's exact atom AST) is handled by an `assert_ne!` that does not need the literal AST written out.

**Type consistency:** `fill_word(&mut Word, …)`/`fill_word_parts(&mut [WordPart], …)`/`fill_param_modifier(&mut ParamModifier, …)` are defined in Task 1 Step 4 and called from the extended `fill_command` arms in Step 5 and internally. `AssignTarget` is imported in Step 1 and used in Step 5. All AST field names (`WordPart::*`, `ParamModifier::*`, `ArrayLiteralElement { subscript, value }`, `AssignTarget::Indexed { subscript, .. }`, `SubscriptKind::Index`, `exec.inline_assignments`/`program`/`args`/`redirects`, `Assignment { target, value, .. }`) match the definitions in `lexer.rs`/`command.rs`.

**Ordering note:** the visit order is words-then-redirects; the design deliberately pins the redirect-before-Word mirror. Task 2 Step 2 guards against an inverted order (which would swap which case is pinned).
