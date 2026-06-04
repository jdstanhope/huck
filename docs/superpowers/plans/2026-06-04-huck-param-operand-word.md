# huck v84 — parse `${…}` operands as words Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax. Fresh subagent per task with spec-compliance + code-quality review between tasks.

**Goal:** Fix the `${var:+(…)}` / `${var:-(…)}` parse bug — parse a brace-modifier operand as a WORD (expansions + quoting active; metacharacters `(` `)` `|` `;` `&` `<` `>` literal; whitespace kept as splittable unquoted literal) instead of running the command tokenizer on it.

**Architecture:** Replace `parse_braced_operand`'s `tokenize()`-then-reject-operators body (`src/lexer.rs:1300`) with a single-pass char-walk modeled on `scan_expanding_body_line`, reusing `read_dollar_expansion` / `scan_backtick_substitution` / `flush_body_literal`, and adding `'…'`/`"…"` span handling. One shared function fixes all operand-bearing modifiers (`:-`/`:=`/`:?`/`:+`, `${v/pat/repl}`, `${v:off:len}`).

**Tech Stack:** Rust 1.85+, no new dependencies.

**Spec:** `docs/superpowers/specs/2026-06-04-huck-param-operand-word-design.md` (read it — root cause + bash semantics).

**Branch:** `v84-param-operand` (create from `main` in Preamble).

**Commit trailer (every commit):**
```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Preamble P.1: Branch setup

- [ ] **Step 1:** `git status && git rev-parse --abbrev-ref HEAD` → clean tree on `main`.
- [ ] **Step 2:** `git checkout -b v84-param-operand`.
- [ ] **Step 3:** Baseline: `cargo test --quiet 2>&1 | grep -E "^test result" | awk '{s+=$4} END{print "Baseline:", s}'` → expect **2368**.
- [ ] **Step 4:** `cargo clippy --all-targets 2>&1 | tail -2` → clean.

---

## File-structure map

| File | Responsibility | Tasks |
|------|----------------|-------|
| `src/lexer.rs` | rewrite `parse_braced_operand` to a word char-walk; update 2 operand unit tests + add new ones; remove `InvalidBraceOperand` if unused | 1 |
| `tests/param_operand_integration.rs` | NEW. Binary-driven integration tests | 2 |
| `tests/scripts/param_operand_diff_check.sh` | NEW. huck's 11th bash-diff harness | 2 |
| `docs/bash-divergences.md`, `README.md` | fix entry; changelog; summary stamp; README v84 row | 2 |

---

## Task 1: Rewrite `parse_braced_operand` as a word parser

**Files:**
- Modify: `src/lexer.rs` — rewrite `parse_braced_operand` (~line 1300); update/add unit tests; remove `InvalidBraceOperand` if unused.

- [ ] **Step 1: Write the failing unit tests**

Add to the lexer tests module (near the existing `parse_braced_operand` tests, ~line 3713). Use the existing test helpers' style; these assert the FLATTENED literal text of the returned `Word` via a small local helper (add it if one doesn't already exist):

```rust
// Local test helper: concatenate the literal text of a Word's parts
// (expansions render as a placeholder so structure tests stay simple).
fn operand_lits(w: &crate::command::Word) -> String {
    let mut s = String::new();
    for p in &w.0 {
        match p {
            WordPart::Literal { text, .. } => s.push_str(text),
            WordPart::Var { name, .. } => { s.push('$'); s.push_str(name); }
            _ => s.push('§'), // any other expansion part
        }
    }
    s
}

#[test]
fn operand_parens_are_literal() {
    assert_eq!(operand_lits(&parse_braced_operand("(a)").unwrap()), "(a)");
}

#[test]
fn operand_pipe_semicolon_amp_are_literal() {
    assert_eq!(operand_lits(&parse_braced_operand("a|b;c&d").unwrap()), "a|b;c&d");
    assert_eq!(operand_lits(&parse_braced_operand("a(b)c").unwrap()), "a(b)c");
}

#[test]
fn operand_expansion_with_parens() {
    // `($x)` → literal "(", Var x, literal ")"
    let w = parse_braced_operand("($x)").unwrap();
    assert_eq!(operand_lits(&w), "($x)");
}

#[test]
fn operand_single_quote_is_literal_span() {
    // '|;()' inside single quotes → one quoted literal "|;()"
    let w = parse_braced_operand("'|;()'").unwrap();
    assert_eq!(operand_lits(&w), "|;()");
    assert!(matches!(w.0.as_slice(), [WordPart::Literal { quoted: true, .. }]));
}

#[test]
fn operand_double_quote_keeps_expansion() {
    // "a $x b" → quoted literal "a ", Var x (quoted), quoted literal " b"
    let w = parse_braced_operand("\"a $x b\"").unwrap();
    assert_eq!(operand_lits(&w), "a $x b");
}

#[test]
fn operand_nested_brace() {
    let w = parse_braced_operand("${y:-z}").unwrap();
    assert!(matches!(w.0.as_slice(), [WordPart::ParamExpansion { .. }]));
}

#[test]
fn operand_empty_is_empty_word() {
    assert!(parse_braced_operand("").unwrap().0.is_empty());
}

#[test]
fn operand_plain_words_split_friendly() {
    // "foo bar" → unquoted literal "foo bar" (one run; splits downstream).
    let w = parse_braced_operand("foo bar").unwrap();
    assert_eq!(operand_lits(&w), "foo bar");
    assert!(w.0.iter().all(|p| matches!(p, WordPart::Literal { quoted: false, .. })));
}
```

Also UPDATE the two pre-existing tests that assert the OLD behavior:
- The test asserting `parse_braced_operand("foo | bar")` is `Err(LexError::InvalidBraceOperand)` (~line 3729) → change to assert `Ok` with `operand_lits(...) == "foo | bar"`.
- The `"foo bar"` test (~line 3720) asserting a specific multi-part (foo/space/bar) structure → replace with the `operand_plain_words_split_friendly` assertion above (or update it to check `operand_lits == "foo bar"`).

- [ ] **Step 2: Run, expect failure**

Run: `cargo test --quiet --bin huck operand 2>&1 | tail -8`
Expected: the new `operand_*` tests FAIL (current code rejects `(`/`|`/`;` operands with `InvalidBraceOperand`).

- [ ] **Step 3: Rewrite `parse_braced_operand`**

Replace the entire body of `parse_braced_operand` (`src/lexer.rs`, ~line 1300-1324) with a word char-walk. It reuses `read_dollar_expansion`, `scan_backtick_substitution`, and `flush_body_literal` (all already in `src/lexer.rs`):

```rust
/// Parses a brace-modifier operand BODY (already extracted up to the matching
/// `}` by `scan_braced_operand`) as a single WORD: `$…` / `` `…` `` / quotes are
/// expansions/quoting; ALL other characters — including shell metacharacters
/// `(` `)` `|` `;` `&` `<` `>` and whitespace — are LITERAL. Unquoted literal
/// text (incl. spaces) is emitted as `quoted: false` Literal parts so an
/// unquoted `${x:-a b}` still field-splits to `a` `b` downstream; inner quoted
/// spans produce `quoted: true` parts that suppress splitting. Matches bash:
/// the operand of `:-`/`:=`/`:?`/`:+` (and substitution/substring operands) is a
/// word, not a command.
fn parse_braced_operand(body: &str) -> Result<Word, LexError> {
    let mut chars = body.chars().peekable();
    let mut parts: Vec<WordPart> = Vec::new();
    let mut cur = String::new();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                // Backslash escapes the next char as an (unquoted) literal.
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            '$' => {
                flush_body_literal(&mut parts, &mut cur, false);
                read_dollar_expansion(&mut chars, &mut parts, false)?;
            }
            '`' => {
                flush_body_literal(&mut parts, &mut cur, false);
                let sequence = scan_backtick_substitution(&mut chars)?;
                parts.push(WordPart::CommandSub { sequence, quoted: false });
            }
            '\'' => {
                // Single-quoted span: everything literal until the next `'`.
                flush_body_literal(&mut parts, &mut cur, false);
                let mut s = String::new();
                loop {
                    match chars.next() {
                        None => return Err(LexError::UnterminatedQuote),
                        Some('\'') => break,
                        Some(ch) => s.push(ch),
                    }
                }
                parts.push(WordPart::Literal { text: s, quoted: true });
            }
            '"' => {
                // Double-quoted span: $/`/\ active; everything else literal (quoted).
                flush_body_literal(&mut parts, &mut cur, false);
                loop {
                    match chars.next() {
                        None => return Err(LexError::UnterminatedQuote),
                        Some('"') => break,
                        Some('\\') => match chars.peek().copied() {
                            Some(e @ ('$' | '`' | '"' | '\\')) => {
                                chars.next();
                                flush_body_literal(&mut parts, &mut cur, true);
                                parts.push(WordPart::Literal { text: e.to_string(), quoted: true });
                            }
                            _ => cur.push('\\'),
                        },
                        Some('$') => {
                            flush_body_literal(&mut parts, &mut cur, true);
                            read_dollar_expansion(&mut chars, &mut parts, true)?;
                        }
                        Some('`') => {
                            flush_body_literal(&mut parts, &mut cur, true);
                            let sequence = scan_backtick_substitution(&mut chars)?;
                            parts.push(WordPart::CommandSub { sequence, quoted: true });
                        }
                        Some(ch) => cur.push(ch),
                    }
                }
                flush_body_literal(&mut parts, &mut cur, true);
            }
            other => cur.push(other),
        }
    }
    flush_body_literal(&mut parts, &mut cur, false);
    Ok(Word(parts))
}
```

> `flush_body_literal(parts, &mut cur, quoted)` already exists (~line 963) — it pushes a `Literal { text: take(cur), quoted }` when `cur` is non-empty. `read_dollar_expansion(&mut chars, &mut parts, quoted)` and `scan_backtick_substitution(&mut chars)` are the same helpers `scan_expanding_body_line` uses. Confirm the `Word` tuple field access (`w.0`) matches the codebase (`pub struct Word(pub Vec<WordPart>)` — adjust if it's a named field).

- [ ] **Step 4: Run the operand tests, expect pass**

Run: `cargo test --quiet --bin huck operand 2>&1 | tail -10` → all pass (new + the 2 updated).

- [ ] **Step 5: Remove `InvalidBraceOperand` if now unused**

Run: `grep -rn "InvalidBraceOperand" src/`. If the only remaining references are the enum variant + its `Display`/formatting arm + (now-removed) tests, delete the variant and its `Display` arm (search `src/shell.rs`/`src/lexer.rs` for where `LexError` is rendered). If `cargo build` then complains about a missing match arm, that confirms the render site — remove that arm too. If any non-test code still produces it, leave it. Re-build clean.

- [ ] **Step 6: Build + full suite + clippy + smoke**

```bash
cargo build 2>&1 | tail -3
cargo test --quiet 2>&1 | grep -E "^test result" | awk '{p+=$4;f+=$6} END{print "PASS="p" FAIL="f}'
cargo clippy --all-targets 2>&1 | tail -2
# smoke: the Debian PS1 operand + metachar operands
printf 'x=v\necho "[${x:+($x)}]"\nunset y\necho "[${y:-(a|b;c)}]"\n' | ./target/debug/huck
```
Expected: FAIL=0 (2368 baseline + new tests, minus none). Smoke prints `[(v)]` then `[(a|b;c)]`. (Verify the smoke matches `bash -c '...'`.)

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
v84 task 1: parse ${...} operands as words, not commands

parse_braced_operand no longer runs the command tokenizer on the operand
(which rejected `(`/`|`/`;`/etc. as operators). It now walks the operand body
as a single word: $.../`...`/quotes are expansions/quoting; all other
metacharacters are literal; unquoted whitespace stays splittable. Fixes
${var:+(...)} / ${var:-(...)} (e.g. the stock Debian PS1
${debian_chroot:+($debian_chroot)}) and, via the shared function, substitution
patterns/replacements and parenthesized substring offsets. InvalidBraceOperand
removed (no longer produced). Updated 2 operand tests + added 8.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Integration tests + bash-diff harness + docs

**Files:**
- Create: `tests/param_operand_integration.rs`, `tests/scripts/param_operand_diff_check.sh` (+x).
- Modify: `docs/bash-divergences.md`, `README.md`.

- [ ] **Step 1: Integration tests** (`tests/param_operand_integration.rs`; mirror an existing `tests/*_integration.rs` spawn helper):

```rust
//! Integration tests for v84: ${...} operands parse as words (metachars literal).
use std::io::Write;
use std::process::{Command, Stdio};

fn run(script: &str) -> (String, i32) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_huck"))
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().expect("spawn huck");
    child.stdin.as_mut().unwrap().write_all(script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let o = child.wait_with_output().unwrap();
    (String::from_utf8_lossy(&o.stdout).into(), o.status.code().unwrap_or(-1))
}

#[test]
fn alt_operand_with_parens_and_expansion() {
    assert_eq!(run("x=v\necho \"[${x:+($x)}]\"\n").0, "[(v)]\n");
    assert_eq!(run("unset y\necho \"[${y:+($y)}]\"\n").0, "[]\n");
}

#[test]
fn default_operand_with_metachars_literal() {
    assert_eq!(run("unset y\necho \"[${y:-(a|b;c)}]\"\n").0, "[(a|b;c)]\n");
}

#[test]
fn debian_ps1_line_parses() {
    // The exact construct from the stock Debian ~/.bashrc PS1.
    let (out, code) = run("debian_chroot=\nPS1=\"${debian_chroot:+($debian_chroot)}\\u@\\h\"\necho ok\n");
    assert_eq!(code, 0);
    assert!(out.contains("ok"), "stdout: {out:?}");
}

#[test]
fn default_operand_unquoted_splits() {
    // unquoted ${y:-a b c} field-splits into 3 args
    assert_eq!(run("unset y\nfor w in ${y:-a b c}; do printf '%s|' \"$w\"; done; echo\n").0, "a|b|c|\n");
}

#[test]
fn default_operand_quoted_stays_one() {
    assert_eq!(run("unset y\nfor w in \"${y:-a b c}\"; do printf '%s|' \"$w\"; done; echo\n").0, "a b c|\n");
}

#[test]
fn substitution_pattern_with_parens() {
    assert_eq!(run("v='a(b)c'\necho \"${v/(b)/X}\"\n").0, "aXc\n");
}

#[test]
fn substring_offset_parenthesized_arith() {
    assert_eq!(run("v=abcdef\necho \"${v:(1+1):2}\"\n").0, "cd\n");
}
```
Run: `cargo test --test param_operand_integration 2>&1 | grep -E "^test result"` → all pass (7). If `substring_offset_parenthesized_arith` fails, check that the arith evaluator accepts the parenthesized offset string the operand now yields; if huck's substring offset doesn't support a leading `(`, drop that one test and note it (the core fix — literal metachars in word operands — is what matters).

- [ ] **Step 2: bash-diff harness** `tests/scripts/param_operand_diff_check.sh` (huck's 11th; mirror `tests/scripts/loop_levels_diff_check.sh`):

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v84: ${...} operands parse as words.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
check "alt parens+expansion"  'x=v; echo "[${x:+($x)}]"'
check "alt unset"             'unset y; echo "[${y:+($y)}]"'
check "default metachars"     'unset y; echo "[${y:-(a|b;c)}]"'
check "default unquoted split" 'unset y; for w in ${y:-a b c}; do printf "%s|" "$w"; done; echo'
check "default quoted one"    'unset y; for w in "${y:-a b c}"; do printf "%s|" "$w"; done; echo'
check "single-quoted operand" 'unset y; echo "[${y:-|;()}]"'
check "debian PS1 operand"    'debian_chroot=; PS1="${debian_chroot:+($debian_chroot)}x"; echo "$PS1"'
check "subst pattern parens"  'v="a(b)c"; echo "${v/(b)/X}"'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```
`chmod +x tests/scripts/param_operand_diff_check.sh`.

- [ ] **Step 3: Build + run harness; iterate to all-pass**

```bash
cargo build --quiet
tests/scripts/param_operand_diff_check.sh
```
Expected: `Total: 8, Pass: 8, Fail: 0`. If a fragment diverges, `diff` shows it. (If `subst pattern parens` diverges due to a separate glob-pattern detail, investigate; the operand-parsing fix should make all 8 byte-identical.)

- [ ] **Step 4: Docs — `docs/bash-divergences.md`**

Add a new entry (a follow-on near the parameter-expansion entries, e.g. after M-15/M-16, or as a low/medium fixed entry):
```
- **M-15a: `${var:OP word}` operands parse as words** — `[fixed v84]` medium.
  The operand of a brace modifier (`:-`/`-`/`:=`/`=`/`:?`/`?`/`:+`/`+`, plus
  `${v/pat/repl}` patterns/replacements and `${v:off:len}` offsets) is now parsed
  as a word: `$…`/`` `…` ``/quotes expand/quote, but all other metacharacters
  (`(` `)` `|` `;` `&` `<` `>`) are LITERAL, matching bash. Previously the
  operand was run through the command tokenizer and any operator char errored
  with "invalid operator in parameter-expansion operand" — which broke common
  bash-isms like the stock Debian PS1 `${debian_chroot:+($debian_chroot)}`.
  Unquoted operands still field-split on whitespace; inner quotes suppress
  splitting. `parse_braced_operand` rewritten as a word char-walk (reusing
  `read_dollar_expansion`/`scan_backtick_substitution`); `LexError::InvalidBraceOperand`
  removed. Discovered loading a stock Debian `~/.bashrc`.
```
Add a `2026-06-04` change-log entry. Update the Summary "Last updated" stamp and Tier-2 Notes/count consistently.

- [ ] **Step 5: `README.md`** — add after the v83 row:
```
| v84       | `${var:+(…)}` operands parse as words (metachars literal)        |
```

- [ ] **Step 6: Final full suite + all 11 harnesses + clippy**

```bash
cargo test --quiet 2>&1 | grep -E "^test result" | awk '{p+=$4;f+=$6} END{print "PASS="p" FAIL="f}'
cargo clippy --all-targets 2>&1 | tail -2
cargo build --quiet
for h in arrays ifs test_combinators completion function_keyword arith_for loop_levels select script_mode pipefail param_operand; do
  echo -n "$h: "; tests/scripts/${h}_diff_check.sh 2>&1 | tail -1
done
```
Expected: FAIL=0; all 11 harnesses `Fail: 0`.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
v84 task 2: integration tests + bash-diff harness + docs

tests/param_operand_integration.rs (7 tests: alt/default operands with
parens+metachars, the Debian PS1 line, unquoted-splits vs quoted-one,
substitution pattern parens, parenthesized substring offset).
tests/scripts/param_operand_diff_check.sh (huck's 11th harness, 8 fragments
byte-identical to bash). docs: M-15a [fixed v84] + changelog + README row.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Final review checklist (before merge)

- [ ] All tests pass (`FAIL=0`); clippy clean.
- [ ] All 11 bash-diff harnesses `Fail: 0` (no regression in the prior 10).
- [ ] `${x:+($x)}`, `${y:-(a|b;c)}`, and the Debian PS1 `${debian_chroot:+($debian_chroot)}` all parse & match bash.
- [ ] Unquoted `${y:-a b c}` field-splits to 3 args; quoted stays 1.
- [ ] Inner quotes in operands honored (`'…'` / `"…"`); nested `${…}` works.
- [ ] `${v/(b)/X}` substitution + parenthesized substring offset work.
- [ ] No regression in existing parameter-expansion tests (`cargo test --bin huck` paramexp/expansion modules).

## Merge

`AskUserQuestion` before merging (per CLAUDE.md). Then `git merge --no-ff` into `main`, push, delete branch; update memory files.
