# v183: comments in delimiter close-finders (shared mechanism) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Recognise `#` comments consistently in the delimiter close-finders that bypass the main tokenizer — the array literal (`scan_array_literal`) and the `$()` close-finder (`scan_cmdsub_body`) — via ONE shared `skip_line_comment` helper, so a delimiter character inside a comment no longer closes its construct. Clears the parse-sweep "expected a command" array cluster and a `$()` sibling.

**Architecture:** A small shared helper `skip_line_comment(chars)` (consume to end-of-line) replaces the inline comment-skip in the main tokenizer. The array literal folds comment-skipping into its inter-element separator skipper (no special-case in the loop body). `scan_cmdsub_body` gains a word-boundary flag and consumes word-start comments verbatim into the body (re-tokenized + stripped later), so a `)` inside a comment isn't counted. Two distinct bugs found during design — the subshell leading-comment parser bug and the `[i]=#value` drop — are logged as deferred divergences, not fixed.

**Tech Stack:** Rust (huck shell). Verification via `cargo test`, `tests/scripts/*_diff_check.sh`, and `tools/parse_sweep.sh`.

**Spec:** `docs/superpowers/specs/2026-06-18-comments-in-delimiter-scanners-design.md`

**Branch:** `v183-comments-delimiter-scanners` (fresh from `main`; the earlier `v183-array-literal-comments` branch's localized commit is discarded).

---

## Confirmed bash behavior (the contract)

| Fragment | bash result |
|---|---|
| `a=(\n 1\n # note (paren) and ) here\n 2\n)` → `"${a[@]}"` | `1 2` |
| `a=( # lead\n x y\n)` → `"${a[@]}"` | `x y` |
| `a=(x#y a#b)` → `"${a[@]}"` | `x#y a#b` |
| `set -- a b c; a=($#)` → `"${a[@]}"` | `3` |
| `echo "[$(echo hi  # c with ) paren\n)]"` | `[hi]` |
| `echo "[$(# c )\necho yo)]"` | `[yo]` |
| `echo "[$(echo a;# c ) z\necho b)]"` | `[a`⏎`b]` |
| `echo "[$(echo a#b)]"` | `[a#b]` |
| `echo "[$( (echo hi)  # ) c\n)]"` | `[hi]` |

---

### Task 1: Shared `skip_line_comment` helper + array-literal separator fold

**Files:**
- Modify: `src/lexer.rs` — add `skip_line_comment`; main-loop `'#'` arm (`:606`); rename+extend `skip_array_literal_whitespace` (`:2790`/`:2806`) and its call in `scan_array_literal` (`:2751`)
- Test: `src/lexer.rs` (`mod tests`)

**Context:** The main tokenizer (`:606`, `'#' if !has_token`) is the only comment site today. `scan_array_literal`'s loop calls `skip_array_literal_whitespace` then matches `)` / EOF / element. `skip_array_literal_whitespace` is referenced ONLY in `src/lexer.rs` (the def, the call, one comment) — no test references it. Test helpers `parse_assignments` / `array_lit` are in `mod tests`.

- [ ] **Step 1: Create a fresh branch from main**

```bash
cd /home/john/projects/shuck
git checkout main
git branch -D v183-array-literal-comments 2>/dev/null || true
git checkout -b v183-comments-delimiter-scanners
```

- [ ] **Step 2: Add the `skip_line_comment` helper**

In `src/lexer.rs`, add (a natural home is just above `skip_array_literal_whitespace`, ~`:2789`):

```rust
/// Consumes a `#` line comment's body up to (but NOT including) the terminating
/// newline; the caller's loop handles the newline. The opening `#` must already
/// be confirmed as a comment-start (word boundary) by the caller.
fn skip_line_comment(chars: &mut CharCursor<'_>) {
    while let Some(&c) = chars.peek() {
        if c == '\n' {
            break;
        }
        chars.next();
    }
}
```

- [ ] **Step 3: Dedup the main-tokenizer comment skip**

In `src/lexer.rs`, the main loop arm (`:606`) currently reads:

```rust
            '#' if !has_token => {
                // POSIX: an unquoted `#` that begins a word starts a comment
                // to end-of-line. `#` mid-word (has_token=true) falls through
                // to the catch-all as a literal char.
                while let Some(&ch) = chars.peek() {
                    if ch == '\n' { break; }
                    chars.next();
                }
                // The trailing newline (if any) is handled by the outer loop.
            }
```

Replace its body with the helper call:

```rust
            '#' if !has_token => {
                // POSIX: an unquoted `#` that begins a word starts a comment to
                // end-of-line. `#` mid-word (has_token) falls through as literal.
                skip_line_comment(&mut chars);
            }
```

- [ ] **Step 4: Rename + extend the array separator skipper**

In `src/lexer.rs`, `skip_array_literal_whitespace` currently reads:

```rust
/// Skips whitespace AND newlines inside an array literal.
fn skip_array_literal_whitespace(
    chars: &mut CharCursor<'_>,
) {
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
        } else {
            break;
        }
    }
}
```

Replace it with (renamed, comment-aware):

```rust
/// Skips inter-element separators inside an array literal: whitespace, newlines,
/// and `#` comments. The post-skip position is always an element boundary (after
/// `(` or inter-element whitespace), so a `#` here is unambiguously a comment —
/// its body (incl. any `)`) must NOT be read as elements or close the literal.
fn skip_array_literal_separators(
    chars: &mut CharCursor<'_>,
) {
    loop {
        while let Some(&c) = chars.peek() {
            if c.is_whitespace() {
                chars.next();
            } else {
                break;
            }
        }
        if chars.peek() == Some(&'#') {
            skip_line_comment(chars);
        } else {
            break;
        }
    }
}
```

- [ ] **Step 5: Update the call site in `scan_array_literal`**

In `src/lexer.rs`, `scan_array_literal`'s loop head reads:

```rust
    loop {
        // Skip whitespace AND newlines (array literals span lines in bash).
        skip_array_literal_whitespace(chars);
        match chars.peek() {
```

Replace those two lines:

```rust
    loop {
        // Skip inter-element separators: whitespace, newlines, and comments.
        skip_array_literal_separators(chars);
        match chars.peek() {
```

- [ ] **Step 6: Build**

Run: `cargo build 2>&1 | tail -5`
Expected: compiles clean (the rename has no other references).

- [ ] **Step 7: Manual array behavior check**

```bash
B=target/debug/huck
chk() { printf '%b\n' "$2" > /tmp/v183.sh
  printf '%-22s huck=<%s> bash=<%s>\n' "$1" \
    "$($B /tmp/v183.sh 2>&1 | tr '\n' '|')" "$(bash --norc /tmp/v183.sh 2>&1 | tr '\n' '|')"; }
chk 'comment paren'   'a=(\n  1\n  # note (paren) and ) here\n  2\n)\necho "${a[@]}"'
chk 'comment after (' 'a=(  # lead\n  x y\n)\necho "${a[@]}"'
chk 'midword hash'    'a=(x#y a#b)\necho "${a[@]}"'
chk 'dollar-hash'     'set -- a b c; a=($#)\necho "${a[@]}"'
```
Expected (huck == bash): `1 2`, `x y`, `x#y a#b`, `3`.

- [ ] **Step 8: Add unit tests (helper + array)**

In `src/lexer.rs` `mod tests`, add a `skip_line_comment` test (near the lexer helper tests) and the two array tests (near the `array_literal_*` tests):

```rust
    #[test]
    fn skip_line_comment_stops_before_newline() {
        // The opening `#` is the caller's; this runs the body to (not incl.) \n.
        let mut chars = CharCursor::new("a comment ) here\nNEXT");
        skip_line_comment(&mut chars);
        assert_eq!(chars.next(), Some('\n'));
        assert_eq!(chars.next(), Some('N'));
    }
```

```rust
    #[test]
    fn array_literal_skips_comment_with_paren() {
        // v183: a `#` comment between elements (incl. one whose text contains
        // `)`) is skipped — the `)` must NOT close the array early.
        let assigns = parse_assignments("a=(\n# c )\n1\n)");
        let els = array_lit(&assigns[0].value);
        assert_eq!(els.len(), 1);
        assert!(els[0].subscript.is_none());
    }

    #[test]
    fn array_literal_midword_hash_is_literal() {
        // v183 regression: a `#` MID-word (`x#y`) is NOT a comment.
        let assigns = parse_assignments("a=(x#y z)");
        let els = array_lit(&assigns[0].value);
        assert_eq!(els.len(), 2);
    }
```

- [ ] **Step 9: Run the new unit tests + clippy**

Run: `cargo test --lib skip_line_comment_stops_before_newline 2>&1 | tail -6 && cargo test --lib array_literal_ 2>&1 | tail -8 && cargo clippy 2>&1 | tail -5`
Expected: all pass; clippy clean.

- [ ] **Step 10: Commit**

```bash
git add src/lexer.rs
git commit -m "$(cat <<'EOF'
v183: shared skip_line_comment helper; array literals skip # comments

Extracted skip_line_comment (consume to end-of-line) and used it in the main
tokenizer (dedup). Renamed skip_array_literal_whitespace ->
skip_array_literal_separators, folding `#`-comment skipping into the
inter-element separator skip so a `)` inside a comment no longer closes the
array. Clears the array-literal-comment parse cluster (ioam6, sysctl,
param_cmdsub_split harness).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

## Report back
STATUS, commit SHA, the Step 7 table, Step 9 result, any concerns.

---

### Task 2: `scan_cmdsub_body` — word-boundary-aware comment skip

**Files:**
- Modify: `src/lexer.rs:2081` (`scan_cmdsub_body`)
- Test: `src/lexer.rs` (`mod tests`, near `scan_cmdsub_body_*` tests ~`:5371`)

**Context:** `scan_cmdsub_body(chars, out, unterminated)` collects a `$(…)` body verbatim into `out` while finding the matching `)` (paren-depth + quote/escape aware). It is the shared close-finder for `scan_paren_substitution`, `consume_paren_cmdsub_verbatim`, and the arith disambiguation. The body is re-tokenized later (`parse_substitution_body`), which strips comments — so a word-start `#` comment only needs to be consumed VERBATIM into `out` so its `)` is not counted. bash's word-boundary rule: `#` is a comment when preceded by start, whitespace, or a metacharacter (`( ) ; & | < >`). Existing tests (`scan_cmdsub_body_basic_consumes_through_close_paren`, `_balances_nested_and_arith`, `_skips_quoted_paren`, `_unterminated_uses_passed_error`) must stay green.

- [ ] **Step 1: Add boundary tracking + the comment arm**

In `src/lexer.rs`, `scan_cmdsub_body` currently begins:

```rust
fn scan_cmdsub_body(
    chars: &mut CharCursor<'_>,
    out: &mut String,
    unterminated: LexError,
) -> Result<(), LexError> {
    let mut depth: usize = 0;
    loop {
        match chars.next() {
            None => return Err(unterminated),
            Some(')') if depth == 0 => return Ok(()),
            Some(')') => {
                depth -= 1;
                out.push(')');
            }
            Some('(') => {
                depth += 1;
                out.push('(');
            }
            Some('\\') => {
                out.push('\\');
                match chars.next() {
                    Some(c) => out.push(c),
                    None => return Err(unterminated),
                }
            }
            Some('\'') => {
                out.push('\'');
                loop {
                    match chars.next() {
                        Some('\'') => {
                            out.push('\'');
                            break;
                        }
                        Some(c) => out.push(c),
                        None => return Err(unterminated),
                    }
                }
            }
            Some('"') => {
                out.push('"');
                loop {
                    match chars.next() {
                        Some('"') => {
                            out.push('"');
                            break;
                        }
                        Some('\\') => {
                            out.push('\\');
                            match chars.next() {
                                Some(c) => out.push(c),
                                None => return Err(unterminated),
                            }
                        }
                        Some(c) => out.push(c),
                        None => return Err(unterminated),
                    }
                }
            }
            Some(c) => out.push(c),
        }
    }
}
```

Replace the whole function body with the boundary-aware version (adds `at_boundary`, a `#` arm, and `at_boundary` updates on the existing arms):

```rust
fn scan_cmdsub_body(
    chars: &mut CharCursor<'_>,
    out: &mut String,
    unterminated: LexError,
) -> Result<(), LexError> {
    let mut depth: usize = 0;
    // bash recognises a `#` comment only at a word boundary: start-of-body,
    // after whitespace, or after a metacharacter `( ) ; & | < >`. Track it so a
    // `)` inside a word-start comment does not close the substitution.
    let mut at_boundary = true;
    loop {
        match chars.next() {
            None => return Err(unterminated),
            Some('#') if at_boundary => {
                // Word-start comment to end-of-line: keep it VERBATIM in `out`
                // (re-tokenized + stripped later) so its `)` is not counted. The
                // next char (the newline) restores `at_boundary`.
                out.push('#');
                while let Some(&c) = chars.peek() {
                    if c == '\n' {
                        break;
                    }
                    out.push(c);
                    chars.next();
                }
            }
            Some(')') if depth == 0 => return Ok(()),
            Some(')') => {
                depth -= 1;
                out.push(')');
                at_boundary = true;
            }
            Some('(') => {
                depth += 1;
                out.push('(');
                at_boundary = true;
            }
            Some('\\') => {
                out.push('\\');
                match chars.next() {
                    Some(c) => out.push(c),
                    None => return Err(unterminated),
                }
                at_boundary = false;
            }
            Some('\'') => {
                out.push('\'');
                loop {
                    match chars.next() {
                        Some('\'') => {
                            out.push('\'');
                            break;
                        }
                        Some(c) => out.push(c),
                        None => return Err(unterminated),
                    }
                }
                at_boundary = false;
            }
            Some('"') => {
                out.push('"');
                loop {
                    match chars.next() {
                        Some('"') => {
                            out.push('"');
                            break;
                        }
                        Some('\\') => {
                            out.push('\\');
                            match chars.next() {
                                Some(c) => out.push(c),
                                None => return Err(unterminated),
                            }
                        }
                        Some(c) => out.push(c),
                        None => return Err(unterminated),
                    }
                }
                at_boundary = false;
            }
            Some(c) => {
                out.push(c);
                at_boundary = c.is_whitespace()
                    || matches!(c, ';' | '&' | '|' | '<' | '>');
            }
        }
    }
}
```

- [ ] **Step 2: Build**

Run: `cargo build 2>&1 | tail -5`
Expected: clean.

- [ ] **Step 3: Manual cmdsub behavior check**

```bash
B=target/debug/huck
chk() { printf '%b\n' "$2" > /tmp/v183c.sh
  printf '%-20s huck=<%s> bash=<%s>\n' "$1" \
    "$($B /tmp/v183c.sh 2>&1 | tr '\n' '|')" "$(bash --norc /tmp/v183c.sh 2>&1 | tr '\n' '|')"; }
chk 'paren in comment' 'echo "[$(echo hi  # c with ) paren\n)]"'
chk 'comment after $(' 'echo "[$(# c )\necho yo)]"'
chk 'hash after ;'     'echo "[$(echo a;# c ) z\necho b)]"'
chk 'midword a#b'      'echo "[$(echo a#b)]"'
chk 'nested then #'    'echo "[$( (echo hi)  # ) c\n)]"'
chk 'plain control'    'echo "[$(echo hello)]"'
```
Expected (huck == bash): `[hi]`, `[yo]`, `[a|b]`, `[a#b]`, `[hi]`, `[hello]`.

- [ ] **Step 4: Add unit tests + run existing cmdsub tests**

In `src/lexer.rs` `mod tests`, near the `scan_cmdsub_body_*` tests, add:

```rust
    #[test]
    fn scan_cmdsub_body_skips_word_start_comment() {
        // v183: a word-start `#` comment is kept verbatim in the body; a `)`
        // inside it does NOT close the substitution. Stops at the FINAL `)`.
        let mut chars = CharCursor::new("echo hi # c with ) paren\n)rest");
        let mut out = String::new();
        scan_cmdsub_body(&mut chars, &mut out, LexError::UnterminatedSubstitution).unwrap();
        assert_eq!(out, "echo hi # c with ) paren\n");
        assert_eq!(chars.next(), Some('r'));
    }

    #[test]
    fn scan_cmdsub_body_midword_hash_not_comment() {
        // v183 regression: `#` mid-word (`a#b`) is literal, not a comment.
        let mut chars = CharCursor::new("echo a#b)");
        let mut out = String::new();
        scan_cmdsub_body(&mut chars, &mut out, LexError::UnterminatedBrace).unwrap();
        assert_eq!(out, "echo a#b");
    }
```

Run: `cargo test --lib scan_cmdsub_body 2>&1 | tail -12`
Expected: all `scan_cmdsub_body_*` tests pass (the 4 existing + 2 new), 0 failed.

- [ ] **Step 5: clippy**

Run: `cargo clippy 2>&1 | tail -5`
Expected: no new warnings.

- [ ] **Step 6: Commit**

```bash
git add src/lexer.rs
git commit -m "$(cat <<'EOF'
v183: scan_cmdsub_body skips word-start # comments

The $() close-finder counted a `)` inside a comment as the closing paren. Add
word-boundary tracking (start / whitespace / `( ) ; & | < >`) and consume a
word-start `#` comment verbatim into the body (re-tokenized + stripped later)
so its `)` is not counted. Covers $() in command position, inside array
elements / ${} operands, and the arith disambiguation (all route through
scan_cmdsub_body).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

## Report back
STATUS, commit SHA, the Step 3 table, Step 4 result (incl. that the 4 existing tests stay green), any concerns.

---

### Task 3: Bash-diff harnesses (array + cmdsub)

**Files:**
- Create: `tests/scripts/array_comment_diff_check.sh`
- Create: `tests/scripts/cmdsub_comment_diff_check.sh`

**Context:** Harnesses pipe each fragment to bash and huck and assert byte-identical output. Fragments may be multi-line single-quoted strings. Model: `tests/scripts/dollar_quote_forms_diff_check.sh`.

- [ ] **Step 1: Write the array harness**

Create `tests/scripts/array_comment_diff_check.sh`:

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v183: `#` comments inside an array
# literal `name=( … )`. A comment at element-start runs to end-of-line; a `)`/`(`
# inside it must NOT be read as elements or close the array (huck used to
# mis-parse → "expected a command": kernel ioam6.sh / sysctl.sh commented rows).
# Mid-word `#` and `$#` stay literal. rc 0 in bash → compare full stdout+exit.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash --norc --noprofile 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

check "comment with paren and close" 'a=(
  1
  # note (paren) and ) here
  2
); echo "${a[@]}"'
check "comment right after open"     'a=(  # lead comment )
  x y
); echo "${a[@]}"'
check "trailing comment after elem"  'a=(
  p q  # trailing ) brace
  r
); echo "${a[@]}"'
check "multiple comment lines"       'a=(
  # first ) comment
  alpha
  # second (paren) comment
  beta
); echo "${a[@]}"'
check "midword hash literal"         'a=(x#y a#b); echo "${a[@]}"'
check "dollar-hash count"            'set -- a b c; a=($#); echo "${a[@]}"'
check "plain array no comment"       'a=(one two three); echo "${a[@]}"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 2: Write the cmdsub harness**

Create `tests/scripts/cmdsub_comment_diff_check.sh`:

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v183: `#` comments inside `$( … )`. A
# word-start `#` comment runs to end-of-line; a `)` inside it must NOT close the
# substitution (huck's close-finder used to count it). Mid-word `#` stays
# literal. rc 0 in bash → compare full stdout+exit.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash --norc --noprofile 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

check "paren in comment"      'echo "[$(echo hi  # c with ) paren
)]"'
check "comment after open"    'echo "[$(# c with ) paren
echo yo)]"'
check "hash after semicolon"  'echo "[$(echo a;# c ) z
echo b)]"'
check "hash after pipe"       'echo "[$(echo a |# c ) z
cat)]"'
check "midword hash literal"  'echo "[$(echo a#b)]"'
check "nested paren then cmt" 'echo "[$( (echo hi)  # ) c
)]"'
check "plain cmdsub control"  'echo "[$(echo hello)]"'
check "var assign cmdsub"     'x=$(echo one  # ) two
); echo "[$x]"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 3: Make executable, build, run both**

Run:

```bash
chmod +x tests/scripts/array_comment_diff_check.sh tests/scripts/cmdsub_comment_diff_check.sh
cargo build 2>&1 | tail -2
bash tests/scripts/array_comment_diff_check.sh
bash tests/scripts/cmdsub_comment_diff_check.sh
```

Expected: array `Total: 7, Pass: 7, Fail: 0`; cmdsub `Total: 8, Pass: 8, Fail: 0`; both exit 0. If any case FAILs, STOP and report (do NOT weaken a case).

- [ ] **Step 4: Prove both harnesses are non-tautological**

```bash
git worktree add /tmp/huck-prefix main 2>&1 | tail -1
( cd /tmp/huck-prefix && cargo build 2>&1 | tail -1 )
echo "--- array vs pre-fix ---"; HUCK_BIN=/tmp/huck-prefix/target/debug/huck bash tests/scripts/array_comment_diff_check.sh; echo "exit=$?"
echo "--- cmdsub vs pre-fix ---"; HUCK_BIN=/tmp/huck-prefix/target/debug/huck bash tests/scripts/cmdsub_comment_diff_check.sh; echo "exit=$?"
git worktree remove --force /tmp/huck-prefix
```

Expected: BOTH harnesses FAIL against the pre-fix binary (the comment cases mis-parse; controls pass), exit 1 each. If either PASSES pre-fix, it is tautological — STOP and report. (`main` is the pre-v183 baseline.)

- [ ] **Step 5: Commit**

```bash
git add tests/scripts/array_comment_diff_check.sh tests/scripts/cmdsub_comment_diff_check.sh
git commit -m "$(cat <<'EOF'
v183: bash-diff harnesses for # comments in array literals and $()

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

## Report back
STATUS, commit SHA, both Total/Pass/Fail lines, the Step 4 pre-fix exits and which cases failed pre-fix (proving non-tautology).

---

### Task 4: Parse-sweep payoff, regression, and deferred-divergence docs

**Files:**
- Modify: `docs/bash-divergences.md` (add two `[deferred]` entries + bump Tier-4 count)
- Modify (only if found): old-behavior tests from the up-front grep
- Run: full `cargo test`, all `tests/scripts/*_diff_check.sh`, `tools/parse_sweep.sh`

- [ ] **Step 1: Up-front grep**

Run:

```bash
grep -rn "skip_array_literal_whitespace" src/ tests/   # must be ZERO (rename complete)
grep -rn "scan_array_literal\|scan_cmdsub_body\|ArrayLiteral\|=(" src/ tests/ | grep -i '#' | head -30
```

Confirm the old name is fully gone. Classify any `#`-in-array/cmdsub test hits UPDATE (encodes pre-fix behavior) vs LEAVE. Report each; report "no updates needed" if so.

- [ ] **Step 2: Full test suite**

Run: `cargo test 2>&1 | tail -30`
Expected: 0 failures across all unit + integration binaries. A failure on a `#`-in-array/cmdsub assertion: verify against bash; if bash treats it as a comment, the test encoded the bug (UPDATE); else STOP and report.

- [ ] **Step 3: Whole bash-diff harness suite**

```bash
cargo build 2>&1 | tail -1
for s in tests/scripts/*_diff_check.sh; do
  out=$(bash "$s" 2>&1); rc=$?
  printf '%s rc=%s %s\n' "$s" "$rc" "$(echo "$out" | tail -1)"
done | grep -v 'Fail: 0' || echo "ALL HARNESSES GREEN"
```
Expected: `ALL HARNESSES GREEN`. (Note: huck's own `param_cmdsub_split_diff_check.sh` is now also parseable by huck — but it is still run by bash as a harness; it must stay green.)

- [ ] **Step 4: Parse-sweep payoff**

```bash
tools/parse_sweep.sh tools/scripts.tsv tools/parse_results.tsv 2>&1 | tail -15
for F in \
  /usr/src/linux-headers-6.8.0-110/tools/testing/selftests/net/ioam6.sh \
  /usr/src/linux-headers-6.8.0-110/tools/testing/selftests/sysctl/sysctl.sh \
  /home/john/projects/shuck/tests/scripts/param_cmdsub_split_diff_check.sh; do
  echo "=== $F ==="; ./target/debug/huck -n "$F"; echo "huck-n rc=$?"; bash -n "$F"; echo "bash-n rc=$?"
done
echo "=== remaining 'expected a command' gaps? ==="
awk -F'\t' '$3=="HUCK_GAP" && $7 ~ /expected a command/{print $6}' tools/parse_results.tsv || echo "(none)"
```
Expected: ioam6, sysctl, param_cmdsub_split `huck -n` SILENT rc 0. Report new HUCK_GAP vs 20; LENIENT/CRASH stay 0; whether the file-history snapshots cleared; whether any remaining "expected a command" row is a DIFFERENT construct.

- [ ] **Step 5: Add the two deferred-divergence entries**

In `docs/bash-divergences.md`, in the Tier-4 (low-impact) section, add two `[deferred]` low entries (use the next free `L-` ids — grep `L-` to find them):

```markdown
- **L-NN: subshell with a leading comment/blank line before the first command** — `[deferred]`, low (found during v183). `(\n# c\necho hi\n)` errors in huck (`expected a command`) though `( # c\necho hi )` works. `(` is a real token and the body is tokenized normally (comments stripped), so this is NOT a comment-scanner bug — `parse_subshell` does not skip leading newlines / comment-blank lines before the first command. Rare (a comment as the very first line of a subshell body); the same pattern inside `{ … }` groups already works.
- **L-NN: subscripted array element whose value begins with `#` is dropped** — `[deferred]`, low (found during v183). `a=([0]=#x)` → huck `${a[0]}` empty, bash `#x`. The `[i]=` subscript is consumed, then the value word `#x` is collected and RE-tokenized via `tokenize`, whose word-start `#` rule treats the leading `#` as a comment → empty value. bash keeps a subscript value verbatim. The bare element form `a=(#x …)` correctly IS a comment in both shells. Pathological (`#`-leading subscript value); fix would re-tokenize the value with a non-comment-start context.
```

Update the Tier-4 count in the Summary table (+2).

- [ ] **Step 6: Commit**

```bash
git add docs/bash-divergences.md
# add any test files updated in Step 1/2
git commit -m "$(cat <<'EOF'
v183: log deferred divergences (subshell leading comment; [i]=#value drop)

Two distinct bugs found while broadening v183 to the comment-scanner class,
not fixed here. Plus regression + parse-sweep verification (array cluster +
$() comment close-finder cleared).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

## Report back
STATUS, commit SHA, the Step 1 grep result (old name gone + classification), Step 2 cargo test summary, Step 3 result, Step 4 per-file rc + new HUCK_GAP vs 20 + which "expected a command" rows remain, confirmation the two L-entries were added with the count bumped.

---

## Final review (whole-branch, before merge)

- [ ] **Step 1: Review the full diff** — `git diff main...HEAD`. Confirm: `skip_line_comment` added + used in the main loop; `skip_array_literal_separators` rename+fold; `scan_cmdsub_body` boundary-aware comment skip; the helper/array/cmdsub unit tests; two new harnesses; two divergence entries. Nothing else.
- [ ] **Step 2: Scope boundary** — `scan_array_element_word`, `scan_backtick_body`, `${…}` operand finders, `parse_subshell`, and the runtime unchanged. The subshell + `[i]=#value` bugs are LOGGED, not fixed.
- [ ] **Step 3: Hand off** — project merge ritual (AskUserQuestion before merging to main): merge `--no-ff`, push, delete the local branch, record v183 in `project_huck_iterations.md` + `MEMORY.md`, update the backlog note.

---

## Self-Review (plan vs spec)

**1. Spec coverage:**
- Shared `skip_line_comment` + main-loop dedup → Task 1 Steps 2-3. ✓
- Array separator rename+fold, call-site update → Task 1 Steps 4-5. ✓
- `scan_cmdsub_body` boundary-aware comment skip → Task 2 Step 1. ✓
- Unit tests (helper, array ×2, cmdsub ×2) → Task 1 Step 8 + Task 2 Step 4. ✓
- Two harnesses (array, cmdsub) with results + non-tautology → Task 3. ✓
- Parse-sweep payoff + up-front grep (incl. old-name-gone) + full regression → Task 4 Steps 1-4. ✓
- Two deferred-divergence entries + count bump → Task 4 Step 5. ✓
- All harnesses green + clippy → Task 3/Task 4 Step 3 + Task 1/2 clippy. ✓

**2. Placeholder scan:** No TBD/TODO. Every code step shows complete before/after code; every run step shows command + expected output. (`L-NN` in the divergence entries is resolved by the grep-for-next-id instruction in the same step.)

**3. Type consistency:** `skip_line_comment(&mut CharCursor)` used by the main loop, array separator skip, (and the helper unit test). `scan_cmdsub_body(chars, out, unterminated) -> Result<(), LexError>` signature unchanged (body only; the 4 existing tests pin its non-comment behavior). `skip_array_literal_separators` is the sole renamed symbol, with its one call site updated and zero test references (verified). `parse_assignments`/`array_lit` helpers + `ArrayLiteralElement.subscript: Option<Word>` verified.

**Resolved during planning:** verified all 9 contract fragments (array + cmdsub) against bash; the rename's blast radius (lexer.rs only, no tests); that the 4 existing `scan_cmdsub_body_*` tests assert non-comment behavior unaffected by the change; the boundary rule (`( ) ; & | < >` + whitespace + start).
