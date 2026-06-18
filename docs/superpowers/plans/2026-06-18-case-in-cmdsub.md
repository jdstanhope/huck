# v186: `case` statements inside `$(…)` (case-aware `scan_cmdsub_body`) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give `scan_cmdsub_body` (the `$()` close-finder) `case`-statement awareness so a bare case-pattern `)` (e.g. `mlxsw_spectrum)`) at paren-depth 0 is treated as a pattern terminator, not the closing `)` of the substitution — clearing the parse-sweep `case`-in-cmdsub cluster (`mlxsw_lib.sh` ×2).

**Architecture:** A bounded state machine added to `scan_cmdsub_body` (`src/lexer.rs`), mirroring bash's `LEX_INCASE`: track `case_depth` by recognizing bare `case`/`esac` words in COMMAND position (`cmd_pos`, computed from command separators + introducer keywords so `$(echo case)` doesn't false-positive). While `case_depth > 0`, a paren-depth-0 `)` is a pattern terminator. This is the pragmatic alternative to fully parse-driven `$()` (spiked, deferred — it has a perf wall on huck's batch tokenizer).

**Tech Stack:** Rust (huck shell). Verification via `cargo test`, `tests/scripts/*_diff_check.sh`, and `tools/parse_sweep.sh`.

**Spec:** `docs/superpowers/specs/2026-06-18-case-in-cmdsub-design.md`

**Branch:** `v186-case-in-cmdsub` (from `main`; do NOT implement on `main`).

---

## Confirmed bash behavior (the contract; each inside `"$( … )"`)

| Fragment | bash | huck (today) |
|---|---|---|
| `case $y in a) echo hit;; *) echo no;; esac` | `hit` | ★ unterminated 'case' |
| `echo case` (arg) | `case` | `case` (unchanged) |
| `if true; then case $y in a) echo T;; esac; fi` | `T` | ★ |
| `case $y in a) case $y in a) echo deep;; esac;; esac` | `deep` | ★ |
| `case $y in a\|b) echo alt;; esac` | `alt` | ★ |
| `case $y in a) echo A;;& *) echo B;; esac` | `A`⏎`B` | ★ |
| `case $y in (a) echo p;; esac` | `p` | `p` (parens balance, unchanged) |
| `echo x \| grep case \|\| echo none` | `none` | `none` (unchanged) |
| `C=spectrum2; $(case $C in spectrum) echo 1;; spectrum*) echo ${C#spectrum};; esac)` | `2` | ★ |

`★` = currently `syntax error in command substitution: unterminated 'case'`.

---

### Task 1: case-aware `scan_cmdsub_body`

**Files:**
- Modify: `src/lexer.rs` — replace the body of `scan_cmdsub_body`
- Test: `src/lexer.rs` (`mod tests`, near the `scan_cmdsub_body_*` tests)

**Context:** `scan_cmdsub_body(chars, out, unterminated)` collects a `$(…)` body verbatim into `out` while finding the matching `)` (paren-depth + quote/escape + v183 comment aware). It is the single `$()` close-finder used by `scan_paren_substitution`, `consume_paren_cmdsub_verbatim`, and `split_modifier_operand`. The 4 existing `scan_cmdsub_body_*` tests (basic / nested-arith / quoted-paren / unterminated) and the v183 comment behavior must stay green.

- [ ] **Step 1: Create the branch**

```bash
cd /home/john/projects/shuck
git checkout main && git checkout -b v186-case-in-cmdsub
```

- [ ] **Step 2: Replace `scan_cmdsub_body` with the case-aware version**

In `src/lexer.rs`, replace the ENTIRE `scan_cmdsub_body` function with:

```rust
fn scan_cmdsub_body(
    chars: &mut CharCursor<'_>,
    out: &mut String,
    unterminated: LexError,
) -> Result<(), LexError> {
    let mut depth: usize = 0;
    // `#` comment recognition (v183): a `#` at a word boundary starts a comment.
    let mut at_boundary = true;
    // v186: `case … esac` state so a BARE case-pattern `)` at paren-depth 0 is a
    // pattern terminator, not the cmdsub close. `cmd_pos` = the next word begins
    // at a COMMAND position (so a bare `case`/`esac` there is a keyword, but
    // `echo case` / `grep case` are not). `word` accumulates the current BARE
    // word (identifier chars); `word_bare` goes false once a quote/`$`/other char
    // makes the word not a bare keyword. KNOWN LIMITATION: a pattern LITERALLY
    // named `case`/`esac` after `;;` (where `cmd_pos` is true), or a `VAR=val case`
    // prefix-assignment case, is mis-counted — pathological, matches bash's own
    // LEX_INCASE edges, and absent from real code.
    let mut case_depth: usize = 0;
    let mut cmd_pos = true;
    let mut word = String::new();
    let mut word_bare = true;

    // End the current word: recognise a bare `case`/`esac` keyword at command
    // position; return whether it was a command-introducer keyword (for the
    // space transition). Resets `word`/`word_bare`.
    macro_rules! end_word {
        () => {{
            let introducer = word_bare
                && matches!(
                    word.as_str(),
                    "if" | "then" | "elif" | "else" | "while" | "until" | "do"
                );
            if word_bare && cmd_pos {
                if word == "case" {
                    case_depth += 1;
                } else if word == "esac" {
                    case_depth = case_depth.saturating_sub(1);
                }
            }
            word.clear();
            word_bare = true;
            introducer
        }};
    }

    loop {
        match chars.next() {
            None => return Err(unterminated),
            Some('#') if at_boundary => {
                end_word!();
                // Word-start comment to end-of-line: keep it VERBATIM in `out`
                // (re-tokenized + stripped later) so its `)` is not counted.
                out.push('#');
                while let Some(&c) = chars.peek() {
                    if c == '\n' {
                        break;
                    }
                    out.push(c);
                    chars.next();
                }
                // the trailing newline (next char) restores at_boundary + cmd_pos
            }
            // The cmdsub close: depth-0 `)` only when NOT inside a `case`.
            Some(')') if depth == 0 && case_depth == 0 => return Ok(()),
            // depth-0 `)` inside a `case` is a pattern terminator — keep scanning;
            // a clause body (commands) follows.
            Some(')') if depth == 0 => {
                end_word!();
                out.push(')');
                at_boundary = true;
                cmd_pos = true;
            }
            Some(')') => {
                end_word!();
                depth -= 1;
                out.push(')');
                at_boundary = true;
                cmd_pos = true;
            }
            Some('(') => {
                end_word!();
                depth += 1;
                out.push('(');
                at_boundary = true;
                cmd_pos = true;
            }
            Some('\\') => {
                word_bare = false;
                out.push('\\');
                match chars.next() {
                    Some(c) => out.push(c),
                    None => return Err(unterminated),
                }
                at_boundary = false;
            }
            Some('\'') => {
                word_bare = false;
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
                word_bare = false;
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
                if c.is_ascii_alphanumeric() || c == '_' {
                    // identifier char: extend the current bare word.
                    if word_bare {
                        word.push(c);
                    }
                    at_boundary = false;
                } else if c.is_whitespace() {
                    let introducer = end_word!();
                    // whitespace keeps command position only after an introducer
                    // keyword (`then case` → keyword; `echo case` → arg).
                    cmd_pos = introducer;
                    at_boundary = true;
                } else if matches!(c, ';' | '&' | '|') {
                    end_word!();
                    cmd_pos = true;
                    at_boundary = true;
                } else if matches!(c, '{' | '}') {
                    end_word!();
                    cmd_pos = c == '{';
                    at_boundary = false;
                } else if matches!(c, '<' | '>') {
                    end_word!();
                    cmd_pos = false; // redirect — same command
                    at_boundary = true;
                } else {
                    // `$`, `-`, `.`, `*`, `?`, `=`, `~`, backtick, etc.: continues
                    // / starts a word that is not a bare keyword.
                    word_bare = false;
                    at_boundary = false;
                }
            }
        }
    }
}
```

- [ ] **Step 3: Build**

Run: `cargo build 2>&1 | tail -5`
Expected: compiles clean.

- [ ] **Step 4: Manual behavior check against bash**

Run:

```bash
B=target/debug/huck
chk() { printf '%b\n' "$2" > /tmp/v186.sh
  printf '%-22s huck=<%s> bash=<%s>\n' "$1" \
    "$($B /tmp/v186.sh 2>&1 | tr '\n' '|')" "$(bash --norc /tmp/v186.sh 2>&1 | tr '\n' '|')"; }
chk 'case in cmdsub'  'y=a; echo "$(case $y in a) echo hit;; *) echo no;; esac)"'
chk 'echo case (arg)' 'echo "$(echo case)"'
chk 'if;then case'    'y=a; echo "$(if true; then case $y in a) echo T;; esac; fi)"'
chk 'nested case'     'y=a; echo "$(case $y in a) case $y in a) echo deep;; esac;; esac)"'
chk 'alternation'     'y=b; echo "$(case $y in a|b) echo alt;; esac)"'
chk ';;& fallthru'    'y=a; echo "$(case $y in a) echo A;;& *) echo B;; esac)"'
chk 'paren pattern'   'y=a; echo "$(case $y in (a) echo p;; esac)"'
chk 'grep case arg'   'echo "$(echo x | grep case || echo none)"'
chk 'mlxsw shape'     'C=spectrum2; echo "$(case $C in spectrum) echo 1;; spectrum*) echo ${C#spectrum};; esac)"'
chk 'control no-case' 'echo "$(echo a; echo b)"'
```

Expected (every line huck byte-equal to bash): `hit`, `case`, `T`, `deep`, `alt`, `A|B`, `p`, `none`, `2`, `a|b`.

- [ ] **Step 5: Add lexer unit tests**

In `src/lexer.rs` `mod tests`, near the `scan_cmdsub_body_*` tests, add:

```rust
    #[test]
    fn scan_cmdsub_body_case_pattern_paren_not_close() {
        // v186: a bare case-pattern `)` (depth 0) is a pattern terminator, not the
        // cmdsub close. Stops at the FINAL `)` after `esac`.
        let mut chars = CharCursor::new("case $y in a) echo hit;; esac)rest");
        let mut out = String::new();
        scan_cmdsub_body(&mut chars, &mut out, LexError::UnterminatedSubstitution).unwrap();
        assert_eq!(out, "case $y in a) echo hit;; esac");
        assert_eq!(chars.next(), Some('r'));
    }

    #[test]
    fn scan_cmdsub_body_case_as_arg_is_not_keyword() {
        // v186: `case` NOT in command position (an argument) is a plain word — the
        // first `)` closes.
        let mut chars = CharCursor::new("echo case)rest");
        let mut out = String::new();
        scan_cmdsub_body(&mut chars, &mut out, LexError::UnterminatedBrace).unwrap();
        assert_eq!(out, "echo case");
        assert_eq!(chars.next(), Some('r'));
    }

    #[test]
    fn scan_cmdsub_body_nested_case() {
        // v186: nested `case … esac` — only the FINAL `)` closes.
        let mut chars = CharCursor::new("case $y in a) case $y in a) :;; esac;; esac)X");
        let mut out = String::new();
        scan_cmdsub_body(&mut chars, &mut out, LexError::UnterminatedBrace).unwrap();
        assert_eq!(out, "case $y in a) case $y in a) :;; esac;; esac");
        assert_eq!(chars.next(), Some('X'));
    }
```

- [ ] **Step 6: Run the new unit tests + the existing cmdsub_body tests**

Run: `cargo test --lib scan_cmdsub_body 2>&1 | tail -15`
Expected: the 3 new tests + the 4 existing (`_basic_consumes_through_close_paren`, `_balances_nested_and_arith`, `_skips_quoted_paren`, `_unterminated_uses_passed_error`) + the v183 comment tests all pass, 0 failed. If any EXISTING test fails, STOP and report.

- [ ] **Step 7: clippy**

Run: `cargo clippy 2>&1 | tail -5`
Expected: no new warnings.

- [ ] **Step 8: Commit**

```bash
git add src/lexer.rs
git commit -m "$(cat <<'EOF'
v186: case-aware scan_cmdsub_body (case statements inside $())

scan_cmdsub_body now tracks `case … esac` state (mirroring bash's LEX_INCASE):
it recognizes bare `case`/`esac` in command position (via cmd_pos: command
separators + introducer keywords, so `$(echo case)` is unaffected) and, while
inside a case, treats a paren-depth-0 `)` as a pattern terminator rather than
the cmdsub close. Clears the parse-sweep case-in-cmdsub cluster (mlxsw_lib.sh).
A pattern literally named case/esac after `;;` is a documented pathological edge.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

## Report back
STATUS, commit SHA, the Step 4 table (all must match), Step 6 result (incl. existing tests green), any concerns. If a code site doesn't match above, find it by surrounding code and report.

---

### Task 2: Bash-diff harness `case_in_cmdsub_diff_check.sh`

**Files:**
- Create: `tests/scripts/case_in_cmdsub_diff_check.sh`

**Context:** Harnesses pipe each fragment to bash AND huck and assert byte-identical output. All cases print clean stdout, rc 0 in bash. Model: `tests/scripts/cmdsub_comment_diff_check.sh`.

- [ ] **Step 1: Write the harness**

Create `tests/scripts/case_in_cmdsub_diff_check.sh`:

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v186: `case … esac` statements inside
# `$( … )`. A bare case-pattern `)` must not close the substitution; `case` as an
# argument is a plain word. Kernel mlxsw_lib.sh hit this. rc 0 in bash → compare
# full stdout+exit.
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

check "case in cmdsub"        'y=a; echo "$(case $y in a) echo hit;; *) echo no;; esac)"'
check "case as arg"           'echo "$(echo case)"'
check "if then case"          'y=a; echo "$(if true; then case $y in a) echo T;; esac; fi)"'
check "nested case"           'y=a; echo "$(case $y in a) case $y in a) echo deep;; esac;; esac)"'
check "alternation pattern"   'y=b; echo "$(case $y in a|b) echo alt;; esac)"'
check "fallthrough ;;&"       'y=a; echo "$(case $y in a) echo A;;& *) echo B;; esac)"'
check "parenthesized pattern" 'y=a; echo "$(case $y in (a) echo p;; esac)"'
check "case word after pipe"  'echo "$(echo x | grep case || echo none)"'
check "mlxsw real shape"      'C=spectrum2; echo "$(case $C in spectrum) echo 1;; spectrum*) echo ${C#spectrum};; esac)"'
check "case clause has cmdsub" 'y=a; echo "$(case $y in a) echo $(echo inner);; esac)"'
check "control no case"       'echo "$(echo a; echo b)"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 2: Make executable, build, run**

Run:

```bash
chmod +x tests/scripts/case_in_cmdsub_diff_check.sh
cargo build 2>&1 | tail -2
bash tests/scripts/case_in_cmdsub_diff_check.sh
```

Expected: `Total: 11, Pass: 11, Fail: 0`, exit 0. If a case FAILs, STOP and report (do NOT weaken).

- [ ] **Step 3: Prove the harness is non-tautological**

```bash
git worktree add /tmp/huck-prefix main 2>&1 | tail -1
( cd /tmp/huck-prefix && cargo build 2>&1 | tail -1 )
HUCK_BIN=/tmp/huck-prefix/target/debug/huck bash tests/scripts/case_in_cmdsub_diff_check.sh; echo "prefix-exit=$?"
git worktree remove --force /tmp/huck-prefix
```

Expected: the `case`-bearing cases FAIL against the pre-fix binary (unterminated 'case'); the `case as arg` / `parenthesized pattern` / `case word after pipe` / `control no case` controls pass pre-fix; prefix-exit=1. If it PASSES pre-fix, the harness is tautological — STOP and report. (`main` is the pre-v186 baseline.)

- [ ] **Step 4: Commit**

```bash
git add tests/scripts/case_in_cmdsub_diff_check.sh
git commit -m "$(cat <<'EOF'
v186: bash-diff harness for case statements inside $()

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

## Report back
STATUS, commit SHA, Step 2 Total/Pass/Fail line, Step 3 prefix-exit + which cases failed pre-fix (proving non-tautology).

---

### Task 3: Parse-sweep payoff and regression

**Files:**
- Modify (only if found): old-behavior tests from the up-front grep
- Run: full `cargo test`, all `tests/scripts/*_diff_check.sh`, `tools/parse_sweep.sh`

- [ ] **Step 1: Up-front grep**

Run:

```bash
grep -rn "scan_cmdsub_body\|unterminated.*case\|in command substitution" src/ tests/ | grep -iv "fn scan_cmdsub_body" | head -30
```

Classify any hit UPDATE (encodes the old `case`-closes-early behavior) vs LEAVE (the v186 new tests / unrelated). Report each. (None expected to need UPDATE — the change only affects `$(case …)` which previously errored.)

- [ ] **Step 2: Full test suite**

Run: `cargo test 2>&1 | tail -30`
Expected: 0 failures across all unit + integration binaries. A failure on a cmdsub assertion: verify against bash; if it encoded the old behavior, UPDATE; else STOP and report.

- [ ] **Step 3: Whole bash-diff harness suite**

Run:

```bash
cargo build 2>&1 | tail -1
for s in tests/scripts/*_diff_check.sh; do
  out=$(bash "$s" 2>&1); rc=$?
  printf '%s rc=%s %s\n' "$s" "$rc" "$(echo "$out" | tail -1)"
done | grep -v 'Fail: 0' || echo "ALL HARNESSES GREEN"
```

Expected: `ALL HARNESSES GREEN` (esp. `cmdsub_comment_diff_check.sh` from v183 stays green — the case state must not disturb comment handling).

- [ ] **Step 4: Parse-sweep payoff**

```bash
tools/parse_sweep.sh tools/scripts.tsv tools/parse_results.tsv 2>&1 | tail -15
for F in /usr/src/linux-headers-6.8.0-110/tools/testing/selftests/drivers/net/mlxsw/mlxsw_lib.sh \
  /usr/src/linux-headers-6.8.0-124/tools/testing/selftests/drivers/net/mlxsw/mlxsw_lib.sh; do
  echo "=== $F ==="; ./target/debug/huck -n "$F"; echo "huck-n rc=$?"; bash -n "$F"; echo "bash-n rc=$?"
done
echo "=== remaining case gaps? ==="
awk -F'\t' '$3=="HUCK_GAP" && tolower($7) ~ /case/{print $6}' tools/parse_results.tsv || echo "(none)"
```

Expected: both `mlxsw_lib.sh` copies `huck -n` SILENT rc 0 = bash; no `case` `HUCK_GAP` rows remain. Report the new HUCK_GAP vs the 8 baseline; `HUCK_LENIENT`/`HUCK_CRASH`/**`HUCK_TIMEOUT`** stay 0 (the case-state-machine is O(N), unlike the spiked parse-driven approach). Note any remaining row that's a DIFFERENT construct.

- [ ] **Step 5: Commit (only if Step 1/2 changed a file)**

If a legitimate test update was needed, commit it; else state "no commit needed" (Steps 2-4 are verification only).

## Report back
STATUS, commit SHA (or "no commit needed"), the Step 1 grep classification, Step 2 cargo test summary, Step 3 result, Step 4 per-file rc + new HUCK_GAP vs 8 + confirmation no case rows remain AND HUCK_TIMEOUT is 0.

---

## Final review (whole-branch, before merge)

- [ ] **Step 1: Review the full diff** — `git diff main...HEAD`. Confirm only `scan_cmdsub_body` + the 3 lexer unit tests, plus the new harness (and any Task-3 residual), changed.
- [ ] **Step 2: Scope boundary** — only `scan_cmdsub_body` changed; the `$((`/`((`/`scan_arith_block` paths, the runtime, and the parse-driven rewrite (deferred) untouched; no `bash-divergences.md` change.
- [ ] **Step 3: Hand off** — project merge ritual (AskUserQuestion before merging to main): merge `--no-ff`, push, delete the local branch, record v186 in `project_huck_iterations.md` + `MEMORY.md`, update the backlog note.

---

## Self-Review (plan vs spec)

**1. Spec coverage:**
- case-state machine in `scan_cmdsub_body` (case_depth, cmd_pos, word/word_bare, the depth-0-`)`-with-case_depth split, introducer keywords) → Task 1 Step 2 (full function). ✓
- Lexer unit tests (pattern-`)` not close; case-as-arg; nested) → Task 1 Step 5. ✓
- New harness with the 8 contract cases + mlxsw shape + clause-with-cmdsub + control → Task 2. ✓
- Parse-sweep: mlxsw_lib ×2 clear, no case rows, **HUCK_TIMEOUT 0** → Task 3 Step 4. ✓
- Full `cargo test` + up-front grep + comment harness stays green → Task 3 Steps 1-3 + Task 1 Step 6. ✓
- All `*_diff_check.sh` green + clippy → Task 3 Step 3 + Task 1 Step 7. ✓
- No `bash-divergences.md` change; documented pathological edge in the function comment → Task 1 Step 2 comment + Final review. ✓

**2. Placeholder scan:** No TBD/TODO. The full replacement function and complete test/harness bodies are given; every run step has expected output.

**3. Type consistency:** `scan_cmdsub_body(chars, out, unterminated) -> Result<(), LexError>` signature unchanged (body only). The `end_word!` macro mutates the in-scope `case_depth`/`word`/`word_bare`/`cmd_pos` (macro_rules defined in-fn can reference fn locals). The 4 existing `scan_cmdsub_body_*` tests pin its non-case behavior; the v183 `at_boundary`/comment handling is preserved in every arm.

**Resolved during planning:** traced the mlxsw real shape + all 8 contract cases against the state machine; confirmed `$(echo case)` and `grep case` stay non-keyword (cmd_pos false after a plain word + space); confirmed nested case via the counter; documented the pattern-named-`case`-after-`;;` and `VAR=val case` edges as known/pathological.
