# v166: `$()`/backtick-aware `scan_braced_operand` (fix L-52) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `scan_braced_operand` skip `$(…)`/`$((…))`/backtick command substitutions when extracting a `${…}` operand body, so a literal `}` inside one no longer truncates the operand (fix L-52).

**Architecture:** Add a private `consume_paren_cmdsub_verbatim` helper (paren-depth + quote-skip, appends text verbatim) and call it from a new `$(` case in `scan_braced_operand`'s `'$'` arm; add a backtick arm that consumes a `` `…` `` span verbatim. Both intercept the command substitution before its inner chars reach the operand's `}`/brace logic. No signature changes; no other call sites.

**Tech Stack:** Rust (edition 2024). `CharCursor` (lexer.rs:37): `new`/`peek` (returns `Option<&char>`)/`next` (returns `Option<char>`); implements `Iterator<Item = char>`.

**Spec:** `docs/superpowers/specs/2026-06-16-scan-braced-operand-cmdsub-design.md`

**Branch:** `v166-scan-braced-operand-cmdsub`

---

## Background the implementer needs

- `scan_braced_operand` is at `src/lexer.rs:2019`. It extracts a `${…}` operand body up to the matching `}`, with `depth: u32 = 1`, tracking `${`-nesting (in the `'$'` arm) and `'…'`/`"…"` quote spans. It does **not** recognize `$(…)` or backticks — that is L-52. It has four callers (lexer.rs:3053, 3067, 3086, 3226) and existing unit tests (`scan_braced_operand_*`) at lexer.rs:5112+; the test idiom is `CharCursor::new("<operand-body>}")` then `scan_braced_operand(&mut chars)` returns the body up to (not including) the closing `}`.
- The current `'$'` arm (to be replaced) is exactly:
  ```rust
            Some('$') => {
                // Only a `${` (dollar-brace) nests the `${...}` and needs a
                // matching `}`. A BARE `{` (e.g. in a `%%`/`##` glob pattern like
                // `${x%%[<{(]*}`) is a literal character and must NOT raise depth,
                // or the real `}` would close the inner brace and the operand would
                // never terminate.
                body.push('$');
                if chars.peek() == Some(&'{') {
                    chars.next();
                    body.push('{');
                    depth += 1;
                }
            }
  ```
- Verified symptom on the current binary: `huck -c 's=a}b; echo "${s/$(echo a}b)/Z}"'` → `syntax error: unterminated command substitution`; bash prints `Z`. A *quoted* `}` inside the command substitution already works (the existing quote arms skip it).
- `LexError::UnterminatedBrace` is the variant `scan_braced_operand` already returns for an unterminated operand; the new helper reuses it.

---

### Task 1: Add the helper + command-substitution handling + unit tests

A single atomic change to `src/lexer.rs` (helper + the two arms + tests). Unit tests come first (TDD: they fail to compile / fail until the arms exist).

**Files:**
- Modify: `src/lexer.rs` — add `consume_paren_cmdsub_verbatim` before `scan_braced_operand`; edit the `'$'` arm; add a backtick arm; add unit tests in the `#[cfg(test)] mod tests` block.

- [ ] **Step 1: Write the failing unit tests**

Add these inside the `#[cfg(test)] mod tests { … }` block in `src/lexer.rs`, next to the existing `scan_braced_operand_*` tests (around lexer.rs:5112+):

```rust
    #[test]
    fn scan_braced_operand_skips_paren_cmdsub_with_brace() {
        let mut chars = CharCursor::new("$(echo a}b)/Z}");
        assert_eq!(scan_braced_operand(&mut chars).unwrap(), "$(echo a}b)/Z");
    }

    #[test]
    fn scan_braced_operand_skips_backtick_cmdsub_with_brace() {
        let mut chars = CharCursor::new("`echo a}b`/Z}");
        assert_eq!(scan_braced_operand(&mut chars).unwrap(), "`echo a}b`/Z");
    }

    #[test]
    fn scan_braced_operand_skips_nested_cmdsub() {
        let mut chars = CharCursor::new("$(echo $(echo a}b))/Q}");
        assert_eq!(scan_braced_operand(&mut chars).unwrap(), "$(echo $(echo a}b))/Q");
    }

    #[test]
    fn scan_braced_operand_skips_arith_cmdsub() {
        let mut chars = CharCursor::new("$((1+2))}");
        assert_eq!(scan_braced_operand(&mut chars).unwrap(), "$((1+2))");
    }

    #[test]
    fn scan_braced_operand_unterminated_paren_cmdsub_errors() {
        let mut chars = CharCursor::new("$(echo a");
        assert_eq!(
            scan_braced_operand(&mut chars).unwrap_err(),
            LexError::UnterminatedBrace
        );
    }

    #[test]
    fn scan_braced_operand_paren_cmdsub_skips_quoted_paren() {
        // A `)` inside a quoted span within $(…) must not end the substitution.
        let mut chars = CharCursor::new("$(echo \")\")}");
        assert_eq!(scan_braced_operand(&mut chars).unwrap(), "$(echo \")\")");
    }
```

- [ ] **Step 2: Run the tests to confirm they fail**

Run: `cargo test --lib scan_braced_operand 2>&1 | grep -E 'test result|FAILED|panicked' | head`
Expected: the new tests FAIL (e.g. `scan_braced_operand_skips_paren_cmdsub_with_brace` panics on an assertion mismatch — current code truncates at the inner `}`). The pre-existing `scan_braced_operand_*` tests still pass.

- [ ] **Step 3: Add the `consume_paren_cmdsub_verbatim` helper**

Insert this function immediately **before** `fn scan_braced_operand(` in `src/lexer.rs`:

```rust
/// Consumes a `$(…)` command substitution body VERBATIM from `chars`, starting
/// just after the opening `(` (which the caller has already appended to `out`),
/// through the matching `)` (also appended). Any unquoted `(` raises the paren
/// depth and any unquoted `)` lowers it, so nested `$(…)`, `$((…))`, and
/// `$( (…) )` all balance; `'…'`/`"…"` spans are skipped (double-quote honors
/// `\`) so a `)` or `}` inside them does not affect depth. Running out of input
/// yields `Err(LexError::UnterminatedBrace)` (the same error `scan_braced_operand`
/// raises for an unterminated operand). Mirrors `scan_paren_substitution`'s loop
/// but appends text instead of parsing it.
fn consume_paren_cmdsub_verbatim(
    chars: &mut CharCursor<'_>,
    out: &mut String,
) -> Result<(), LexError> {
    let mut depth: u32 = 1;
    loop {
        match chars.next() {
            None => return Err(LexError::UnterminatedBrace),
            Some('(') => {
                depth += 1;
                out.push('(');
            }
            Some(')') => {
                depth -= 1;
                out.push(')');
                if depth == 0 {
                    return Ok(());
                }
            }
            Some('\\') => {
                out.push('\\');
                if let Some(c) = chars.next() {
                    out.push(c);
                }
            }
            Some('\'') => {
                out.push('\'');
                loop {
                    match chars.next() {
                        None => return Err(LexError::UnterminatedBrace),
                        Some('\'') => {
                            out.push('\'');
                            break;
                        }
                        Some(c) => out.push(c),
                    }
                }
            }
            Some('"') => {
                out.push('"');
                loop {
                    match chars.next() {
                        None => return Err(LexError::UnterminatedBrace),
                        Some('"') => {
                            out.push('"');
                            break;
                        }
                        Some('\\') => {
                            out.push('\\');
                            if let Some(c) = chars.next() {
                                out.push(c);
                            }
                        }
                        Some(c) => out.push(c),
                    }
                }
            }
            Some(c) => out.push(c),
        }
    }
}
```

- [ ] **Step 4: Replace the `'$'` arm in `scan_braced_operand` to also handle `$(`**

In `scan_braced_operand`, replace the existing `Some('$') => { … }` arm (shown verbatim in Background) with:

```rust
            Some('$') => {
                // `${` (dollar-brace) nests the operand and needs a matching `}`.
                // A BARE `{` (e.g. in a `%%`/`##` glob pattern like `${x%%[<{(]*}`)
                // is literal and must NOT raise depth. `$(` opens a command
                // substitution whose body — including any `}` — is consumed
                // verbatim so it cannot close the operand (L-52).
                body.push('$');
                match chars.peek() {
                    Some(&'{') => {
                        chars.next();
                        body.push('{');
                        depth += 1;
                    }
                    Some(&'(') => {
                        chars.next();
                        body.push('(');
                        consume_paren_cmdsub_verbatim(chars, &mut body)?;
                    }
                    _ => {}
                }
            }
```

- [ ] **Step 5: Add a backtick arm to `scan_braced_operand`**

Immediately **after** the `Some('\'') => { … }` single-quote arm (and before the `Some('$') => { … }` arm) in `scan_braced_operand`, add:

```rust
            Some('`') => {
                // Backtick command substitution: consume verbatim through the
                // matching unescaped backtick so a `}` inside it does not close
                // the operand (L-52). `\` escapes the next char inside.
                body.push('`');
                loop {
                    match chars.next() {
                        None => return Err(LexError::UnterminatedBrace),
                        Some('`') => {
                            body.push('`');
                            break;
                        }
                        Some('\\') => {
                            body.push('\\');
                            if let Some(c) = chars.next() {
                                body.push(c);
                            }
                        }
                        Some(c) => body.push(c),
                    }
                }
            }
```

- [ ] **Step 6: Run the new unit tests — they pass**

Run: `cargo test --lib scan_braced_operand`
Expected: all `scan_braced_operand_*` tests pass (the 5 pre-existing + 6 new = 11 in this group; `test result: ok`).

- [ ] **Step 7: Full lib unit suite + clippy**

Run: `cargo test --lib 2>&1 | grep -E 'test result: ok|test result: FAILED' | tail -1`
Expected: `test result: ok.` with 0 failed (≈2202 passed).
Run: `cargo clippy --lib --bins --quiet 2>&1 | grep -E 'warning|error' || echo CLEAN`
Expected: `CLEAN`.

- [ ] **Step 8: Build the binary and spot-check the L-52 fix end-to-end**

Run:
```bash
cargo build --quiet && H=./target/debug/huck
for f in 's=a}b; echo "${s/$(echo a}b)/Z}"' 's=xy; echo "${s/`echo a}b`/Z}"' 's=xyz; echo "${s/$(echo $(echo a}b))/Q}"'; do
  b=$(bash -c "$f" 2>&1); h=$($H -c "$f" 2>&1); [ "$b" = "$h" ] && echo "OK   $f -> [$h]" || echo "DIFF $f -> bash[$b] huck[$h]"; done
```
Expected: three `OK` lines (`[Z]`, `[xy]`, `[xyz]`).

- [ ] **Step 9: Commit**

```bash
git add src/lexer.rs
git commit -m "v166: \$()/backtick-aware scan_braced_operand (fix L-52)

Add consume_paren_cmdsub_verbatim + a backtick arm so scan_braced_operand
skips command substitutions when extracting a \${…} operand body. A literal }
inside a \$(…)/\$((…))/backtick no longer truncates the operand (huck
previously threw 'unterminated command substitution'; bash succeeds).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Extend the bash-diff harness + full regression

**Files:**
- Modify: `tests/scripts/param_cmdsub_split_diff_check.sh`

- [ ] **Step 1: Add the L-52 fragments**

In `tests/scripts/param_cmdsub_split_diff_check.sh`, inside the `fragments=( … )` array, add this block immediately before the `# --- plain forms …` comment line:

```bash
    # --- L-52: literal } inside a command substitution in the operand ---
    's=a}b; echo "${s/$(echo a}b)/Z}"'
    's=xy; echo "${s/`echo a}b`/Z}"'
    's=xyz; echo "${s/$(echo $(echo a}b))/Q}"'
    's=ab; echo "${s/$(echo "}")/Z}"'
```

(The harness reports `${#fragments[@]}` so the count updates automatically; no other edit needed.)

- [ ] **Step 2: Run the harness**

Run: `bash tests/scripts/param_cmdsub_split_diff_check.sh`
Expected: `all 20 param-cmdsub-split fragments produce identical output to bash` and exit 0.

- [ ] **Step 3: Run ALL bash-diff harnesses (regression)**

Run: `p=0; f=0; for s in tests/scripts/*_diff_check.sh; do bash "$s" >/dev/null 2>&1 && p=$((p+1)) || { f=$((f+1)); echo "FAIL $s"; }; done; echo "$p passed, $f failed"`
Expected: `92 passed, 0 failed` (no new harness file — the existing one is extended).

- [ ] **Step 4: Run the full test suite**

Run: `cargo test >/tmp/v166.log 2>&1; echo "exit: $?"; grep -cE 'test result: FAILED' /tmp/v166.log`
Expected: `exit: 0` and a FAILED count of `0`.

- [ ] **Step 5: Commit**

```bash
git add tests/scripts/param_cmdsub_split_diff_check.sh
git commit -m "test: L-52 fragments (} inside operand command substitution) for v166

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Resolve the L-52 divergence doc

**Files:**
- Modify: `docs/bash-divergences.md`

- [ ] **Step 1: Delete the L-52 entry**

Remove the entire `- **L-52: ${…} operand body truncates …**` bullet from the Tier-4 list (it sits immediately before the `- **L-32: ...**` bullet). The doc tracks only current divergences; a resolved one is deleted.

- [ ] **Step 2: Decrement the Tier-4 count**

In the summary table, change the Tier-4 row from `42` to `41`:

```markdown
| Low-impact (Tier 4) | 41 | Open edge cases / cosmetic divergences (`[low]`/`[intentional]`/`[deferred]`). |
```

- [ ] **Step 3: Commit**

```bash
git add docs/bash-divergences.md
git commit -m "docs: resolve L-52 (v166 — scan_braced_operand \$()/backtick-aware)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Final review (orchestrator, after all tasks)

- Whole-branch diff: only `src/lexer.rs` (helper + two arm edits + unit tests), the harness, and `docs/bash-divergences.md` changed. Confirm `scan_braced_operand`'s signature and the four call sites are untouched.
- Re-verify the L-52 cases and a couple of the v165 L-10 cases by hand against bash (confirm no regression in the sibling area).
- Merge `v166-scan-braced-operand-cmdsub` to main `--no-ff` after user confirmation (AskUserQuestion); push; delete the local branch.
- Record the iteration in `project_huck_iterations.md` + `MEMORY.md`.

---

## Self-review (plan vs spec)

- **Spec coverage:** `$(` handling in the `'$'` arm (Task 1 Step 4) ✓; `consume_paren_cmdsub_verbatim` helper, paren-depth + quote-skip + `UnterminatedBrace` (Task 1 Step 3) ✓; backtick arm (Task 1 Step 5) ✓; unit tests incl. unterminated + quoted-paren (Task 1 Step 1) ✓; harness extension with `$()`/backtick/nested/quoted-`}` cases (Task 2) ✓; full regression incl. clippy + all harnesses (Task 1 Step 7, Task 2 Steps 3–4) ✓; L-52 deleted + count 42→41 (Task 3) ✓; scope boundary — `split_modifier_operand`, `arith.rs`, process substitution untouched ✓.
- **Placeholder scan:** none — every step shows exact code or a precise deletion + expected output.
- **Type consistency:** `consume_paren_cmdsub_verbatim(&mut CharCursor, &mut String) -> Result<(), LexError>` used consistently; `scan_braced_operand` keeps its `(&mut CharCursor) -> Result<String, LexError>` signature; `chars.peek()` compared with `Some(&'{')`/`Some(&'(')` (it returns `Option<&char>`).
