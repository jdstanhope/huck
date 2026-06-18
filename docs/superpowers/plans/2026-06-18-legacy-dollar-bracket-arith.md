# Legacy `$[ expr ]` Arithmetic Expansion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make huck evaluate `$[ expr ]` exactly like `$(( expr ))` (bash's deprecated arithmetic-expansion form), which huck currently does not implement at all.

**Architecture:** Lex-time only. Add a `$[` arm to `scan_dollar_expansion` that scans to the matching `]` with a new fully-aware close-finder, then feeds the body through the existing `arith_string_to_word`, emitting the existing `WordPart::Arith`. All arithmetic semantics and evaluation are reused unchanged. No AST, parser, expand, or executor change.

**Tech Stack:** Rust. Files: `src/lexer.rs` (lexer + unit tests), `src/shell.rs` (error message), `src/continuation.rs` (REPL continuation), `tests/scripts/legacy_arith_diff_check.sh` (bash-diff harness).

**Spec:** `docs/superpowers/specs/2026-06-18-legacy-dollar-bracket-arith-design.md`

**Background the implementer needs:**
- huck's lexer tokenizes the whole input, then parses. `scan_dollar_expansion` (`src/lexer.rs`) handles everything after a `$`: `$(…)`, `$((…))`, `${…}`, `$'…'`, `$var`, etc. It has NO `$[` arm today, so `$[3+4]` lexes as a literal `$` plus ordinary `[3+4]` shell tokens (wrong — bash prints `7`).
- A `$((…))` expansion produces `WordPart::Arith { body: Word, quoted: bool }`, where `body` is built by `arith_string_to_word(inner, opts)` — an *expandable Word*, NOT an evaluated number (arith is parsed/evaluated later, at expand time). `$[…]` will produce the identical `WordPart::Arith`.
- The existing `$((…))` close-finder is `scan_arith_body` (scans to `))`, balancing parens). `$[…]` needs an analogous close-finder that scans to `]`.
- An arithmetic body can legitimately contain `]`: array subscripts (`a[1]`, `${a[i]}`), nested `$[…]`, and `]` inside nested `$(…)`/quotes. The close-finder must not stop at those — hence "fully aware".
- `scan_cmdsub_body(chars, out, err)` is the canonical `$(…)` scanner: the opening `$(` is already consumed by the caller; it appends the body to `out` and consumes (but does NOT append) the matching `)`. It is itself quote- and nesting-aware (handles `$( … )`, `$(( … ))`, `$( (…) )`).
- Bash-diff harnesses live in `tests/scripts/*_diff_check.sh`; each runs a fragment through `bash -c` and `huck -c` and asserts byte-identical combined output + exit. They are run by `bash tests/scripts/<name>.sh` (build huck first).

---

## Task 1: Lex `$[ expr ]` as arithmetic

**Files:**
- Modify: `src/lexer.rs` — add `LexError::UnterminatedLegacyArith` (near line 21, before `UnterminatedArithBlock`); add helpers `push_quoted_span`, `scan_braced_skip`, `scan_legacy_arith_body` (near `scan_arith_body`, ~line 2074); add the `$[` arm in `scan_dollar_expansion` (after the `Some('{')` arm, ~line 1793).
- Modify: `src/shell.rs` — add the user-facing message arm (after line 804).
- Test: `src/lexer.rs` `mod tests` (near `tokenize_arith_simple`, ~line 5341).

- [ ] **Step 1: Add the `LexError` variant and its message (so the project compiles with the new variant).**

In `src/lexer.rs`, insert before the `UnterminatedArithBlock` variant (the doc-commented cluster, ~line 21):

```rust
    /// `$[ 1+2` — EOF before the `]` closing a legacy `$[ … ]` arithmetic
    /// expansion (bash's deprecated synonym for `$(( … ))`).
    UnterminatedLegacyArith,
```

In `src/shell.rs`, immediately after the `UnterminatedArith` arm (line 804):

```rust
        LexError::UnterminatedLegacyArith => {
            ": unterminated '$[' arithmetic expansion (expected ']')".to_string()
        }
```

- [ ] **Step 2: Verify it still compiles.**

Run: `cargo build 2>&1 | tail -3`
Expected: builds clean (the new variant is declared and exhaustively handled; no behavior yet).

- [ ] **Step 3: Write the failing lexer unit tests.**

In `src/lexer.rs` `mod tests`, near `tokenize_arith_simple` (it defines a helper `arith_body_lit(part) -> &str` that returns the text of a single-literal arith body — reuse it):

```rust
    #[test]
    fn tokenize_legacy_arith_basic() {
        // `$[ … ]` is bash's deprecated synonym for `$(( … ))`; the body is
        // deferred as a single literal, evaluated at eval time.
        let tokens = tokenize("$[2**(3*2)]").unwrap();
        assert_eq!(tokens.len(), 1);
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        assert_eq!(parts.len(), 1);
        let WordPart::Arith { quoted, .. } = &parts[0] else {
            panic!("expected Arith part, got {:?}", parts[0])
        };
        assert!(!(*quoted));
        assert_eq!(arith_body_lit(&parts[0]), "2**(3*2)");
    }

    #[test]
    fn tokenize_legacy_arith_array_subscript() {
        // Raw `[`/`]` nesting: the subscript brackets balance, so the FINAL `]`
        // closes the expansion (not the subscript's `]`).
        let tokens = tokenize("$[a[1]+1]").unwrap();
        assert_eq!(tokens.len(), 1);
        let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
        assert_eq!(parts.len(), 1);
        assert_eq!(arith_body_lit(&parts[0]), "a[1]+1");
    }

    #[test]
    fn tokenize_legacy_arith_aware_close() {
        // A `]` inside a nested command sub or a quoted span must NOT close the
        // `$[…]` early — each lexes to exactly one Arith word.
        for src in ["$[ $(echo ']')+1 ]", "$[ \"x]\" + 1 ]"] {
            let tokens = tokenize(src).unwrap_or_else(|e| panic!("{src}: {e:?}"));
            assert_eq!(tokens.len(), 1, "{src} closed early: {tokens:?}");
            let Token::Word(Word(parts)) = &tokens[0] else { panic!("{src}") };
            assert_eq!(parts.len(), 1, "{src}: {parts:?}");
            assert!(
                matches!(parts[0], WordPart::Arith { .. }),
                "{src}: {:?}",
                parts[0]
            );
        }
    }

    #[test]
    fn tokenize_legacy_arith_unterminated() {
        assert!(matches!(
            tokenize("$[ 1+2"),
            Err(LexError::UnterminatedLegacyArith)
        ));
    }
```

- [ ] **Step 4: Run the new tests to confirm they fail.**

Run: `cargo test --lib tokenize_legacy_arith 2>&1 | tail -20`
Expected: FAIL — without a `$[` arm, `$[2**(3*2)]` lexes as a literal `$` + tokens (not an `Arith` part), and `$[ 1+2` does not error, so the `matches!`/`assert_eq!` assertions fail.

- [ ] **Step 5: Implement the helpers and the `$[` arm.**

In `src/lexer.rs`, add these three functions next to `scan_arith_body` (after it ends, ~line 2074):

```rust
/// Appends a quoted span — the opening quote already pushed by the caller —
/// through its matching closing `quote`, verbatim. Single quotes take every
/// char literally; double quotes honor `\` so `\"` does not close the span.
/// Running out of input returns `Err(err)`.
fn push_quoted_span(
    chars: &mut CharCursor<'_>,
    quote: char,
    out: &mut String,
    err: LexError,
) -> Result<(), LexError> {
    loop {
        match chars.next() {
            None => return Err(err),
            Some(c) if c == quote => {
                out.push(c);
                return Ok(());
            }
            Some('\\') if quote == '"' => {
                out.push('\\');
                if let Some(c) = chars.next() {
                    out.push(c);
                }
            }
            Some(c) => out.push(c),
        }
    }
}

/// Skips a `${…}` parameter expansion VERBATIM — the opening `${` already
/// consumed and pushed by the caller — appending through the matching `}` at
/// brace-depth 0 (inclusive). Tracks `{`/`}` depth and `'…'`/`"…"` spans so a
/// `}` inside a nested expansion or quote does not close early. Used by
/// `scan_legacy_arith_body` so a `]` inside `${…}` cannot close the `$[…]`.
fn scan_braced_skip(
    chars: &mut CharCursor<'_>,
    out: &mut String,
) -> Result<(), LexError> {
    let mut depth: usize = 1; // inside the outer `${`
    loop {
        match chars.next() {
            None => return Err(LexError::UnterminatedLegacyArith),
            Some('{') => {
                depth += 1;
                out.push('{');
            }
            Some('}') => {
                depth -= 1;
                out.push('}');
                if depth == 0 {
                    return Ok(());
                }
            }
            Some(q @ ('\'' | '"')) => {
                out.push(q);
                push_quoted_span(chars, q, out, LexError::UnterminatedLegacyArith)?;
            }
            Some(c) => out.push(c),
        }
    }
}

/// Reads the inner text of a `$[ … ]` legacy arithmetic expansion. The opening
/// `$[` has already been consumed; this scans forward to the matching `]` and
/// returns the inner text (without the closing `]`). bash treats `$[ expr ]` as
/// exactly `$(( expr ))`, so the caller feeds the result to
/// `arith_string_to_word`. "Fully aware": tracks raw `[`/`]` nesting (so array
/// subscripts `a[1]`, `${a[i]}`, and nested `$[…]` balance as raw brackets) and
/// consumes `'…'`/`"…"` quoted spans and nested `$(…)`/`${…}` verbatim, so a `]`
/// inside any of them does not close the expansion. EOF before the close yields
/// `UnterminatedLegacyArith`.
fn scan_legacy_arith_body(
    chars: &mut CharCursor<'_>,
) -> Result<String, LexError> {
    let mut body = String::new();
    let mut depth: usize = 0; // raw `[` nesting
    loop {
        match chars.next() {
            None => return Err(LexError::UnterminatedLegacyArith),
            Some('[') => {
                depth += 1;
                body.push('[');
            }
            Some(']') => {
                if depth == 0 {
                    return Ok(body);
                }
                depth -= 1;
                body.push(']');
            }
            Some(q @ ('\'' | '"')) => {
                body.push(q);
                push_quoted_span(chars, q, &mut body, LexError::UnterminatedLegacyArith)?;
            }
            Some('\\') => {
                body.push('\\');
                if let Some(c) = chars.next() {
                    body.push(c);
                }
            }
            Some('$') => {
                body.push('$');
                match chars.peek().copied() {
                    Some('(') => {
                        chars.next(); // consume '('
                        body.push('(');
                        // scan_cmdsub_body expects `$(` already consumed; it
                        // appends the body and consumes the matching `)`.
                        scan_cmdsub_body(chars, &mut body, LexError::UnterminatedLegacyArith)?;
                        body.push(')');
                    }
                    Some('{') => {
                        chars.next(); // consume '{'
                        body.push('{');
                        scan_braced_skip(chars, &mut body)?;
                    }
                    // `$x` / `$1` / nested `$[…]` — handled by the raw-char arms
                    // (a nested `$[…]`'s own brackets balance via `depth`).
                    _ => {}
                }
            }
            Some(c) => body.push(c),
        }
    }
}
```

Then add the `$[` arm in `scan_dollar_expansion`, immediately after the `Some('{')` arm (which ends at ~line 1793):

```rust
        Some('[') => {
            chars.next(); // consume '['
            let inner = scan_legacy_arith_body(chars)?;
            let body = arith_string_to_word(&inner, opts)?;
            parts.push(WordPart::Arith { body, quoted });
        }
```

- [ ] **Step 6: Run the new tests to confirm they pass.**

Run: `cargo test --lib tokenize_legacy_arith 2>&1 | tail -12`
Expected: PASS — all four tests green.

- [ ] **Step 7: Run the full lib tests + clippy to confirm no regression.**

Run: `cargo test --lib 2>&1 | grep "test result:" && cargo clippy 2>&1 | tail -3`
Expected: `test result: ok. … 0 failed`; clippy clean (no new warnings on the added functions).

- [ ] **Step 8: Commit.**

```bash
git add src/lexer.rs src/shell.rs
git commit -m "$(cat <<'EOF'
v188: lex $[ expr ] as arithmetic (alias of $(( expr )))

scan_dollar_expansion gains a $[ arm that scans to the matching ] via a new
fully-aware scan_legacy_arith_body (balances raw [ ], skips quotes + nested
$(…)/${…}) and feeds the body to arith_string_to_word, emitting WordPart::Arith.
New LexError::UnterminatedLegacyArith. No AST/parser/expand/executor change.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: REPL continuation for an unterminated `$[`

**Files:**
- Modify: `src/continuation.rs` — add `UnterminatedLegacyArith` to `is_unterminated_lex` (line ~35) and a test (in `mod tests`, near `unterminated_arith_block_requests_more_input`, ~line 188).

- [ ] **Step 1: Write the failing continuation test.**

In `src/continuation.rs` `mod tests`, after `unterminated_arith_block_requests_more_input`:

```rust
    #[test]
    fn unterminated_legacy_arith_requests_more_input() {
        // `$[ 1 +` — no closing `]`. The lexer signals UnterminatedLegacyArith,
        // which is_unterminated_lex treats as incomplete so the REPL prompts for
        // continuation (via the generic OpenQuote reason, like other unterminated
        // lex spans).
        assert_eq!(
            classify("echo $[ 1 +", false),
            Completeness::Incomplete(ContinuationReason::OpenQuote)
        );
    }
```

- [ ] **Step 2: Run it to confirm it fails.**

Run: `cargo test --lib unterminated_legacy_arith_requests_more_input 2>&1 | tail -12`
Expected: FAIL — `is_unterminated_lex` does not yet include `UnterminatedLegacyArith`, so `classify` returns `Completeness::Error`, not `Incomplete(OpenQuote)`.

- [ ] **Step 3: Register the variant in `is_unterminated_lex`.**

In `src/continuation.rs`, add the line to the `matches!` list (after `UnterminatedArith`, line ~35):

```rust
            | LexError::UnterminatedArith
            | LexError::UnterminatedLegacyArith
            | LexError::UnterminatedArithBlock
```

- [ ] **Step 4: Run the test to confirm it passes.**

Run: `cargo test --lib unterminated_legacy_arith_requests_more_input 2>&1 | tail -8`
Expected: PASS.

- [ ] **Step 5: Commit.**

```bash
git add src/continuation.rs
git commit -m "$(cat <<'EOF'
v188: REPL continuation for an unterminated $[

is_unterminated_lex now treats UnterminatedLegacyArith as incomplete, so a
multi-line `$[ …` prompts for continuation like `$((` and other open spans.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Bash-diff harness

**Files:**
- Create: `tests/scripts/legacy_arith_diff_check.sh`

- [ ] **Step 1: Create the harness.**

Write `tests/scripts/legacy_arith_diff_check.sh` (mirrors the structure of `tests/scripts/process_sub_diff_check.sh`):

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v188: legacy $[ … ] arithmetic
# expansion (bash's deprecated synonym for $(( … ))).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1; echo "rc=$?")
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

check "basic add"        'echo $[3+4]'
check "no-glob star"     'echo $[ 2 * 5 ]'
check "exponent paren"   'len=4; echo $[ 2**(len*2)-1 ]'
check "ternary"          'IP6=6; echo $[ IP6 ? 128 : 32 ]'
check "base prefix"      'd=ff; echo $[ 16#$d ]'
check "array subscript"  'a=(5 6); echo $[a[1]+1]'
check "rbracket in dpe"  'a=(9); echo $[ ${a[0]} + 1 ]'
check "nested cmdsub"    'echo $[$(echo 3)+1]'
check "nested dollarvar" 'x=4; echo $[${x}+1]'
check "nested legacy"    'echo $[ $[2+3] * 2 ]'
check "in dquotes"       'echo "$[1+2]"'
check "comma"            'echo $[1,2,3]'
check "division"         'echo $[10/3]'
check "neg + assign"     'echo $[ -5 + 1 ]'
check "control $(( ))"   'echo $((1+1))'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 2: Make it executable and run it.**

Run:
```bash
chmod +x tests/scripts/legacy_arith_diff_check.sh
cargo build 2>&1 | tail -1
bash tests/scripts/legacy_arith_diff_check.sh
```
Expected: every line `PASS`, final `Total: 15, Pass: 15, Fail: 0`, exit 0.

- [ ] **Step 3: Prove the harness is non-tautological (would have failed pre-fix).**

Run (builds the pre-fix binary in a throwaway worktree and points the harness at it):
```bash
git worktree add -d /tmp/huck-prefix HEAD~2 2>/dev/null && \
  ( cd /tmp/huck-prefix && cargo build 2>&1 | tail -1 ) && \
  HUCK_BIN=/tmp/huck-prefix/target/debug/huck bash tests/scripts/legacy_arith_diff_check.sh | tail -3
git worktree remove --force /tmp/huck-prefix 2>/dev/null
```
Expected: the pre-fix run reports multiple FAILs (e.g. `basic add`, `exponent paren`, `no-glob star`) — confirming the harness exercises the fix. (`HEAD~2` is before Task 1+2; adjust to the commit before this branch's first task if the worktree base differs.)

- [ ] **Step 4: Commit.**

```bash
git add tests/scripts/legacy_arith_diff_check.sh
git commit -m "$(cat <<'EOF'
v188: bash-diff harness for legacy $[ ] arithmetic

15 cases (pktgen shapes, array subscripts, nested $(…)/${…}/$[…], dquotes,
ternary/comma/division) byte-identical to bash; non-tautological (fails pre-fix).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Parse-sweep payoff, full regression, and memory

**Files:**
- Modify (memory): `/home/john/.claude/projects/-home-john-projects-shuck/memory/project_huck_iterations.md` and `/home/john/.claude/projects/-home-john-projects-shuck/memory/MEMORY.md`.
- No `bash-divergences.md` change (sweep-found; no tracked `M-*`/`L-*`).

- [ ] **Step 1: Up-front grep for any test that encoded the OLD `$[…]`-as-literal behavior.**

Run:
```bash
grep -rn '\$\[' src/ tests/ | grep -iv 'legacy_arith\|2026-06-18' | grep -v '//' | head -30
```
Classify each hit: UPDATE only if it asserts `$[…]` lexes/expands as a literal (none expected — the form was broken, not deliberately tested). LEAVE array-subscript-assignment hits (`a[1]=`, `${a[i]}`) — those are unrelated to `$[`.

- [ ] **Step 2: Full test suite (0 failures).**

Run: `cargo test 2>&1 | grep "test result:" | grep -v "0 failed" || echo "ALL GREEN"`
Expected: `ALL GREEN`.

- [ ] **Step 3: All bash-diff harnesses green.**

Run:
```bash
cargo build 2>&1 | tail -1
for s in tests/scripts/*_diff_check.sh; do
  out=$(bash "$s" 2>&1); echo "$s -> $(echo "$out" | tail -1)"
done | grep -v 'Fail: 0' || echo "ALL HARNESSES GREEN"
```
Expected: `ALL HARNESSES GREEN`.

- [ ] **Step 4: Parse-sweep payoff.**

Run:
```bash
tools/parse_sweep.sh tools/scripts.tsv tools/parse_results.tsv 2>&1 | tail -8
echo "=== bucket totals ==="
awk -F'\t' 'NR>1{c[$3]++} END{for(k in c) printf "%-12s %d\n", k, c[k]}' tools/parse_results.tsv | sort
echo "=== pktgen functions.sh now? ==="
for f in /usr/src/linux-headers-6.8.0-110/samples/pktgen/functions.sh \
         /usr/src/linux-headers-6.8.0-124/samples/pktgen/functions.sh; do
  ./target/debug/huck -n "$f"; echo "$f huck-n rc=$?"
done
echo "=== remaining HUCK_GAP files ==="
awk -F'\t' '$3=="HUCK_GAP"{print $6}' tools/parse_results.tsv
```
Expected: both `pktgen/functions.sh` copies `huck -n` rc 0; `HUCK_GAP` 5 → 3; `HUCK_LENIENT`/`HUCK_CRASH`/`HUCK_TIMEOUT` stay 0. Remaining gaps: `byobu-ulevel` and the two `perf-completion.sh` copies (`${=1}`).

- [ ] **Step 5: Record the iteration in memory.**

Prepend a v188 entry to `project_huck_iterations.md` (newest-first) summarizing: legacy `$[ expr ]` implemented as a lex-time alias of `$(( expr ))` (new `$[` arm in `scan_dollar_expansion` + fully-aware `scan_legacy_arith_body` close-finder + `push_quoted_span`/`scan_braced_skip` helpers + `UnterminatedLegacyArith` variant/message/continuation wiring); root cause was that `$[…]` was entirely UNIMPLEMENTED (lexed as literal `$`+`[…]`), and the sweep's "function definition / line 209" label was misleading (the real trigger was `$[ 2**(len*2)-1 ]` in the function body — bisect each cluster); HUCK_GAP 5→3; merge SHA (fill in after merge); reuse of `arith_string_to_word` meant no AST/expand/executor change.

Update the corresponding index line and the sweep-progression line in `MEMORY.md` (v187 5 → v188 3; remaining backlog = byobu `\`⏎`( array + perf-completion `${=1}`).

- [ ] **Step 6: Commit the memory update.**

```bash
git add /home/john/.claude/projects/-home-john-projects-shuck/memory/project_huck_iterations.md \
        /home/john/.claude/projects/-home-john-projects-shuck/memory/MEMORY.md
git commit -m "$(cat <<'EOF'
v188: record legacy $[ ] arithmetic iteration in memory

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

(The merge-SHA backfill in Step 5 happens after the branch is merged; if unknown at commit time, note "merge pending" and amend post-merge.)

---

## Report-back (Task 4)

Report: STATUS, commit SHAs, the Step 1 grep classification, Step 2 cargo-test summary, Step 3 harness result, Step 4 (both pktgen `huck -n` rc + new HUCK_GAP vs the 5 baseline + that LENIENT/CRASH/TIMEOUT stay 0 + that byobu-ulevel and perf-completion remain), and confirmation no `bash-divergences.md` change was needed.
