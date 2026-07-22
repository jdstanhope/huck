# v321 — RHS nested-quote backslash strip Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a backslash inside a nested `"…"` span of a double-quoted
value-family parameter expansion strip to match bash (`\p` → `p`), flipping
the bash-suite `rhs-exp` category FAIL → PASS.

**Architecture:** One gated match arm in the huck-syntax lexer's
`scan_step_param_operand` (the `Mode::ParamWordOperand` scanner), plus a
bash-diff harness and lexer unit tests. No engine or parser changes.

**Tech Stack:** Rust; huck-syntax crate; bash-diff `*_diff_check.sh` harness.

Spec: `docs/superpowers/specs/2026-07-22-rhs-exp-nested-quote-backslash-design.md`
Issue: [#253](https://github.com/jdstanhope/huck/issues/253)

## Global Constraints

- bash 5.2.21 is the fidelity target. The rule (verified): inside a nested
  `"…"` span of a value-family word operand (`:-`/`:=`/`:?`/`:+` and
  non-colon `-`/`+`), a backslash before a **non-special** char is DROPPED
  when the enclosing `${…}` is double-quoted (`\p`→`p`), and KEPT otherwise
  (`\p`→`\p`). Backslash before `$` `` ` `` `"` `\` already matches bash in
  all cases and must stay unchanged.
- Change is confined to the `_ =>` (non-special) arm of the `in_dquote`
  backslash handling in `scan_step_param_operand`. Do NOT touch the special
  arm, the `in_dquote == false` branch, pattern operands (`#`/`%`/`/`),
  substring offsets, or single-quote handling.
- Commit trailer on every commit:
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- Run `cargo fmt --all` before each commit.
- Per repo memory: build the binary with `cargo build -p huck`; run tests
  per-crate single-threaded (`cargo test -p huck-syntax --lib --jobs 1 --
  --test-threads 1`); guard any bash-diff sweep with `ulimit -v 1500000` +
  `timeout`. NEVER run `cargo test --workspace` (OOMs this box). NEVER copy
  bash's GPL `rhs-exp.tests` text into the repo — author synthetic fragments.

---

### Task 1: Gate the nested-quote backslash arm on `enclosing_dquote`

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` (`scan_step_param_operand`, the
  `in_dquote == true` branch's `Some('\\') =>` block, currently ~lines
  2706–2735)
- Test: `crates/huck-syntax/src/lexer.rs` (existing `#[cfg(test)]` module)

**Interfaces:**
- Consumes: `scan_step_param_operand(&mut self, sep, end, in_dquote: bool,
  enclosing_dquote: bool)` — both flags are already parameters; no signature
  change.
- Produces: no new public API. The lexer now emits a `TokenKind::Lit { text,
  quoted: true }` WITHOUT the leading backslash for a non-special `\X` inside
  a nested `"…"` span when `enclosing_dquote == true`.

- [ ] **Step 1: Write the failing test**

Add to the huck-syntax lexer test module. Use whatever token-lexing helper
the module already uses (e.g. a `fn lex(src: &str) -> Vec<TokenKind>` /
`lex_all` helper — match the existing test style; if a helper exists that
returns tokens for a source string, reuse it). The assertion is that the
nested `"\p"` inside a double-quoted `${v:+…}` yields a `p` Literal with no
backslash, while the unquoted-outer counterpart keeps `\p`.

```rust
#[test]
fn param_value_nested_dquote_backslash_strips_when_enclosing_dquoted() {
    // Outer ${…} is double-quoted; nested "\p" → backslash dropped (`p`).
    let toks = lex_tokens(r#""${v:+a="\p"}""#);
    let lits: String = literal_texts(&toks); // concat of all Lit texts
    assert!(
        lits.contains("a=p") && !lits.contains(r"a=\p"),
        "expected nested \\p to strip to p, got: {lits:?}"
    );
}

#[test]
fn param_value_nested_dquote_backslash_kept_when_enclosing_unquoted() {
    // Outer ${…} is UNQUOTED; nested "\p" → backslash kept (`\p`).
    let toks = lex_tokens(r#"${v:+a="\p"}"#);
    let lits: String = literal_texts(&toks);
    assert!(
        lits.contains(r"a=\p"),
        "expected nested \\p to stay \\p when outer unquoted, got: {lits:?}"
    );
}

#[test]
fn param_value_nested_dquote_special_backslash_unchanged() {
    // `\$` in the nested span → single `$` under BOTH contexts (guard).
    let q = literal_texts(&lex_tokens(r#""${v:+x="\$"}""#));
    let u = literal_texts(&lex_tokens(r#"${v:+x="\$"}"#));
    assert!(q.contains("x=$") && !q.contains(r"x=\$"), "quoted: {q:?}");
    assert!(u.contains("x=$") && !u.contains(r"x=\$"), "unquoted: {u:?}");
}
```

> Implementer note: `lex_tokens` / `literal_texts` are placeholders for the
> module's existing helpers. If the module lexes differently (e.g. asserts on
> `Token` structs or on parsed `WordPart`s), express the same three
> assertions in that style: (1) double-quoted outer + nested `"\p"` → the
> operand contributes a `p` literal, no backslash; (2) unquoted outer → `\p`
> kept; (3) `\$` → `$` under both. Do not invent a new harness if one exists.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p huck-syntax --lib --jobs 1 -- --test-threads 1 param_value_nested_dquote`
Expected: the `strips_when_enclosing_dquoted` and `special_backslash`
(quoted half) tests FAIL — huck currently keeps `\p` and… (the `\$` case may
already pass; the `strip` test is the key red).

- [ ] **Step 3: Implement the gate**

Replace the `_ =>` arm of the `in_dquote` backslash block. Current code:

```rust
_ => {
    let mut s = String::from("\\");
    if let Some(ch) = self.cursor.next() {
        s.push(ch);
    }
    self.history.push(Token::new(
        TokenKind::Lit {
            text: s,
            quoted: true,
        },
        Span::new(off, l, c),
    ));
}
```

New code:

```rust
_ => {
    // v321 (#253): when the enclosing `${…}` is itself double-quoted,
    // bash does a second de-quoting pass over a nested `"…"` span in a
    // value-family word — a backslash before a NON-special char is
    // DROPPED (`\p` → `p`). Outside that context, standard double-quote
    // rules keep the backslash (`\p` → `\p`). The special arm above
    // (`$` `` ` `` `"` `\`) already drops the backslash under both rules,
    // so only this arm is context-dependent.
    let next = self.cursor.next();
    let mut s = String::new();
    if !enclosing_dquote {
        s.push('\\');
    }
    if let Some(ch) = next {
        s.push(ch);
    }
    self.history.push(Token::new(
        TokenKind::Lit {
            text: s,
            quoted: true,
        },
        Span::new(off, l, c),
    ));
}
```

Leave the `Some(e @ ('$' | '`' | '"' | '\\')) =>` special arm and every other
part of the function untouched.

> Note on the degenerate `\`-at-EOF case: if `next` is `None` (a trailing
> backslash), this is already an unterminated-quote error caught by the
> branch's top-level `None => Err(UnterminatedQuote)` on the following scan
> step; the emitted token is irrelevant. No special handling needed.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p huck-syntax --lib --jobs 1 -- --test-threads 1 param_value_nested_dquote`
Expected: all three PASS.

- [ ] **Step 5: Run the full huck-syntax lib suite (regression guard)**

Run: `cargo test -p huck-syntax --lib --jobs 1 -- --test-threads 1`
Expected: all green (no existing param-expansion/quoting test regresses).

- [ ] **Step 6: `cargo fmt --all` and commit**

```bash
cargo fmt --all
git add crates/huck-syntax/src/lexer.rs
git commit -m "$(cat <<'EOF'
v321: strip nested-quote backslash in double-quoted value expansion (#253)

Inside a nested "…" span of a value-family word operand (:-/:=/:?/:+ and
non-colon -/+), bash drops a backslash before a non-special char when the
enclosing ${…} is double-quoted (\p → p); huck kept it. Gate the non-special
arm of scan_step_param_operand's in_dquote branch on enclosing_dquote.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Bash-diff harness + baseline-doc PASS-count update

**Files:**
- Create: `tests/scripts/rhs_exp_nested_quote_diff_check.sh`
- Modify: `docs/bash-test-suite-baseline.md` (PASS 17 → 18; move `rhs-exp`
  from FAIL near-miss to PASS)

**Interfaces:**
- Consumes: the Task 1 lexer behavior (built `huck` binary).
- Produces: an auto-discovered `*_diff_check.sh` (picked up by
  `tests/scripts/run_diff_checks.sh`'s `tests/scripts/*_diff_check.sh` glob).

- [ ] **Step 1: Read an existing harness for the exact pattern**

Read one current `tests/scripts/*_diff_check.sh` (e.g.
`tests/scripts/param_*_diff_check.sh` or any small one) to copy its exact
structure: how it locates the huck binary, the bash-vs-huck compare loop, the
byte-identical assertion, and the pass/fail exit convention. Match it exactly.

- [ ] **Step 2: Write the harness**

Author `tests/scripts/rhs_exp_nested_quote_diff_check.sh` in that same shape.
Fragments (synthetic — NOT bash's GPL test file), each run through both
shells and compared byte-for-byte. Cover, using `printf '<%s>\n'` so the
result is visible verbatim:

```sh
# Fixtures (each a full one-liner fed to both bash and huck):
v=X; printf '<%s>\n' "${v:+a="\p"b}"      # dq outer, nested "\p"  -> <a=pb>
v=X; printf '<%s>\n' "${v:+a="\'"b}"      # dq outer, nested "\'"  -> <a='b>
v=X; printf '<%s>\n' "${v:+a="\$"b}"      # dq outer, nested "\$"  -> <a=$b>  (special, guard)
v=X; printf '<%s>\n' "${v:+a="\\"b}"      # dq outer, nested "\\"  -> <a=\b>  (special, guard)
v=X; printf '<%s>\n' "${v:+a=\pb}"        # dq outer, BARE \p      -> <a=\pb> (guard: not nested)
v=X; printf '<%s>\n' ${v:+a="\p"b}        # UNQUOTED outer, "\p"   -> <a=\pb> (guard)
unset v; printf '<%s>\n' "${v:-a="\p"b}"  # :- family, dq outer    -> <a=pb>
unset v; printf '<%s>\n' "${v:=a="\p"b}"  # := family, dq outer    -> <a=pb>
printf '<%s>\n' "A\pB"                     # plain dq, no ${…}       -> <A\pB> (scope boundary)
```

The harness must assert huck's stdout equals bash's stdout for every
fragment. Do not hardcode the expected strings if the existing harnesses
compare live `bash` vs `huck` output — follow that live-compare convention so
the fixtures document intent while bash is the oracle.

- [ ] **Step 3: Build both binaries and run the new harness**

```bash
cargo build -p huck            # debug
cargo build --release -p huck  # release (run_diff_checks builds/uses both)
ulimit -v 1500000
timeout 120 bash tests/scripts/rhs_exp_nested_quote_diff_check.sh
```
Expected: PASS (exit 0, no diff reported).

- [ ] **Step 4: Run the full bash-diff sweep (regression guard)**

```bash
ulimit -v 1500000
timeout 300 bash tests/scripts/run_diff_checks.sh
```
Expected: all checks green, including the new one.

- [ ] **Step 5: Confirm the `rhs-exp` bash-suite category flips to PASS**

With bash 5.2.21 source available (`BASH_SOURCE_DIR`):
```bash
ulimit -v 2000000
HUCK_BASH_TEST_CATEGORY=rhs-exp HUCK_TEST_TIMEOUT=60 \
  BASH_SOURCE_DIR=<path-to-bash-5.2.21> \
  timeout 150 bash tests/bash-test-suite/runner.sh
```
Expected: summary shows `rhs-exp | PASS`, empty diff.

- [ ] **Step 6: Update the baseline doc**

In `docs/bash-test-suite-baseline.md`: bump the PASS count 17 → 18, move
`rhs-exp` out of the FAIL near-miss ranking into the PASS set, and adjust any
"near-miss" prose that named `rhs-exp` (the next near-misses become
`dbg-support2` and `nquote`). Do not paste any bash test output.

- [ ] **Step 7: `cargo fmt --all` (no-op for shell) and commit**

```bash
git add tests/scripts/rhs_exp_nested_quote_diff_check.sh docs/bash-test-suite-baseline.md
git commit -m "$(cat <<'EOF'
v321: rhs-exp nested-quote backslash diff harness + baseline flip (#253)

Add rhs_exp_nested_quote_diff_check.sh (synthetic fragments, bash oracle) and
record the rhs-exp category flip FAIL→PASS (bash-suite PASS 17→18).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-Review

- **Spec coverage:** Task 1 = the single gated arm (spec §Design/Single
  site). Task 2 = the harness + suite flip + baseline update (spec §Testing
  1/3/4 + §Documentation). Lexer unit tests (spec §Testing 2) are in Task 1.
- **Placeholders:** `lex_tokens`/`literal_texts` are explicitly flagged as
  stand-ins for the module's existing helpers with a fallback instruction —
  acceptable since the test-helper API is module-local and must be matched to
  what exists.
- **Type consistency:** no signature changes; `enclosing_dquote` is already a
  parameter of `scan_step_param_operand`.
- **Scope:** value family only; pattern operands / substring / single quotes
  untouched — stated in Global Constraints and both tasks.
