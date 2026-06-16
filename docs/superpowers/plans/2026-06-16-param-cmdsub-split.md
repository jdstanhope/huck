# v165: `$()`-aware `${…}` operand split (fix L-10) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the `${var/pat/repl}` and `${var:off:len}` operand splitters skip command substitutions (`$(…)`, `$((…))`, backticks) when locating the modifier delimiter, fixing the L-10 syntax-error bug, by collapsing the two near-clone splitters into one shared helper.

**Architecture:** Add one private function `split_modifier_operand(body, delim)` in `src/lexer.rs` — a `CharCursor` state machine that finds the first *top-level* delimiter, skipping single/double quotes, backticks, `$(…)` (nested parens), and `{}` braces, appending everything else verbatim. The existing `split_substitution_body` and `split_substring_body` become thin wrappers over it (return types preserved).

**Tech Stack:** Rust (edition 2024). No new dependencies. `CharCursor` (lexer.rs:37) provides `new`/`peek`/`next`/`offset`/`line` and implements `Iterator<Item = char>` (so `by_ref()` works).

**Spec:** `docs/superpowers/specs/2026-06-16-param-cmdsub-split-design.md`

**Branch:** `v165-param-cmdsub-split`

---

## Background the implementer needs

- The two functions live in `src/lexer.rs`: `split_substitution_body` (currently ~lines 3098–3149, splits on `/`, returns `(String, String)`) and `split_substring_body` (~lines 3172–3227, splits on `:`, returns `(String, Option<String>)`). Each has exactly **one** caller: `split_substitution_body` at lexer.rs:3087, `split_substring_body` at lexer.rs:3159 (inside `scan_substring_operands`). No other call sites.
- Both currently track `{}` brace depth, `'…'`, `"…"`, and `\`-escapes, but **not** `$(…)` / backticks — that is the L-10 bug. Verified symptom on the current binary: `huck -c 's=a-b-c; echo "${s/$(echo a/x)/Z}"'` → `syntax error: unterminated command substitution` (bash prints `a-b-c`).
- Reference for the paren+quote skipping: `scan_paren_substitution` (lexer.rs:2183) counts `(`/`)` depth, raises depth on `$(`, and skips `'…'`/`"…"` spans (double-quote span honors `\` escapes). The new helper mirrors that logic but appends text verbatim instead of parsing.
- `CharCursor::peek` returns `Option<&char>` (compare with `Some(&'(')`); `CharCursor::next` returns `Option<char>`.
- The lexer test module is `#[cfg(test)] mod tests { use super::*; … }` near the bottom of `src/lexer.rs` (the first test fn is around line 3450, e.g. `fn has_redir_fd`). Private fns like `split_modifier_operand` are callable from it.

---

### Task 1: Add the shared `split_modifier_operand` helper + rewrite the two wrappers

This is a single atomic change (the two old function bodies are replaced by wrappers that call the new helper). Unit tests for the new helper come first (TDD: they won't compile until the function exists).

**Files:**
- Modify: `src/lexer.rs` — add `split_modifier_operand`; replace the bodies of `split_substitution_body` and `split_substring_body`; add unit tests in the `#[cfg(test)] mod tests` block.

- [ ] **Step 1: Write the failing unit tests**

Add these tests inside the `#[cfg(test)] mod tests { … }` block in `src/lexer.rs` (it already has `use super::*;`):

```rust
    #[test]
    fn split_modifier_operand_basic_split() {
        assert_eq!(split_modifier_operand("a/b", '/'), ("a".into(), Some("b".into())));
        assert_eq!(split_modifier_operand("a", '/'), ("a".into(), None));
        assert_eq!(split_modifier_operand("2:3", ':'), ("2".into(), Some("3".into())));
        assert_eq!(split_modifier_operand("2", ':'), ("2".into(), None));
    }

    #[test]
    fn split_modifier_operand_skips_command_sub() {
        // A delimiter inside $(...) is NOT the split point (L-10).
        assert_eq!(
            split_modifier_operand("$(echo a/x)/Z", '/'),
            ("$(echo a/x)".into(), Some("Z".into()))
        );
        assert_eq!(
            split_modifier_operand("$(echo 1:2)", ':'),
            ("$(echo 1:2)".into(), None)
        );
        // Nested $( $() ).
        assert_eq!(
            split_modifier_operand("$(echo $(echo a/b))/Q", '/'),
            ("$(echo $(echo a/b))".into(), Some("Q".into()))
        );
        // $(( ... )) arithmetic with a ternary colon inside.
        assert_eq!(
            split_modifier_operand("$((1>0?2:3))", ':'),
            ("$((1>0?2:3))".into(), None)
        );
    }

    #[test]
    fn split_modifier_operand_skips_backtick() {
        assert_eq!(
            split_modifier_operand("`echo a/x`/Z", '/'),
            ("`echo a/x`".into(), Some("Z".into()))
        );
    }

    #[test]
    fn split_modifier_operand_quotes_and_escapes() {
        // A quoted delimiter is kept verbatim and does not split.
        assert_eq!(
            split_modifier_operand("\"a/b\"/x", '/'),
            ("\"a/b\"".into(), Some("x".into()))
        );
        // An escaped delimiter un-escapes to the literal char and does not split.
        assert_eq!(split_modifier_operand("a\\/b/x", '/'), ("a/b".into(), Some("x".into())));
        // \\ un-escapes to a single backslash.
        assert_eq!(split_modifier_operand("a\\\\b", '/'), ("a\\b".into(), None));
    }

    #[test]
    fn split_modifier_operand_brace_nesting() {
        // A delimiter inside ${...} plain nesting is not the split point.
        assert_eq!(split_modifier_operand("${x:-y}", ':'), ("${x:-y}".into(), None));
    }
```

- [ ] **Step 2: Run the tests to confirm they fail (don't compile yet)**

Run: `cargo test --lib split_modifier_operand 2>&1 | grep -E 'error\[|cannot find'`
Expected: a compile error like `cannot find function `split_modifier_operand` in this scope` (the function doesn't exist yet).

- [ ] **Step 3: Add the `split_modifier_operand` helper**

Insert this function in `src/lexer.rs` immediately **before** `fn split_substitution_body` (so the helper and its two callers sit together):

```rust
/// Splits a `${…}` modifier operand body on the FIRST top-level `delim`,
/// returning `(before, Some(after))` if a top-level delimiter was found, or
/// `(before, None)` otherwise. "Top level" excludes single quotes, double
/// quotes, backticks, a `$(…)` command substitution (nested parens — also
/// covers `$((…))` and `$( (…) )`), and `{…}` braces. Skipped spans are
/// appended VERBATIM so the segments re-parse exactly as written. At the top
/// level only, `\delim` un-escapes to `delim` and `\\` to `\`; any other `\x`
/// keeps the backslash. Inside a command substitution escapes are verbatim
/// (they belong to the command), mirroring `scan_paren_substitution`.
fn split_modifier_operand(body: &str, delim: char) -> (String, Option<String>) {
    let mut first = String::new();
    let mut second = String::new();
    let mut delim_seen = false;
    let mut paren_depth: u32 = 0; // > 0 while inside a $( … ) command substitution
    let mut brace_depth: u32 = 0; // { } nesting, tracked only at paren_depth 0
    let mut chars = CharCursor::new(body);
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                let dst = if delim_seen { &mut second } else { &mut first };
                if paren_depth == 0 {
                    match chars.peek().copied() {
                        Some(d) if d == delim => {
                            chars.next();
                            dst.push(delim);
                        }
                        Some('\\') => {
                            chars.next();
                            dst.push('\\');
                        }
                        _ => dst.push('\\'),
                    }
                } else {
                    dst.push('\\');
                    if let Some(nc) = chars.next() {
                        dst.push(nc);
                    }
                }
            }
            '\'' => {
                let dst = if delim_seen { &mut second } else { &mut first };
                dst.push('\'');
                for qc in chars.by_ref() {
                    dst.push(qc);
                    if qc == '\'' {
                        break;
                    }
                }
            }
            '"' => {
                let dst = if delim_seen { &mut second } else { &mut first };
                dst.push('"');
                while let Some(qc) = chars.next() {
                    dst.push(qc);
                    if qc == '\\' {
                        if let Some(nc) = chars.next() {
                            dst.push(nc);
                        }
                    } else if qc == '"' {
                        break;
                    }
                }
            }
            '`' => {
                let dst = if delim_seen { &mut second } else { &mut first };
                dst.push('`');
                while let Some(qc) = chars.next() {
                    dst.push(qc);
                    if qc == '\\' {
                        if let Some(nc) = chars.next() {
                            dst.push(nc);
                        }
                    } else if qc == '`' {
                        break;
                    }
                }
            }
            '$' => {
                let dst = if delim_seen { &mut second } else { &mut first };
                dst.push('$');
                if chars.peek() == Some(&'(') {
                    chars.next();
                    dst.push('(');
                    paren_depth += 1;
                }
            }
            '(' if paren_depth > 0 => {
                paren_depth += 1;
                if delim_seen { second.push('('); } else { first.push('('); }
            }
            ')' if paren_depth > 0 => {
                paren_depth -= 1;
                if delim_seen { second.push(')'); } else { first.push(')'); }
            }
            '{' if paren_depth == 0 => {
                brace_depth += 1;
                if delim_seen { second.push('{'); } else { first.push('{'); }
            }
            '}' if paren_depth == 0 => {
                brace_depth = brace_depth.saturating_sub(1);
                if delim_seen { second.push('}'); } else { first.push('}'); }
            }
            c if c == delim && paren_depth == 0 && brace_depth == 0 && !delim_seen => {
                delim_seen = true;
            }
            _ => {
                if delim_seen { second.push(c); } else { first.push(c); }
            }
        }
    }
    if delim_seen {
        (first, Some(second))
    } else {
        (first, None)
    }
}
```

- [ ] **Step 4: Replace `split_substitution_body`'s body with a wrapper**

Replace the entire existing `fn split_substitution_body(body: &str) -> (String, String) { … }` (the ~50-line function around lexer.rs:3098–3149) with:

```rust
/// Splits a `${var/pat/repl}` operand body into `(pattern, replacement)` on the
/// first top-level `/` (skipping command substitutions / quotes / braces — see
/// `split_modifier_operand`). A missing replacement (`${var/pat}`) yields `""`,
/// matching bash's treatment of `${var/pat}` as `${var/pat/}`.
fn split_substitution_body(body: &str) -> (String, String) {
    let (pattern, replacement) = split_modifier_operand(body, '/');
    (pattern, replacement.unwrap_or_default())
}
```

- [ ] **Step 5: Replace `split_substring_body`'s body with a wrapper**

Replace the entire existing `fn split_substring_body(body: &str) -> (String, Option<String>) { … }` (the ~55-line function around lexer.rs:3172–3227; keep its existing `///` doc comment above it) with:

```rust
fn split_substring_body(body: &str) -> (String, Option<String>) {
    split_modifier_operand(body, ':')
}
```

- [ ] **Step 6: Run the new unit tests — they pass**

Run: `cargo test --lib split_modifier_operand`
Expected: `test result: ok. 5 passed`.

- [ ] **Step 7: Run the full lib unit suite + clippy (no regression)**

Run: `cargo test --lib 2>&1 | tail -3`
Expected: `test result: ok.` with 0 failed (≈2197+ passed).
Run: `cargo clippy --lib --bins --quiet 2>&1 | grep -E 'warning|error' || echo CLEAN`
Expected: `CLEAN`.

- [ ] **Step 8: Commit**

```bash
git add src/lexer.rs
git commit -m "v165: \$()-aware \${…} operand split — unify the two splitters (fix L-10)

Replace split_substitution_body + split_substring_body with one shared
split_modifier_operand(body, delim) that skips \$(…)/\$((…))/backticks (nested
parens) plus the existing quote/brace spans when locating the modifier
delimiter. Fixes L-10: huck previously threw 'unterminated command
substitution' for a / or : inside a command substitution in a \${…} operand.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Bash-diff harness + full regression

**Files:**
- Create: `tests/scripts/param_cmdsub_split_diff_check.sh`

- [ ] **Step 1: Create the harness**

Create `tests/scripts/param_cmdsub_split_diff_check.sh` with this content:

```bash
#!/usr/bin/env bash
# v165: run ${…} substitution/substring operands containing command
# substitutions through bash and huck, asserting byte-identical output.
# Guards the L-10 fix ($()/backtick/$(( )) delimiters skipped) and that the
# plain (no-command-substitution) forms are unchanged.
set -u

HUCK="$(dirname "$0")/../../target/debug/huck"
if [ ! -x "$HUCK" ]; then
    echo "build huck first: cargo build" >&2
    exit 1
fi
if ! command -v bash >/dev/null 2>&1; then
    echo "bash not found on PATH; this differential harness requires bash" >&2
    exit 1
fi

fragments=(
    # --- L-10 cases: delimiter inside $(...) ---
    's=abcdefgh; echo "${s:$(echo 1:2 | cut -d: -f1)}"'
    's=a-b-c; echo "${s/$(echo a/x)/Z}"'
    's=a.b.c; echo "${s/$(echo .)/X}"'
    # --- backtick command substitution ---
    's=a-b; echo "${s/`echo a/x`/Z}"'
    # --- $(( )) arithmetic operands, incl. a ternary colon ---
    's=abcdef; echo "${s:$((1+1)):$((1+2))}"'
    's=abcdef; echo "${s:$((1>0?2:3))}"'
    # --- nested $( $() ) ---
    's=xyz; echo "${s/$(echo $(echo a/b))/Q}"'
    # --- quoted / escaped delimiter in the operand ---
    's=axb; echo "${s/"a/b"/Z}"'
    's=a/b/c; echo "${s/a\/b/Z}"'
    # --- plain forms (must be unchanged by the refactor) ---
    's=abcdefgh; echo "${s:2:3}"'
    's=abcdefgh; echo "${s:2}"'
    's=a.b.c; echo "${s//./X}"'
    's=a.b.c; echo "${s/./X}"'
    's=hello; echo "${s/l}"'
    's=foobar; echo "${s#foo}"; echo "${s%bar}"'
    's=abc; echo "${s:${#s}-1}"'
)

fail=0
for f in "${fragments[@]}"; do
    b_out=$(bash -c "$f" 2>&1)
    h_out=$("$HUCK" -c "$f" 2>&1)
    if [ "$b_out" != "$h_out" ]; then
        echo "DIFF on: $f"
        diff <(printf '%s\n' "$b_out") <(printf '%s\n' "$h_out") || true
        echo "---"
        fail=1
    fi
done

if [ "$fail" -eq 0 ]; then
    echo "all ${#fragments[@]} param-cmdsub-split fragments produce identical output to bash"
fi
exit "$fail"
```

- [ ] **Step 2: Make it executable and run it**

Run: `chmod +x tests/scripts/param_cmdsub_split_diff_check.sh && bash tests/scripts/param_cmdsub_split_diff_check.sh`
Expected: `all 16 param-cmdsub-split fragments produce identical output to bash` and exit 0. (On the pre-Task-1 code this harness would have FAILED on the L-10/backtick/nested fragments — it now passes because Task 1 fixed them.)

- [ ] **Step 3: Run ALL bash-diff harnesses (regression — expect 92 now)**

Run: `p=0; f=0; for s in tests/scripts/*_diff_check.sh; do bash "$s" >/dev/null 2>&1 && p=$((p+1)) || { f=$((f+1)); echo "FAIL $s"; }; done; echo "$p passed, $f failed"`
Expected: `92 passed, 0 failed`.

- [ ] **Step 4: Run the full test suite (unit + integration)**

Run: `cargo test >/tmp/v165.log 2>&1; echo "exit: $?"; grep -cE 'test result: FAILED' /tmp/v165.log`
Expected: `exit: 0` and a FAILED count of `0`.

- [ ] **Step 5: Commit**

```bash
git add tests/scripts/param_cmdsub_split_diff_check.sh
git commit -m "test: bash-diff harness for \$()-aware \${…} operand split (v165)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Close out the divergence docs (resolve L-10, log L-52)

**Files:**
- Modify: `docs/bash-divergences.md`

- [ ] **Step 1: Delete the resolved L-10 entry**

Remove the entire `### L-10: …` block from `docs/bash-divergences.md` (the heading at ~line 225 through its `- **Workaround**:` line, inclusive, plus the trailing blank line). The doc tracks only *current* divergences; a resolved one is deleted, not annotated.

- [ ] **Step 2: Add the L-52 sibling entry**

Add this as a new bullet in the Tier-4 list, immediately after the L-51 entry (the `- **L-51: a pipeline gets its own process group …**` bullet):

```markdown
- **L-52: `${…}` operand body truncates when a command substitution inside it contains a literal `}`** — `[deferred]`, low (found 2026-06-16 alongside the L-10 fix). `scan_braced_operand` (the function that extracts a `${…}` operand body up to the matching `}`) tracks `{}` depth and quoted spans but not `$(…)`, so a command substitution whose body contains a literal `}` ends the operand early: `s=a}b; echo "${s/$(echo a}b)/Z}"` → bash `Z`, huck `syntax error: unterminated command substitution`. This is a sibling of the resolved L-10 (a different scanner — body extraction vs the operand split — and a markedly rarer trigger: a literal `}` inside a command substitution inside a `${…}` operand). The common L-10 cases (delimiter inside `$(…)`/backticks/`$((…))`/nesting, no `}` inside the command substitution) are fixed in v165. Fix: give `scan_braced_operand` the same `$(…)` paren-skip the v165 split helper uses, so a `}` inside a command substitution does not close the operand.
```

- [ ] **Step 3: Keep the Tier-4 summary count consistent**

The summary table line `| Low-impact (Tier 4) | 42 | …` stays **42** (L-10 removed, L-52 added — net zero). Verify it still reads `42`; no edit needed unless the number drifted.

- [ ] **Step 4: Commit**

```bash
git add docs/bash-divergences.md
git commit -m "docs: resolve L-10 (v165), log L-52 sibling (scan_braced_operand \$()-with-})

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Final review (orchestrator, after all tasks)

- Whole-branch diff review: only `src/lexer.rs` (helper + two wrappers + unit tests), the new harness, and `docs/bash-divergences.md` changed. Confirm the two wrapper functions return the original types (`(String, String)` and `(String, Option<String>)`) and that no other call sites were touched.
- Re-verify the L-10 cases by hand against bash (`${s/$(echo a/x)/Z}`, `${s:$(echo 1:2|cut -d: -f1)}`, the backtick and nested forms) and spot-check a few plain forms for unchanged output.
- Merge `v165-param-cmdsub-split` to main `--no-ff` after user confirmation (AskUserQuestion); push; delete the local branch.
- Record the iteration in `project_huck_iterations.md` + `MEMORY.md`; mark improvement #3 of the architecture-review sequence done; note L-52 as the remaining sibling.

---

## Self-review (plan vs spec)

- **Spec coverage:** shared `split_modifier_operand` helper with the specified skip set + top-level-only escape handling (Task 1 Step 3) ✓; two wrappers, return types preserved, `None`→`""` for substitution (Task 1 Steps 4–5) ✓; unit tests for the state machine (Task 1 Step 1) ✓; bash-diff harness with L-10 + backtick + `$((…))` + nested + quoted/escaped + plain forms (Task 2) ✓; full regression incl. clippy + all harnesses (Task 1 Step 7, Task 2 Steps 3–4) ✓; L-52 logged, L-10 deleted, count consistent (Task 3) ✓; out-of-scope `scan_braced_operand`/`arith.rs` untouched ✓.
- **Placeholder scan:** none — every code/step shows exact content; deletions identify the exact block; commands have expected output.
- **Type consistency:** `split_modifier_operand(&str, char) -> (String, Option<String>)` used consistently; wrappers return `(String, String)` and `(String, Option<String>)` exactly as the existing callers (`split_substitution_body` at lexer.rs:3087, `split_substring_body` at lexer.rs:3159) expect.
