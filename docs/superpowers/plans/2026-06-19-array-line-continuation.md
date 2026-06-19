# Array Line-Continuation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make huck parse `name=\`⏎`(array)` (a `\`-newline line continuation between `=`/`+=` and the array `(`) as the array assignment it is, like bash.

**Architecture:** Add a `skip_line_continuations` lexer helper (consume `\`-newline pairs via cloned-cursor lookahead) and call it right before the array-`(` peek in the `name=(` and `name+=(` tokenizer arms. Also document huck's malformed-`${…}` parse-strictness as intentional.

**Tech Stack:** Rust. File: `src/lexer.rs` (+ tests). New harness `tests/scripts/array_line_continuation_diff_check.sh`. Doc `docs/bash-divergences.md`.

**Spec:** `docs/superpowers/specs/2026-06-19-array-line-continuation-design.md`

**Background the implementer needs:**
- huck's main tokenizer (`src/lexer.rs`) detects a compound-array RHS by peeking for `(` IMMEDIATELY after `=`/`+=`:
  - the `'='` arm (~line 918): after `current.push('=')`, `if chars.peek() == Some(&'(') { chars.next(); flush_literal(…); let elements = scan_array_literal(&mut chars, opts)?; parts.push(WordPart::ArrayLiteral(elements)); }`
  - the `'+' if … '=' …'` arm (~line 952): the `name+=(` form, similar `if chars.peek() == Some(&'(')` block.
- `chars` is a mutable local `CharCursor<'_>` (`#[derive(Clone)]`, has `peek(&mut self) -> Option<&char>` and `Iterator::next`). It's passed as `&mut chars` to scan functions.
- For `arr=\`⏎`(`, the char after `=` is `\` (the continuation), so the peek fails → no array → `(a b c)` reparses as a function-def/subshell → "function definition" syntax error.
- `\`⏎ is a POSIX line continuation deleted before tokenizing; huck's generic `'\\'` arm (~line 581, `Some('\n') => {}`) already deletes it everywhere EXCEPT it isn't seen by this lookahead peek.
- Test helper `parse_assignments(src: &str)` (in `src/lexer.rs` `mod tests`) returns the parsed assignments; each has `.target.name()`, `.append: bool`, `.value.0` (a `Vec<WordPart>`). Model new tests on `compound_rhs_is_array_literal`.
- The subscripted form `a[i]=(…)` (~line 1027) is INVALID in bash ("cannot assign list to array member") — do NOT touch it.
- Bash-diff harnesses: `tests/scripts/*_diff_check.sh`, run via `bash tests/scripts/<name>.sh`.

---

## Task 1: `skip_line_continuations` + wire into the array peeks

**Files:**
- Modify: `src/lexer.rs` — add the helper; call it in the `=` and `+=` arms.
- Test: `src/lexer.rs` `mod tests`.

- [ ] **Step 1: Write the failing lexer tests** in `src/lexer.rs` `mod tests` (near `compound_rhs_is_array_literal`):

```rust
    #[test]
    fn array_assignment_with_line_continuation() {
        // `arr=\<NL>(a b c)` — the \<NL> between `=` and `(` is a line
        // continuation (deleted pre-tokenization), so this is `arr=(a b c)`.
        let assigns = parse_assignments("arr=\\\n(a b c)");
        assert_eq!(assigns.len(), 1);
        assert_eq!(assigns[0].target.name(), "arr");
        assert!(!assigns[0].append);
        let els = assigns[0].value.0.iter().find_map(|p| match p {
            WordPart::ArrayLiteral(els) => Some(els),
            _ => None,
        }).expect("ArrayLiteral part present");
        assert_eq!(els.len(), 3);
    }

    #[test]
    fn array_append_with_line_continuation() {
        let assigns = parse_assignments("arr+=\\\n(d)");
        assert_eq!(assigns.len(), 1);
        assert!(assigns[0].append);
        let els = assigns[0].value.0.iter().find_map(|p| match p {
            WordPart::ArrayLiteral(els) => Some(els),
            _ => None,
        }).expect("ArrayLiteral part present");
        assert_eq!(els.len(), 1);
    }

    #[test]
    fn backslash_escape_after_eq_is_not_continuation() {
        // `arr=\x` — `\x` is a literal escape, NOT a continuation; no array.
        let assigns = parse_assignments("arr=\\x");
        assert_eq!(assigns.len(), 1);
        assert!(
            !assigns[0].value.0.iter().any(|p| matches!(p, WordPart::ArrayLiteral(_))),
            "a backslash-escape must not be treated as a line continuation"
        );
    }
```

- [ ] **Step 2: Run to confirm they FAIL.**

Run: `cargo test --lib array_assignment_with_line_continuation array_append_with_line_continuation backslash_escape_after_eq 2>&1 | tail -20`
Expected: the two continuation tests FAIL (no `ArrayLiteral` — currently the `\<NL>(` misparses); `backslash_escape_after_eq_is_not_continuation` already PASSES (it's a guard for no regression). If a filter with multiple names doesn't work, run each test name separately.

- [ ] **Step 3: Add the helper.** In `src/lexer.rs`, near the other free functions (e.g. just before `scan_array_literal` or `scan_dollar_expansion`):

```rust
/// Consumes any run of `\`-newline line continuations at the cursor (POSIX
/// 2.2.1: `\<NL>` is deleted before tokenizing). Uses a cloned-cursor 2-char
/// lookahead so a `\` NOT followed by a newline (a real escape like `\x`) is
/// left untouched. No-op when the cursor is not at a `\<NL>`.
fn skip_line_continuations(chars: &mut CharCursor<'_>) {
    loop {
        let mut probe = chars.clone();
        if probe.next() == Some('\\') && probe.next() == Some('\n') {
            *chars = probe;
        } else {
            return;
        }
    }
}
```

- [ ] **Step 4: Call it before the two array-`(` peeks.**

In the `'='` arm (~line 924), insert `skip_line_continuations(&mut chars);` immediately before its `if chars.peek() == Some(&'(')`:

```rust
                current.push('=');
                // A `\<NL>` line continuation may sit between `=` and the array
                // `(` (`arr=\<NL>(…)`); bash deletes it pre-tokenization.
                skip_line_continuations(&mut chars);
                if chars.peek() == Some(&'(') {
                    chars.next(); // consume '('
                    flush_literal(&mut parts, &mut current, false);
                    let elements = scan_array_literal(&mut chars, opts)?;
                    parts.push(WordPart::ArrayLiteral(elements));
                }
```

In the `'+='` arm (~line 952), insert the same call before ITS `if chars.peek() == Some(&'(')`:

```rust
                // Compound RHS: `name+=(...)`.
                skip_line_continuations(&mut chars);
                if chars.peek() == Some(&'(') {
                    chars.next();
                    let elements = scan_array_literal(&mut chars, opts)?;
                    parts.push(WordPart::ArrayLiteral(elements));
                }
```

(Do NOT modify the subscripted-lvalue peek `name[i]=(…)` ~line 1027 — `a[i]=(…)` is invalid in bash.)

- [ ] **Step 5: Run to confirm PASS + no regression.**

Run: `cargo test --lib array_assignment_with_line_continuation array_append_with_line_continuation backslash_escape_after_eq 2>&1 | tail -10 && cargo test --lib 2>&1 | grep "test result:" | grep -v "0 failed" || echo OK`
Expected: all three PASS; `OK` (no other lib failures). `cargo clippy --lib` clean.

- [ ] **Step 6: Commit.**

```bash
git add src/lexer.rs
git commit -m "$(cat <<'EOF'
v192: skip line continuations before the array-( assignment peek

`arr=\<NL>(a b c)` is an array assignment with a \<NL> line continuation between
`=` and `(`; bash deletes the continuation pre-tokenization. skip_line_continuations
(cloned-cursor lookahead, only consumes \<NL> pairs) runs before the name=( and
name+=( array peeks so the array is detected. Resolves the byobu-ulevel sweep gap.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: bash-diff harness

**Files:**
- Create: `tests/scripts/array_line_continuation_diff_check.sh`

- [ ] **Step 1: Create the harness** (mirrors `tests/scripts/process_sub_diff_check.sh`):

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v192: `name=\<NL>(array)` — a line
# continuation between `=`/`+=` and the array `(`.
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

# the byobu shape: \<NL> between `=` and `(`
check "elem index"    $'arr=\\\n(a b c)\nprintf "%s\\n" "${arr[1]}"'
check "all elems"     $'arr=\\\n(a b c)\nprintf "%s\\n" "${arr[@]}"'
check "count"         $'arr=\\\n(a b c)\necho "${#arr[@]}"'
# append form
check "append"        $'arr=(a); arr+=\\\n(b c)\necho "${arr[2]}"'
# stacked continuations
check "stacked"       $'arr=\\\n\\\n(x y)\necho "${arr[0]}"'
# negative: scalar with continuation (already worked) stays scalar
check "scalar cont"   $'v=\\\nfoo\necho "[$v]"'
# negative: a literal backslash-escape is NOT a continuation
check "escape"        $'v=\\x\necho "[$v]"'
# control: a normal inline array
check "inline array"  'arr=(p q r); echo "${arr[2]}"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 2: Make executable and run.**

Run:
```bash
chmod +x tests/scripts/array_line_continuation_diff_check.sh
cargo build 2>&1 | tail -1
bash tests/scripts/array_line_continuation_diff_check.sh
```
Expected: all `PASS`, `Fail: 0`, exit 0. If a case diverges, STOP and report the exact bash-vs-huck diff (do NOT weaken). Note: the `$'…'` ANSI-C strings embed the literal `\`-newline (`\\\n`) so bash and huck both receive `arr=\`⏎`(a b c)`.

- [ ] **Step 3: Prove non-tautological** (fails pre-fix):

```bash
BASE=$(git merge-base HEAD main)
git worktree add -d /tmp/huck-prefix "$BASE" 2>&1 | tail -1
( cd /tmp/huck-prefix && cargo build 2>&1 | tail -1 )
HUCK_BIN=/tmp/huck-prefix/target/debug/huck bash tests/scripts/array_line_continuation_diff_check.sh | tail -4
git worktree remove --force /tmp/huck-prefix 2>&1 | tail -1
```
Expected: the continuation-array cases (elem index/all elems/count/append/stacked) FAIL pre-fix; the scalar/escape/inline-array controls PASS. Report the count.

- [ ] **Step 4: Commit.**

```bash
git add tests/scripts/array_line_continuation_diff_check.sh
git commit -m "$(cat <<'EOF'
v192: bash-diff harness for name=\<NL>(array) line continuation

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: doc (intentional `${…}` strictness) + parse-sweep + regression + memory

**Files:**
- Modify: `docs/bash-divergences.md` (new Tier-3 intentional entry + count).
- Modify (memory): `project_huck_iterations.md`, `MEMORY.md`.

- [ ] **Step 1: Full test suite (0 failures).**

Run: `cargo test 2>&1 | grep "test result:" | grep -v "0 failed" || echo "ALL GREEN"`
Expected: `ALL GREEN`. (No existing test should encode the old byobu behavior — it was a hard error; if any test asserted the error, update it.)

- [ ] **Step 2: All harnesses + clippy green.**

Run:
```bash
cargo build 2>&1 | tail -1
for s in tests/scripts/*_diff_check.sh; do out=$(bash "$s" 2>&1); echo "$s :: $(echo "$out" | tail -1)"; done | grep -E "Fail: [1-9]" || echo "ALL HARNESSES GREEN"
cargo clippy --all-targets 2>&1 | tail -3
```
Expected: `ALL HARNESSES GREEN`; clippy clean.

- [ ] **Step 3: Parse-sweep payoff.**

Run:
```bash
tools/parse_sweep.sh tools/scripts.tsv tools/parse_results.tsv 2>&1 | tail -10
echo "=== byobu now? ==="; ./target/debug/huck -n /usr/bin/byobu-ulevel; echo "byobu huck-n rc=$?"
echo "=== remaining HUCK_GAP ==="; awk -F'\t' '$3=="HUCK_GAP"{print $6}' tools/parse_results.tsv
```
Expected: `byobu-ulevel` `huck -n` rc 0; HUCK_GAP 3→2; the 2 remaining are both `perf-completion.sh` (`${=1}`); LENIENT/CRASH/TIMEOUT stay 0. NOTE: `tools/parse_results.tsv` is gitignored (regenerated artifact) — do NOT commit it.

- [ ] **Step 4: Document the `${…}` strictness as intentional.**

In `docs/bash-divergences.md`, read the **Tier 3: Intentional divergences** section (line ~90) to learn its entry-ID scheme and find the next free ID. Add an entry:

```markdown
- **<next-ID>: malformed `${…}` rejected at parse, not runtime** — `[intentional]`. huck rejects a malformed parameter expansion (`${}`, `${=1}`, `${ x}`, `${@x}`, `${1abc}`, `${-x}`, `${.}`) at PARSE time (`syntax error: parameter expansion with empty name` / `invalid parameter-expansion modifier`). bash parses these (`bash -n` rc 0) and emits the identical `bad substitution` error only at RUNTIME. The constructs are invalid in bash either way; huck's earlier error is by design (matching `bash -n`'s leniency would require a deferred-runtime-error path to accept syntax that is broken regardless). This is the parse-sweep's remaining `${=1}` ×2 entries (`perf-completion.sh`, a zsh-only word-split form).
```

Bump the **Tier 3** count in the Summary table (line ~32) by 1 (9 → 10). Verify: `grep -n "Intentional (Tier 3)" docs/bash-divergences.md`.

- [ ] **Step 5: Record the iteration in memory.**

Prepend a v192 entry to `project_huck_iterations.md` (newest-first): parse-sweep fix; `name=\<NL>(array)` — the `=`/`+=` array-`(` peek saw the continuation `\`; new `skip_line_continuations` (cloned-cursor lookahead, only `\<NL>` pairs) before the peek; clears byobu-ulevel; HUCK_GAP 3→2. The 2 remaining `${=1}` entries documented `[intentional]` (huck parse-rejects malformed `${…}`; bash defers bad-substitution to runtime; NOT building the deferred-error path). Merge SHA (fill after merge). Update the `MEMORY.md` index line + the parse-sweep progression line (HUCK_GAP …→2, all remaining intentional → **0 real gaps**).

- [ ] **Step 6: Commit memory + doc.**

```bash
git add docs/bash-divergences.md \
        /home/john/.claude/projects/-home-john-projects-shuck/memory/project_huck_iterations.md \
        /home/john/.claude/projects/-home-john-projects-shuck/memory/MEMORY.md
git commit -m "v192: document \${...} strictness intentional + record iteration

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

(If the controller prefers to handle memory post-merge, do only the `bash-divergences.md` part here and report.)

---

## Report-back (Task 3)

Report: STATUS, all commit SHAs, full `cargo test` summary, harness results (incl. the new `array_line_continuation_diff_check.sh` + its pre-fix FAIL count), clippy status, the parse-sweep result (byobu rc + new HUCK_GAP vs 3 + the 2 remaining files), and what you added to `bash-divergences.md` (the new Tier-3 ID + count bump).
