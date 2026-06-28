# extquote `$'…'`-name gated on double-quote context (M-156) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Decode a `$'…'` ANSI-C quote used as the parameter name inside `${…}` ONLY when the `${…}` is within double quotes (bash `extquote`), matching bash in every context — top-level, `:-`/`:=`/`:+` defaults, and `#`/`%`/`/`/`^`/`,` pattern operands (incl. nesting).

**Architecture:** Add a single-purpose `in_dquote: bool` to `LexerOptions` (already threaded everywhere as `opts`), read ONLY by the extquote-name gate. The gate becomes `!(quoted || opts.in_dquote)`. The five pattern-operand dispatch arms set `in_dquote` via `opts.with_in_dquote(quoted || opts.in_dquote)` — the OR formula composes through nesting. The glob-controlling `enclosing_dquote` argument is untouched, so glob/splitting/single-quote/reconstruction are unchanged. No engine change.

**Tech Stack:** Rust, huck workspace. Tests: `cargo test --workspace`. Bash-compat: `tests/scripts/*_diff_check.sh`.

## Global Constraints

- **Commit trailer** (every commit, verbatim): `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- **No bash source vendoring**: diff harnesses run bash at runtime; never commit bash output.
- **AVOID CPU PEG / HANGS**: wrap every cargo invocation in `timeout` (e.g. `timeout 300 cargo test -p huck-syntax --lib`). Never a bare unbounded `cargo test --workspace`; for the final sweep use a SINGLE `timeout 560 cargo test --workspace > LOG 2>&1` redirected to a log, then grep — running it twice in one shell command exceeds the 10-min tool wall.
- **`in_dquote` is read ONLY by the extquote gate.** It must NOT be passed as the glob-controlling `enclosing_dquote`/`false` argument to any operand scanner — only as a field of the `opts` value.
- **Measured bash 5.2.21 ground truth** (the gate target):
  - unquoted `${$'x1'}`, `y=${$'x1'}`, `${x1}${$'x1'}`, `${x#${$'x1'%$'t'}}`, `${z:-${$'x1'}}` → `bad substitution`, rc 1.
  - quoted `"${$'x1'}"`→`V`, `"${z:-${$'x1'}}"`→`hi`, `"${x#${$'x1'%$'t'}}"`→`tOK`, rc 0.
  - glob-in-pattern unchanged: `x="aXb"; p="a?"; echo "${x#$p}"` → `b` (the `?` stays an active glob).

---

### Task 1: `in_dquote` flag + extquote gate + pattern-arm propagation

**Files:**
- Modify: `crates/huck-syntax/src/lexer.rs` — `LexerOptions` struct (~360); add `with_in_dquote`; the extquote gate (~3117); the five pattern-operand arms (~3701, 3708, 3720, 3733, 3746).
- Modify (mechanical, compile-driven): every `LexerOptions { … }` struct-literal site — `crates/huck-engine/src/continuation.rs:50`, `crates/huck-engine/src/shell.rs:403`, `crates/huck-engine/src/builtins.rs:6345`, `crates/huck-syntax/examples/tokenize_dump.rs:24`, and the in-file test sites in `lexer.rs` (~4296, 4305, 4311, 4317, 4324, 4517, 4532, 4541, 5993, 5994).
- Test: `crates/huck-syntax/src/lexer.rs` `mod tests`; `tests/param_indirect_extquote_integration.rs`.

**Interfaces:**
- Consumes: `recover_bad_subst(chars, parts, quoted, dollar_start)`, `is_valid_param_name(&str)`, `modifier_with_operand`, `scan_optional_braced_operand`, `scan_substitution_operand` (all existing). `quoted: bool` and `opts: LexerOptions` are in scope in both `scan_braced_param_expansion` and `dispatch_braced_modifier`.
- Produces: `LexerOptions { extglob: bool, in_dquote: bool }` + `fn with_in_dquote(self, b: bool) -> Self`.

- [ ] **Step 1: Write the failing integration tests.** In `tests/param_indirect_extquote_integration.rs` (it already has the `run_file(script) -> (stdout, stderr, rc)` harness; `extquote_name_unquoted_is_bad_subst` from v234 is already present — leave it), append:

```rust
#[test]
fn extquote_pattern_quoted_decodes() {
    // Quoted outer -> the nested ${$'x1'%$'t'} in the # pattern decodes.
    let (o, _e, c) = run_file("x=notOK; x1=not; echo \"${x#${$'x1'%$'t'}}\"\n");
    assert_eq!(c, 0);
    assert_eq!(o, "tOK\n");
}

#[test]
fn extquote_pattern_unquoted_is_bad_subst() {
    // Unquoted outer -> the nested extquote name is a runtime bad substitution.
    let (_o, e, c) = run_file("x=notOK; x1=not; echo ${x#${$'x1'%$'t'}}\n");
    assert_eq!(c, 1);
    assert!(e.contains("bad substitution"), "stderr: {e}");
}

#[test]
fn extquote_default_unquoted_is_bad_subst() {
    let (_o, e, c) = run_file("x1=hi; unset z; echo ${z:-${$'x1'}}\n");
    assert_eq!(c, 1);
    assert!(e.contains("bad substitution"), "stderr: {e}");
}

#[test]
fn extquote_default_quoted_decodes() {
    let (o, _e, c) = run_file("x1=hi; unset z; echo \"${z:-${$'x1'}}\"\n");
    assert_eq!(c, 0);
    assert_eq!(o, "hi\n");
}
```

- [ ] **Step 2: Run, observe failures.**

Run: `timeout 200 cargo test --test param_indirect_extquote_integration extquote_pattern_ extquote_default_`
Expected: `extquote_pattern_quoted_decodes` and `extquote_default_quoted_decodes` PASS (already decode unconditionally); `extquote_pattern_unquoted_is_bad_subst` and `extquote_default_unquoted_is_bad_subst` FAIL (huck currently decodes them too).

- [ ] **Step 3: Add the `in_dquote` field + builder.** In `lexer.rs`, replace the `LexerOptions` struct (~360):

```rust
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LexerOptions {
    pub extglob: bool,
    /// True when the `${…}` currently being scanned is inside double quotes.
    /// Read ONLY by the extquote `$'…'`-name gate (M-156); it does NOT affect
    /// glob-literalness, word-splitting, or quoting of operands.
    pub in_dquote: bool,
}

impl LexerOptions {
    /// Returns a copy with `in_dquote` set — used to seed the extquote
    /// double-quote context for a pattern-operand re-parse.
    fn with_in_dquote(self, b: bool) -> Self {
        LexerOptions { in_dquote: b, ..self }
    }
}
```

- [ ] **Step 4: Fix the struct-literal sites the build flags.** Run `timeout 300 cargo build --workspace 2>&1 | grep -E "missing field|error\[" | head`. For EACH flagged `LexerOptions { … }` literal, add `..Default::default()` (the new field defaults to `false`). Sites to update (production first):
  - `crates/huck-engine/src/continuation.rs:50`: `LexerOptions { extglob }` → `LexerOptions { extglob, ..Default::default() }`
  - `crates/huck-engine/src/shell.rs:403`: `LexerOptions { extglob: shell.shopt_options.get("extglob").unwrap_or(false) }` → add `, ..Default::default()`
  - `crates/huck-engine/src/builtins.rs:6345`: `LexerOptions { extglob }` → `LexerOptions { extglob, ..Default::default() }`
  - `crates/huck-syntax/examples/tokenize_dump.rs:24`: `LexerOptions { extglob: true }` → `LexerOptions { extglob: true, ..Default::default() }`
  - In `lexer.rs` tests (~4296, 4305, 4311, 4317, 4324, 4517, 4532, 4541, 5993, 5994): same `..Default::default()` addition to each `LexerOptions { extglob: … }` literal.

  Re-run `timeout 300 cargo build --workspace 2>&1 | tail -3` until clean.

- [ ] **Step 5: Add the extquote gate.** In `scan_braced_param_expansion`, the regular-name path `NameScan::Name { name, decoded }` arm (~3117), add the double-quote-context check BEFORE the existing `is_valid_param_name` check:

```rust
        NameScan::Name { name, decoded } => {
            // extquote: a `$'…'`-decoded name is only valid in double-quote
            // context (bash). `quoted` covers top-level + default operands;
            // `opts.in_dquote` covers pattern operands. Unquoted -> bad subst.
            if decoded && !(quoted || opts.in_dquote) {
                return recover_bad_subst(chars, parts, quoted, dollar_start);
            }
            // A decoded name must be a valid identifier (e.g. `${$'x\ty'}` is
            // invalid -> bad subst). A non-decoded name keeps prior behavior.
            if decoded && !is_valid_param_name(&name) {
                return recover_bad_subst(chars, parts, quoted, dollar_start);
            }
            name
        }
```

(Keep the rest of the arm unchanged; this shows the first lines of the existing arm with the new guard inserted first.)

- [ ] **Step 6: Seed `in_dquote` at the five pattern-operand arms** in `dispatch_braced_modifier`. Change ONLY the `opts` argument passed to each pattern operand scanner to `opts.with_in_dquote(quoted || opts.in_dquote)` — leave the `false` glob argument and everything else as-is:
  - `#` RemovePrefix (~3701): `modifier_with_operand(chars, false, opts, |w| ParamModifier::RemovePrefix { pattern: w, longest })?` → change `opts` to `opts.with_in_dquote(quoted || opts.in_dquote)`.
  - `%` RemoveSuffix (~3708): `modifier_with_operand(chars, false, opts, |w| ParamModifier::RemoveSuffix { pattern: w, longest })?` → change `opts` to `opts.with_in_dquote(quoted || opts.in_dquote)`.
  - `/` Substitute (~3720): `scan_substitution_operand(chars, opts)?` → `scan_substitution_operand(chars, opts.with_in_dquote(quoted || opts.in_dquote))?`.
  - `^` Case-upper (~3733): `scan_optional_braced_operand(chars, opts)?` → `scan_optional_braced_operand(chars, opts.with_in_dquote(quoted || opts.in_dquote))?`.
  - `,` Case-lower (~3746): `scan_optional_braced_operand(chars, opts)?` → `scan_optional_braced_operand(chars, opts.with_in_dquote(quoted || opts.in_dquote))?`.

  Do NOT change the `:-`/`:=`/`:+` default arms (`modifier_with_operand(chars, quoted, …)`) — they already thread `quoted` correctly.

- [ ] **Step 7: Add lexer unit tests** in `lexer.rs` `mod tests` (near the v234 `extquote_name_*` tests):

```rust
    #[test]
    fn extquote_name_unquoted_defers() {
        // Top-level unquoted `${$'x1'}` -> BadSubst (the default tokenize path
        // is unquoted).
        let toks = tokenize(r#"${$'x1'}"#).unwrap();
        let Token::Word(Word(parts)) = &toks[0] else { panic!() };
        assert!(matches!(parts[0], WordPart::ParamExpansion { modifier: ParamModifier::BadSubst { .. }, .. }));
    }

    #[test]
    fn extquote_name_double_quoted_decodes() {
        // Inside `"…"` the name decodes (no BadSubst).
        let toks = tokenize(r#""${$'x1'}""#).unwrap();
        let Token::Word(Word(parts)) = &toks[0] else { panic!() };
        // The single part is the decoded name `x1` (Var or ParamExpansion),
        // NOT a BadSubst.
        let inner = match &parts[0] {
            WordPart::Quoted { parts, .. } => &parts[0],
            other => other,
        };
        let name = match inner {
            WordPart::ParamExpansion { name, modifier, .. } => {
                assert!(!matches!(modifier, ParamModifier::BadSubst { .. }), "should not be BadSubst");
                name
            }
            WordPart::Var { name, .. } => name,
            other => panic!("expected name-bearing part, got {other:?}"),
        };
        assert_eq!(name, "x1");
    }
```

(If `"${$'x1'}"` tokenizes to a top-level `WordPart::Quoted` wrapper, the `match` above unwraps it; if it is a bare `ParamExpansion`/`Var`, the `other => other` arm handles it.)

- [ ] **Step 8: Run all the new + regression tests.**

Run:
```
timeout 300 cargo test -p huck-syntax --lib extquote_
timeout 300 cargo test -p huck-syntax --lib
timeout 200 cargo test --test param_indirect_extquote_integration
```
Expected: all green. CRITICAL regression to confirm: `extquote_nested_pattern_operand` (the v234 `"${x#${$'x1'%$'t'}}"` → `tOK` test) and `extquote_pattern_quoted_decodes` MUST pass (proof the gate doesn't over-fire); `extquote_pattern_unquoted_is_bad_subst` / `extquote_default_unquoted_is_bad_subst` now pass.

- [ ] **Step 9: Diff against bash** for the full ground-truth table + the glob-unchanged guard:

```bash
cargo build -p huck --quiet
H=./target/debug/huck
chk() { printf '%s\n' "$1" > /tmp/g.sh; local b h; b=$(bash /tmp/g.sh 2>&1; echo rc=$?); h=$($H /tmp/g.sh 2>&1; echo rc=$?); [ "$b" = "$h" ] && echo "OK  $1" || { echo "DIFF $1"; echo " bash:$b"; echo " huck:$h"; }; }
chk 'x1=V; echo ${$'"'"'x1'"'"'}'
chk 'x1=V; echo "${$'"'"'x1'"'"'}"'
chk 'x1=V; y=${$'"'"'x1'"'"'}; echo "$y"'
chk 'x=notOK; x1=not; echo ${x#${$'"'"'x1'"'"'%$'"'"'t'"'"'}}'
chk 'x=notOK; x1=not; echo "${x#${$'"'"'x1'"'"'%$'"'"'t'"'"'}}"'
chk 'x1=hi; unset z; echo ${z:-${$'"'"'x1'"'"'}}'
chk 'x1=hi; unset z; echo "${z:-${$'"'"'x1'"'"'}}"'
chk 'x="aXb"; p="a?"; echo "${x#$p}"'
chk 'x="aXbXc"; s="X"; echo "${x//$s/ }"'
```
Expected: every line `OK` (the bad-subst rows match including rc + the `: bad substitution` tail; if ONLY the error-message name-form differs — e.g. `${'x1'}` vs `${$'x1'}` — that is the known M-156 display residual, acceptable; the rc and tail must match).

- [ ] **Step 10: Build workspace warning-clean + commit.**

Run: `timeout 300 cargo build --workspace 2>&1 | tail -3` (clean).

```bash
git add -A
git commit -m "v235: gate extquote \$'…'-name decode on double-quote context (M-156)

bash decodes \$'…'-as-name inside \${…} only within double quotes; huck
decoded unconditionally. Add LexerOptions.in_dquote (read only by the
extquote gate) so the gate becomes !(quoted || opts.in_dquote); the
\#/%///^/, pattern-operand arms seed it via opts.with_in_dquote(quoted ||
opts.in_dquote), which composes through nesting. Glob/splitting unchanged
(enclosing_dquote untouched). No engine change.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Diff harness + full sweep

**Files:**
- Modify: `tests/scripts/param_indirect_extquote_diff_check.sh` (add the quoting-context rows).

**Interfaces:** uses the existing `checkf` / `checkf_badsubst` helpers in that harness.

- [ ] **Step 1: Extend the harness.** Append these cases to `tests/scripts/param_indirect_extquote_diff_check.sh` BEFORE the final `echo`/`exit` summary lines (use `checkf` for value rows, `checkf_badsubst` for bad-subst rows — read the file to confirm the exact helper names; `checkf_badsubst` is the v234 helper that relaxes only the error-message name-form while asserting the tail + rc + continuation):

```bash
# v235 M-156: extquote name gated on double-quote context
checkf          "extquote quoted top"    'x1=V; echo "${$'"'"'x1'"'"'}"'
checkf_badsubst "extquote unquoted top"  'x1=V; echo ${$'"'"'x1'"'"'}; echo after'
checkf          "extquote quoted pat"    'x=notOK; x1=not; echo "${x#${$'"'"'x1'"'"'%$'"'"'t'"'"'}}"'
checkf_badsubst "extquote unquoted pat"  'x=notOK; x1=not; echo ${x#${$'"'"'x1'"'"'%$'"'"'t'"'"'}}; echo after'
checkf          "extquote quoted def"    'x1=hi; unset z; echo "${z:-${$'"'"'x1'"'"'}}"'
checkf_badsubst "extquote unquoted def"  'x1=hi; unset z; echo ${z:-${$'"'"'x1'"'"'}}; echo after'
checkf          "glob-in-pat unchanged"  'x="aXb"; p="a?"; echo "${x#$p}"'
```

(Bump the `Total:` expectation in your head: the harness should now report all-pass.)

- [ ] **Step 2: Build + run the harness.**

Run: `cargo build -p huck --quiet && ./tests/scripts/param_indirect_extquote_diff_check.sh`
Expected: `Fail: 0`. Investigate any FAIL that is not a pure error-message name-form difference as a real bug.

- [ ] **Step 3: Full sweep.**

Run: `timeout 560 cargo test --workspace > /tmp/v235-sweep.log 2>&1; echo "EXIT=$?"; grep -nE "FAILED|panicked|[1-9][0-9]* failed" /tmp/v235-sweep.log | head`
Expected: `EXIT=0`, no failure lines.

- [ ] **Step 4: Commit.**

```bash
git add tests/scripts/param_indirect_extquote_diff_check.sh
git commit -m "v235: extend extquote diff harness with quote-context rows

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Post-implementation (controller, after final review)

- Update `docs/bash-divergences.md`: DELETE the **M-156** entry (closed). The two MINOR display residuals it noted (error-message name-form for `${$'x\ty'}` and `${$"x1"}`) are NOT closed by v235 — re-home them as a small `[deferred]` low entry so they stay tracked.
- Record v235 in `MEMORY.md` + `project_huck_iterations.md`.

## Self-Review

- **Spec coverage:** `in_dquote` field + builder → Task 1 Step 3. Gate `!(quoted || opts.in_dquote)` → Step 5. Pattern-arm propagation `quoted || opts.in_dquote` → Step 6. Construction-site churn → Step 4. Tests (ground-truth table + glob guard + over-fire regression) → Steps 1/7/8/9 + Task 2. Docs → Post-implementation. All spec sections covered.
- **Placeholder scan:** every code step carries the actual code; Step 4 enumerates the concrete sites; the lexer unit test in Step 7 handles both the `Quoted`-wrapper and bare-part shapes.
- **Type consistency:** `LexerOptions { extglob, in_dquote }` + `with_in_dquote(self, bool) -> Self` defined in Step 3 and used identically in Steps 5/6. The gate uses `quoted` (the existing `bool` param) and `opts.in_dquote`. `recover_bad_subst`/`is_valid_param_name` signatures match the v234 call sites.
- **Risk note for the executor:** Step 6 must change ONLY the `opts` argument (not the glob `false`); Step 4's construction-site churn is mechanical but spans three crates — rely on `cargo build --workspace` to surface every site. The `extquote_nested_pattern_operand` regression test is the over-fire guard.
