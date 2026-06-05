# `${var@OP}` Parameter Transforms (Scalar Subset) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement `${var@P}` (prompt), `${var@Q}` (quote), `${var@U}`/`@L`/`@u` (case), `${var@E}` (escape-expand) — replacing today's `invalid parameter-expansion modifier: @`. Clears oh-my-posh's `${prompt@P}` block.

**Architecture:** A new `ParamModifier::Transform { op: TransformOp }` carried by the lexer's `${…@OP}` parse; eval reuses existing helpers (`expand_prompt`, `case_modify`, the ANSI-C decoder, a small `@Q` quoter). Array/attribute forms `@A`/`@K`/`@k`/`@a` are deferred (unknown-operator error).

**Tech Stack:** Rust (binary crate `huck`). Unit `cargo test --bin huck`; integration `cargo test --test <name>`; bash-diff harness under `tests/scripts/`.

---

## File Structure

- `src/lexer.rs` — `TransformOp` enum; `ParamModifier::Transform`; `Some('@')` arm in `dispatch_braced_modifier`; unknown-op error.
- `src/param_expansion.rs` — `Transform { op }` eval arm (6 ops) + a `shell_quote` helper (`@Q`) + a `decode_ansi_c_escapes` helper (`@E`, factored from the `$'…'` path).
- `tests/param_transform_integration.rs`, `tests/scripts/param_transform_diff_check.sh` — NEW.
- `docs/bash-divergences.md`, `README.md` — M-86 `[fixed v96]` + deferrals + changelog + README row.

---

### Task 1: `${var@OP}` transforms (lexer + eval, end-to-end)

**Files:** `src/lexer.rs`, `src/param_expansion.rs` (+ compiler-flagged sites)

- [ ] **Step 1: Write the failing integration test**

Create `tests/param_transform_integration.rs`:

```rust
//! v96: ${var@OP} parameter transforms (M-86, scalar subset).
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

fn run(script: &str) -> (String, i32) {
    let mut child = Command::new(huck_bin())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn huck");
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(), out.status.code().unwrap_or(-1))
}

#[test]
fn transform_upper() { assert_eq!(run("v=hello\necho \"${v@U}\"\n").0, "HELLO\n"); }

#[test]
fn transform_lower() { assert_eq!(run("v=HeLLo\necho \"${v@L}\"\n").0, "hello\n"); }

#[test]
fn transform_upper_first() { assert_eq!(run("v=hello\necho \"${v@u}\"\n").0, "Hello\n"); }

#[test]
fn transform_quote_simple() { assert_eq!(run("v='a b'\necho \"${v@Q}\"\n").0, "'a b'\n"); }

#[test]
fn transform_escape_expand() {
    // v='a\tb' (literal backslash-t) -> @E expands to a<TAB>b
    assert_eq!(run("v='a\\tb'\necho \"${v@E}\"\n").0, "a\tb\n");
}

#[test]
fn transform_prompt_expand_literal() {
    // \n in @P expands to a newline; no env-dependent escapes here.
    assert_eq!(run("v='x\\ny'\necho \"${v@P}\"\n").0, "x\ny\n");
}

#[test]
fn transform_unknown_operator_errors() {
    let (out, rc) = run("v=x\necho \"${v@Z}\"\n");
    assert_ne!(rc, 0, "unknown @-operator should error; out={out:?}");
}
```

Note: before relying on `transform_prompt_expand_literal`/`transform_quote_simple`/`transform_escape_expand`, run each fragment through bash and confirm the expected output byte-for-byte (`printf 'v=...; echo "${v@P}"\n' | bash`); adjust the expected literal to bash's actual output if it differs (bash is the oracle).

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test --test param_transform_integration 2>&1 | tail -20`
Expected: FAIL — `${v@U}` currently errors `invalid parameter-expansion modifier: @`.

- [ ] **Step 3: Add `TransformOp` + `ParamModifier::Transform`**

In `src/lexer.rs`, near `ParamModifier` / `CaseDirection`:
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransformOp {
    PromptExpand, // @P
    Quote,        // @Q
    Upper,        // @U
    Lower,        // @L
    UpperFirst,   // @u
    EscapeExpand, // @E
}
```
Add to `ParamModifier`:
```rust
    Transform { op: TransformOp },
```

- [ ] **Step 4: Add the `Some('@')` arm to `dispatch_braced_modifier`**

In `dispatch_braced_modifier` (`src/lexer.rs:~2190`), add an arm beside the `#`/`%`/`/` arms:
```rust
        Some('@') => {
            let op = match chars.next() {
                Some('P') => TransformOp::PromptExpand,
                Some('Q') => TransformOp::Quote,
                Some('U') => TransformOp::Upper,
                Some('L') => TransformOp::Lower,
                Some('u') => TransformOp::UpperFirst,
                Some('E') => TransformOp::EscapeExpand,
                other => {
                    // @A/@K/@k/@a (deferred) and unknown letters -> bad substitution.
                    return Err(LexError::InvalidBraceModifier(
                        format!("@{}", other.map(|c| c.to_string()).unwrap_or_default())
                    ));
                }
            };
            // After the operator letter, the next char must close the brace.
            match chars.next() {
                Some('}') => {
                    parts.push(WordPart::ParamExpansion {
                        name, modifier: ParamModifier::Transform { op }, quoted, subscript, indirect,
                    });
                    Ok(())
                }
                _ => Err(LexError::UnterminatedBrace),
            }
        }
```
(Confirm the exact `LexError` variants in scope — `InvalidBraceModifier(String)` exists; `UnterminatedBrace` is used elsewhere in this fn. If the unknown-op message should read more like bash's "bad substitution", the `shell::lex_error_message` rendering can be adjusted in Step 4b, but exact text is not required.)

- [ ] **Step 4b (optional): message polish**

If `InvalidBraceModifier("@Z")` renders awkwardly, check `lex_error_message` (src/shell.rs) and ensure the message is sensible (a `bad substitution`-class line). Not required to byte-match bash.

- [ ] **Step 5: Build + fix construction/match sites**

`cargo build --bin huck 2>&1`. The new `ParamModifier::Transform` variant makes the eval `match` in `src/param_expansion.rs` non-exhaustive — that's expected; Step 6 adds the arm. Fix any OTHER non-exhaustive `match ParamModifier` the compiler flags (e.g. in src/expand.rs `expand_array_param`, or anywhere that matches all modifiers) — route `Transform` through the same value-then-apply path or to the scalar `expand_modifier` as appropriate. Add lexer unit tests:
```rust
// in src/lexer.rs tests
#[test]
fn parse_transform_ops() {
    for (src, want) in [("${v@P}", TransformOp::PromptExpand), ("${v@Q}", TransformOp::Quote),
                        ("${v@U}", TransformOp::Upper), ("${v@L}", TransformOp::Lower),
                        ("${v@u}", TransformOp::UpperFirst), ("${v@E}", TransformOp::EscapeExpand)] {
        let parts = match &tokenize(src).unwrap()[0] { Token::Word(Word(p)) => p.clone(), _ => panic!() };
        let WordPart::ParamExpansion { modifier: ParamModifier::Transform { op }, .. } = &parts[0]
            else { panic!("expected Transform for {src}") };
        assert_eq!(*op, want);
    }
    assert!(matches!(tokenize("${v@Z}"), Err(_)));
}
```
(Adapt the token-destructuring to the actual helper pattern used by neighboring lexer tests.)

- [ ] **Step 6: Add the `Transform` eval arm + helpers in `src/param_expansion.rs`**

Mirror the `Case` arm (which uses `lookup_v(shell)` for the value). Add:
```rust
        ParamModifier::Transform { op } => {
            let v = lookup_v(shell);
            let out = match op {
                crate::lexer::TransformOp::Upper =>
                    case_modify(&v, CaseDirection::Upper, true, None, false),
                crate::lexer::TransformOp::Lower =>
                    case_modify(&v, CaseDirection::Lower, true, None, false),
                crate::lexer::TransformOp::UpperFirst =>
                    case_modify(&v, CaseDirection::Upper, false, None, false),
                crate::lexer::TransformOp::Quote => shell_quote(&v),
                crate::lexer::TransformOp::EscapeExpand => decode_ansi_c_escapes(&v),
                crate::lexer::TransformOp::PromptExpand => crate::prompt::expand_prompt(&v, shell),
            };
            ExpansionResult::Value(out)
        }
```

Add the two helpers (in `src/param_expansion.rs` or a shared util):

```rust
/// bash `${v@Q}`: shell-quote so the result re-reads as the same value.
/// Simple strings -> `'…'`; embedded single quotes -> `'\''`; strings with
/// newlines/control chars -> `$'…'` form.
fn shell_quote(v: &str) -> String { /* see Step 6a */ }

/// bash `${v@E}`: expand backslash escapes exactly as `$'…'` does.
fn decode_ansi_c_escapes(v: &str) -> String { /* see Step 6b */ }
```

- [ ] **Step 6a: `shell_quote`**

Match bash's `@Q`. Verify the exact rule against bash FIRST (`printf '%s\n' "${v@Q}"` for `hello`, `a b`, `a'b`, empty, `$'a\nb'`). Implement:
- if `v` contains a newline or ASCII control char → `$'…'` form (escape `\`, `'`, control chars as `\n`/`\t`/`\xHH`).
- else wrap in single quotes, rewriting `'` → `'\''` (reuse `crate::builtins::escape_alias_value` for the inner rewrite, then wrap): `format!("'{}'", escape_alias_value(v))` — BUT confirm bash quotes a simple word like `hello` as `hello` (no quotes) vs `'hello'`. If bash leaves simple words unquoted, only wrap when `v` is empty or contains a char outside the "safe" set (letters/digits/`_`/`-`/`.`/`/`). Encode whatever bash actually does (test `v=hello; ${v@Q}` in bash and match it exactly).

- [ ] **Step 6b: `decode_ansi_c_escapes`**

Reuse the `$'…'` decoder. `src/lexer.rs` has `read_ansi_c_quoted` (1265, scans a `'`-terminated body) and `decode_ansi_c_escape` (1281, one escape). Factor a string-in/string-out helper: walk `v`'s chars; on `\`, call the existing per-escape decoder (make it `pub(crate)` if needed); else copy the char. If the cleanest reuse is to make `read_ansi_c_quoted` operate without requiring the closing `'`, do that minimally. Match bash's `@E` (`a\tb`→`a<TAB>b`; unknown escape `\q`→`\q`). Confirm the exact entry point and `pub(crate)` visibility; report what you factored.

- [ ] **Step 7: Build, run integration + lexer tests, full suite, clippy**

Run: `cargo build --bin huck && cargo test --test param_transform_integration 2>&1 | tail -20` (all pass).
Run: `cargo test --bin huck 2>&1 | tail -5` and `cargo test 2>&1 | grep -E 'test result' | grep -v 'ok\.' | head` (no failures).
Run: `cargo clippy --all-targets 2>&1 | tail -3` (clean).
Manual end-to-end: `printf 'v=hello; echo "${v@U}/${v@Q}/${v@u}"\n' | ./target/debug/huck` matches `| bash`.

- [ ] **Step 8: Commit**

```bash
git add src/lexer.rs src/param_expansion.rs tests/param_transform_integration.rs
git commit -m "feat: \${var@OP} parameter transforms — @P/@Q/@U/@L/@u/@E (M-86 scalar subset)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```
Trailer mandatory/canonical, exactly as shown. (Add any other src file the compiler forced you to touch.)

---

### Task 2: bash-diff harness (21st)

**Files:** `tests/scripts/param_transform_diff_check.sh` (NEW)

- [ ] **Step 1: Create the harness**

Mirror `tests/scripts/dbracket_multiline_diff_check.sh`'s `check` helper. Use ONLY fragments whose output is deterministic and environment-independent (no `\u`/`\h` in `@P` — those vary by user/host). Fragments:
```bash
check "upper"        'v=hello; echo "${v@U}"'
check "lower"        'v=HeLLo; echo "${v@L}"'
check "upper first"  'v=hello; echo "${v@u}"'
check "quote simple" "v='a b'; echo \"\${v@Q}\""
check "quote word"   'v=hello; echo "${v@Q}"'
check "quote squote" "v=\"a'b\"; echo \"\${v@Q}\""
check "quote empty"  'v=; echo "${v@Q}"'
check "escape tab"   'v='"'"'a\tb'"'"'; echo "${v@E}"'
check "escape nl"    'v='"'"'a\nb'"'"'; echo "${v@E}"'
check "prompt nl"    'v='"'"'x\ny'"'"'; echo "${v@P}"'
check "prompt dollar" 'v='"'"'\$'"'"'; echo "${v@P}"'
```
(Adapt the quoting of the `check` args carefully so the FRAGMENT seen by bash/huck is what you intend — test by running the script. The `@P` fragments must avoid env-dependent escapes.)

- [ ] **Step 2: Run the harness**

Run: `cargo build --bin huck && bash tests/scripts/param_transform_diff_check.sh 2>&1 | tail -25`
Expected: every line PASS, `Fail: 0`. If any FAILs, bash is the oracle — investigate. If a `@Q`/`@P` fragment exposes a real parity gap (e.g. bash quotes `hello` differently than huck), fix the helper in Task 1's code (return here after) rather than masking. If a fragment is genuinely environment-dependent, replace it with a deterministic one and note why.

- [ ] **Step 3: Commit**

```bash
git add tests/scripts/param_transform_diff_check.sh
git commit -m "test: bash-diff harness for \${var@OP} transforms (21st)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```
Trailer mandatory/canonical, exactly as shown.

---

### Task 3: Documentation

**Files:** `docs/bash-divergences.md`, `README.md`

- [ ] **Step 1: Read structure**

`grep -n 'M-86\|@OP\|@P\|^## Change log\|Missing features (Tier 2)\|Low-impact\|2026-06-05\|^- \*\*L-[0-9]' docs/bash-divergences.md | head -30`. Read the M-86 deferred entry (added v92) + recent change-log entries + the README table.

- [ ] **Step 2: Flip M-86 to fixed (scalar subset)**

Update M-86 from `[deferred]` to `[fixed v96]`: scalar `${var@OP}` transforms `@P`/`@Q`/`@U`/`@L`/`@u`/`@E` implemented via `ParamModifier::Transform` reusing `expand_prompt`/`case_modify`/ANSI-C decoder/`shell_quote`; the array/attribute forms `@A`/`@K`/`@k`/`@a` remain deferred (note them as a follow-on — next free `M-` number, or a sub-note). Mention it cleared oh-my-posh's `${prompt@P}` block.

- [ ] **Step 3: Low-impact note**

Add an `L-` note (next free number) for the `@P` promptvars-off sub-divergence: huck's `@P` reuses `expand_prompt` which always expands `$VAR`, whereas bash suppresses `$VAR`/command-substitution in `@P` when `shopt -u promptvars`; backslash-escape processing matches. `[intentional]`/low; oh-my-posh's pre-rendered value is unaffected. Also note any `@Q` `$'…'`-form parity limit if the implementer flagged one. Bump the Tier-4 count.

- [ ] **Step 4: Change-log + README row**

Add a `2026-06-05` v96 change-log entry (M-86 scalar transforms; the operators + helper reuse; 21st harness `param_transform_diff_check.sh`; `@A`/`@K`/`@k`/`@a` + promptvars-off deferrals; cleared oh-my-posh `@P` block). Add a v96 README row after v95.

- [ ] **Step 5: Verify + commit**

`grep -n 'v96\|fixed v96\|@OP\|M-86' docs/bash-divergences.md README.md` (confirm, no placeholders).
```bash
git add docs/bash-divergences.md README.md
git commit -m "docs: v96 \${var@OP} transforms fixed (M-86 scalar subset) — changelog, README, deferrals

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```
Trailer mandatory/canonical, exactly as shown.

---

## Self-Review

- **Spec coverage:** §1 lexer + §2 eval → Task 1; testing → Tasks 1/2; M-86 flip + deferrals → Task 3. Covered.
- **Placeholder scan:** none — all new code shown except the two helpers' bodies (Steps 6a/6b), which are specified with their bash-matching rules + the reusable primitives to build on (deliberately, since they require bash-verification of exact output).
- **Type consistency:** `TransformOp { PromptExpand, Quote, Upper, Lower, UpperFirst, EscapeExpand }`; `ParamModifier::Transform { op }`; `case_modify(v, CaseDirection::{Upper,Lower}, all, None, false)`; `expand_prompt(&v, shell)`; helpers `shell_quote(&str)->String`, `decode_ansi_c_escapes(&str)->String`. Matches the read signatures.
- **Edge cases:** unknown operator → error (tested); empty var (`@Q`→`''`); `@P` env-dependent escapes kept out of the harness; `@Q`/`@E` parity verified against bash before encoding expectations.
