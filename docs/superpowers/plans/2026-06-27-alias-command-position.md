# Command-Position-Aware Alias Expander Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the alias expander expand an alias name only when the word is the first word of a simple command — with reserved-word recognition and `case`/`for`/`[[ ]]` context tracking — fixing the v231 regression where `case`-pattern words after `|` are wrongly alias-expanded.

**Architecture:** Replace the single threaded `next_eligible: bool` in `crates/huck-engine/src/alias_expand.rs` with an `Expander` struct that owns `eligible: bool` plus a `ctx: Vec<Ctx>` compound-command stack. Both public entry points (`expand_aliases_in_tokens`, `expand_aliases_in_tokens_mapped`) keep identical signatures, so the source path (`builtins.rs:6380`) and REPL path (`shell.rs:415`) are fixed by one change. The byte-offset/line remap contract is unchanged (alias-body tokens inherit the source index of the name token they replaced).

**Tech Stack:** Rust, huck workspace (`huck-engine`, `huck-syntax`). Tests via `cargo test --workspace`. Bash-compat via `tests/scripts/*_diff_check.sh`.

## Global Constraints

- **Commit trailer** (every commit, verbatim):
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- **No bash source vendoring**: diff harnesses run bash at runtime; never commit bash output.
- **Public API unchanged**: `expand_aliases_in_tokens(tokens, &aliases) -> Result<Vec<Token>, LexError>`, `expand_aliases_in_tokens_mapped(tokens, &aliases) -> Result<(Vec<Token>, Vec<usize>), LexError>`, and `pub(crate) fn simple_word_text(&Word) -> Option<String>` keep their exact signatures.
- **Map contract**: `map[i]` is the source-token index that output token `i` originated from; alias-body tokens inherit the name token's index; untouched tokens map to themselves.
- Reserved words and keywords are matched only against `simple_word_text` (unquoted plain literals).
- Lexer facts (already verified): `[[` and `]]` tokenize as `Token::Word` with literal text `"[["`/`"]]"`. `(( … ))` is a single `Token::ArithBlock` (no interior tokens to track). `;;`/`;&`/`;;&` are `Operator::DoubleSemi`/`SemiAmp`/`DoubleSemiAmp`. `Token` variants are `Word`, `Op`, `Newline`, `Heredoc`, `ArithBlock`, `RedirFd`.

---

### Task 1: Rewrite `alias_expand.rs` as a command-position-aware `Expander`

**Files:**
- Modify (full rewrite of the non-test code; keep + extend the `tests` module): `crates/huck-engine/src/alias_expand.rs`

**Interfaces:**
- Consumes: `crate::lexer::{LexError, Operator, Token, Word, WordPart, tokenize}`.
- Produces (unchanged public surface): `expand_aliases_in_tokens`, `expand_aliases_in_tokens_mapped`, `simple_word_text`.

This is one cohesive state machine; its unit tests are the bite-sized steps. Write each test, watch it fail (or pass, for the preserved ones), then move on.

- [ ] **Step 1: Replace the non-test code** in `crates/huck-engine/src/alias_expand.rs` (everything above `#[cfg(test)] mod tests`) with the following. Leave the existing `mod tests` in place for now; Step 3 appends to it.

```rust
//! Alias expansion. Runs after tokenize, before parse. Substitutes
//! aliases at command position with cycle protection and the bash
//! trailing-space rule. Command position is tracked with reserved-word
//! recognition and compound-command context (`case` / `for` / `[[ ]]`),
//! so words that are not the first word of a simple command (case
//! patterns, for-lists, `[[ ]]` interiors, reserved words themselves)
//! are never alias-expanded.

use std::collections::{HashMap, HashSet};

use crate::lexer::{LexError, Operator, Token, Word, WordPart};

/// Compound-command context. The stack handles nesting (e.g. a `case`
/// inside a `case` body).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Ctx {
    CaseSubject,   // after `case`, before `in`
    CasePattern,   // pattern list: after `in`, or after ;;/;&/;;&
    CaseBody,      // clause body — normal command position resumes
    ForName,       // after `for`/`select`, before `in`
    ForList,       // after for/select `in`, until separator or `do`
    DoubleBracket, // inside [[ ... ]]
}

/// Walks a token stream substituting aliases at command position.
pub fn expand_aliases_in_tokens(
    tokens: Vec<Token>,
    aliases: &HashMap<String, String>,
) -> Result<Vec<Token>, LexError> {
    expand_aliases_in_tokens_mapped(tokens, aliases).map(|(t, _)| t)
}

/// Like `expand_aliases_in_tokens` but also returns, per output token, the index
/// of the SOURCE token it originated from. Alias-body tokens inherit the index of
/// the alias-name token they replaced; untouched tokens map to themselves. Used by
/// the non-interactive source loop to remap byte-offsets/lines back to the raw
/// source after expansion rewrites the token stream.
pub fn expand_aliases_in_tokens_mapped(
    tokens: Vec<Token>,
    aliases: &HashMap<String, String>,
) -> Result<(Vec<Token>, Vec<usize>), LexError> {
    let mut ex = Expander::new(aliases);
    for (src_idx, token) in tokens.into_iter().enumerate() {
        ex.feed(token, src_idx)?;
    }
    Ok((ex.out, ex.map))
}

struct Expander<'a> {
    out: Vec<Token>,
    map: Vec<usize>,
    active: HashSet<String>,
    eligible: bool,
    ctx: Vec<Ctx>,
    aliases: &'a HashMap<String, String>,
}

impl<'a> Expander<'a> {
    fn new(aliases: &'a HashMap<String, String>) -> Self {
        Expander {
            out: Vec::new(),
            map: Vec::new(),
            active: HashSet::new(),
            eligible: true,
            ctx: Vec::new(),
            aliases,
        }
    }

    fn top(&self) -> Option<Ctx> {
        self.ctx.last().copied()
    }

    fn push(&mut self, token: Token, src_idx: usize) {
        self.out.push(token);
        self.map.push(src_idx);
    }

    /// Feed one source token, updating output, map, eligibility, and context.
    fn feed(&mut self, token: Token, src_idx: usize) -> Result<(), LexError> {
        match token {
            Token::Word(w) => self.feed_word(w, src_idx),
            Token::Op(op) => {
                self.feed_op(op, src_idx);
                Ok(())
            }
            Token::Newline => {
                self.feed_newline(src_idx);
                Ok(())
            }
            // Heredoc / ArithBlock / RedirFd: not command-position changing.
            other => {
                self.push(other, src_idx);
                Ok(())
            }
        }
    }

    fn feed_word(&mut self, w: Word, src_idx: usize) -> Result<(), LexError> {
        let text = simple_word_text(&w);

        // Context-driven handling first: words inside subject/pattern/for/[[
        // positions are never alias-expanded; some drive transitions.
        match self.top() {
            Some(Ctx::CaseSubject) => {
                if text.as_deref() == Some("in") {
                    *self.ctx.last_mut().unwrap() = Ctx::CasePattern;
                }
                self.push(Token::Word(w), src_idx);
                self.eligible = false;
                return Ok(());
            }
            Some(Ctx::CasePattern) => {
                if text.as_deref() == Some("esac") {
                    self.ctx.pop();
                }
                self.push(Token::Word(w), src_idx);
                self.eligible = false;
                return Ok(());
            }
            Some(Ctx::ForName) => {
                if text.as_deref() == Some("in") {
                    *self.ctx.last_mut().unwrap() = Ctx::ForList;
                }
                self.push(Token::Word(w), src_idx);
                self.eligible = false;
                return Ok(());
            }
            Some(Ctx::ForList) => {
                self.push(Token::Word(w), src_idx);
                self.eligible = false;
                return Ok(());
            }
            Some(Ctx::DoubleBracket) => {
                if text.as_deref() == Some("]]") {
                    self.ctx.pop();
                }
                self.push(Token::Word(w), src_idx);
                self.eligible = false;
                return Ok(());
            }
            Some(Ctx::CaseBody) | None => {}
        }

        // Normal command-position handling (CaseBody or empty stack).
        if self.eligible {
            if let Some(t) = text.as_deref() {
                if let Some(next_elig) = self.handle_reserved(t) {
                    self.push(Token::Word(w), src_idx);
                    self.eligible = next_elig;
                    return Ok(());
                }
                if !self.active.contains(t)
                    && let Some(body) = self.aliases.get(t).cloned()
                {
                    return self.expand_alias(t.to_string(), body, src_idx);
                }
            }
            // Ordinary command word (no alias): consumes command position.
            self.push(Token::Word(w), src_idx);
            self.eligible = false;
            return Ok(());
        }

        // Argument word.
        self.push(Token::Word(w), src_idx);
        self.eligible = false;
        Ok(())
    }

    /// If `t` is a reserved word recognized at command position, update
    /// context and return `Some(next_eligible)`. Returns `None` if `t` is
    /// not a reserved word (caller then tries alias expansion).
    fn handle_reserved(&mut self, t: &str) -> Option<bool> {
        match t {
            "case" => {
                self.ctx.push(Ctx::CaseSubject);
                Some(false)
            }
            "for" | "select" => {
                self.ctx.push(Ctx::ForName);
                Some(false)
            }
            "[[" => {
                self.ctx.push(Ctx::DoubleBracket);
                Some(false)
            }
            "function" => Some(false),
            "if" | "then" | "elif" | "else" | "do" | "while" | "until" | "{" | "!" | "time" => {
                Some(true)
            }
            "fi" | "done" | "}" => Some(false),
            "esac" => {
                if self.top() == Some(Ctx::CaseBody) {
                    self.ctx.pop();
                }
                Some(false)
            }
            _ => None,
        }
    }

    fn expand_alias(
        &mut self,
        name: String,
        body: String,
        src_idx: usize,
    ) -> Result<(), LexError> {
        self.active.insert(name.clone());
        let inner_tokens = crate::lexer::tokenize(&body)?;
        // The alias body begins at command position.
        self.eligible = true;
        for inner in inner_tokens {
            // Body tokens inherit the alias-name token's source index.
            self.feed(inner, src_idx)?;
        }
        self.active.remove(&name);
        // Trailing-blank rule: a body ending in whitespace makes the next
        // source token alias-eligible.
        self.eligible = body.chars().last().is_some_and(|c| c.is_whitespace());
        Ok(())
    }

    fn feed_op(&mut self, op: Operator, src_idx: usize) {
        match self.top() {
            Some(Ctx::CasePattern) => {
                self.push(Token::Op(op), src_idx);
                if matches!(op, Operator::RParen) {
                    *self.ctx.last_mut().unwrap() = Ctx::CaseBody;
                    self.eligible = true;
                } else {
                    // `|` (pattern alternative), leading `(`, or any other op:
                    // stay in pattern position.
                    self.eligible = false;
                }
            }
            Some(Ctx::CaseBody)
                if matches!(
                    op,
                    Operator::DoubleSemi | Operator::SemiAmp | Operator::DoubleSemiAmp
                ) =>
            {
                self.push(Token::Op(op), src_idx);
                *self.ctx.last_mut().unwrap() = Ctx::CasePattern;
                self.eligible = false;
            }
            Some(Ctx::ForName) | Some(Ctx::ForList) if matches!(op, Operator::Semi) => {
                self.push(Token::Op(op), src_idx);
                self.ctx.pop();
                self.eligible = true;
            }
            Some(Ctx::CaseSubject)
            | Some(Ctx::DoubleBracket)
            | Some(Ctx::ForName)
            | Some(Ctx::ForList) => {
                self.push(Token::Op(op), src_idx);
                self.eligible = false;
            }
            _ => {
                // CaseBody (non-;;) or empty stack: normal separator logic.
                self.push(Token::Op(op), src_idx);
                self.eligible = matches!(
                    op,
                    Operator::Pipe
                        | Operator::And
                        | Operator::Or
                        | Operator::Semi
                        | Operator::Background
                        | Operator::LParen
                );
            }
        }
    }

    fn feed_newline(&mut self, src_idx: usize) {
        match self.top() {
            Some(Ctx::ForName) | Some(Ctx::ForList) => {
                self.ctx.pop();
                self.push(Token::Newline, src_idx);
                self.eligible = true;
            }
            Some(Ctx::CaseSubject) | Some(Ctx::CasePattern) | Some(Ctx::DoubleBracket) => {
                self.push(Token::Newline, src_idx);
                self.eligible = false;
            }
            _ => {
                // CaseBody or empty stack: newline is a command separator.
                self.push(Token::Newline, src_idx);
                self.eligible = true;
            }
        }
    }
}

/// Returns the concatenated literal text of a Word iff every part is
/// an unquoted Literal. Returns None for any quoted, Var, Arith,
/// CommandSub, or Tilde part — aliases only expand from plain
/// unquoted identifiers.
pub(crate) fn simple_word_text(w: &Word) -> Option<String> {
    let mut text = String::new();
    for part in &w.0 {
        match part {
            WordPart::Literal { text: t, quoted: false } => text.push_str(t),
            _ => return None,
        }
    }
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}
```

- [ ] **Step 2: Build to confirm the rewrite compiles and existing unit tests still pass**

Run: `cargo test -p huck-engine --lib alias_expand`
Expected: PASS — all pre-existing `alias_expand::tests` (`simple_expansion`, `no_expansion_outside_command_position`, `recursive_expansion`, `cycle_protection`, `expansion_after_pipe`, `expansion_after_semi`, `trailing_space_chains_expansion`, `quoted_word_not_expanded`, `mapped_expansion_tracks_source_indices`, `mapped_noop_is_identity`) green.

If the `if let … && let …` chained-let syntax does not compile on the toolchain, rewrite the two chained `if let` sites in `feed_word` as nested `if let` blocks (same logic) — check `rustc --version` and the existing use of `&& let` already in `alias_expand.rs` Step 1 (the original file already used `&& let`, so the toolchain supports it).

- [ ] **Step 3: Append the new unit tests** to the `mod tests` block in `crates/huck-engine/src/alias_expand.rs` (after the existing `mapped_noop_is_identity` test, before the closing `}` of `mod tests`):

```rust
    #[test]
    fn case_pattern_word_not_expanded() {
        // The v231 regression: `ls` after `|` is a case pattern, not a command.
        let aliases = make_aliases(&[("ls", "ls --color")]);
        let toks = tokenize("case $x in use | ls | list) echo hi ;; *) echo no ;; esac").unwrap();
        let out = expand_aliases_in_tokens(toks, &aliases).unwrap();
        assert_tokens_eq(&out, "case $x in use | ls | list) echo hi ;; *) echo no ;; esac");
    }

    #[test]
    fn case_subject_word_not_expanded() {
        let aliases = make_aliases(&[("ll", "ls -l")]);
        let toks = tokenize("case ll in a) echo x ;; esac").unwrap();
        let out = expand_aliases_in_tokens(toks, &aliases).unwrap();
        assert_tokens_eq(&out, "case ll in a) echo x ;; esac");
    }

    #[test]
    fn case_body_command_is_expanded() {
        // Inside a clause body we ARE at command position.
        let aliases = make_aliases(&[("ll", "ls -l")]);
        let toks = tokenize("case $x in a) ll ;; esac").unwrap();
        let out = expand_aliases_in_tokens(toks, &aliases).unwrap();
        assert_tokens_eq(&out, "case $x in a) ls -l ;; esac");
    }

    #[test]
    fn nested_case_patterns_not_expanded() {
        let aliases = make_aliases(&[("ls", "ls --color")]);
        let toks =
            tokenize("case $x in a) case $y in ls) echo z ;; esac ;; esac").unwrap();
        let out = expand_aliases_in_tokens(toks, &aliases).unwrap();
        assert_tokens_eq(&out, "case $x in a) case $y in ls) echo z ;; esac ;; esac");
    }

    #[test]
    fn expand_after_then_and_do() {
        // The opposite latent bug: reserved words introduce command position.
        let aliases = make_aliases(&[("ll", "ls -l")]);
        let toks = tokenize("if true; then ll; fi").unwrap();
        let out = expand_aliases_in_tokens(toks, &aliases).unwrap();
        assert_tokens_eq(&out, "if true; then ls -l; fi");

        let toks = tokenize("while true; do ll; done").unwrap();
        let out = expand_aliases_in_tokens(toks, &aliases).unwrap();
        assert_tokens_eq(&out, "while true; do ls -l; done");
    }

    #[test]
    fn reserved_word_not_expanded() {
        // An alias whose name is a reserved word is not expanded in that slot.
        let aliases = make_aliases(&[("then", "echo BAD")]);
        let toks = tokenize("if true; then echo ok; fi").unwrap();
        let out = expand_aliases_in_tokens(toks, &aliases).unwrap();
        assert_tokens_eq(&out, "if true; then echo ok; fi");
    }

    #[test]
    fn for_list_words_not_expanded_body_is() {
        let aliases = make_aliases(&[("ls", "ls --color"), ("ll", "ls -l")]);
        let toks = tokenize("for x in ls ll; do ll; done").unwrap();
        let out = expand_aliases_in_tokens(toks, &aliases).unwrap();
        // `ls` and `ll` in the list stay literal; `ll` in the body expands.
        assert_tokens_eq(&out, "for x in ls ll; do ls -l; done");
    }

    #[test]
    fn double_bracket_interior_not_expanded() {
        let aliases = make_aliases(&[("ll", "ls -l")]);
        let toks = tokenize("[[ ll == x ]]").unwrap();
        let out = expand_aliases_in_tokens(toks, &aliases).unwrap();
        assert_tokens_eq(&out, "[[ ll == x ]]");
    }

    #[test]
    fn mapped_indices_preserved_through_case() {
        // Offsets must still anchor to raw source token indices.
        let aliases = make_aliases(&[("ls", "ls --color")]);
        let toks = tokenize("case $x in ls) echo z ;; esac").unwrap();
        let n = toks.len();
        let (out, map) = expand_aliases_in_tokens_mapped(toks, &aliases).unwrap();
        // No expansion happened, so output is identity-mapped.
        assert_eq!(out.len(), n);
        assert_eq!(map, (0..n).collect::<Vec<_>>());
    }
```

- [ ] **Step 4: Run the full alias_expand unit suite**

Run: `cargo test -p huck-engine --lib alias_expand`
Expected: PASS — all old and new tests green.

- [ ] **Step 5: Run the broader engine + workspace suites to catch behavior shifts**

Run: `cargo test --workspace`
Expected: PASS. If any pre-existing test asserted the *old* (buggy) non-expansion after a reserved word, or wrongly relied on a case pattern being expanded, update it to the bash-correct expectation and note it in the task report. Do not weaken a test to hide a real regression — confirm against bash first.

- [ ] **Step 6: Commit**

```bash
git add crates/huck-engine/src/alias_expand.rs
git commit -m "v232: command-position-aware alias expander

Replace the next_eligible bool with an Expander struct tracking command
position via reserved-word recognition and a case/for/[[ ]] context
stack. Case-pattern words after | are no longer alias-expanded (fixes
the v231 .bashrc regression); expansions after then/do/{ now fire.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Bash-compat diff harness for case / reserved-word alias fragments

**Files:**
- Create: `tests/scripts/alias_case_diff_check.sh`

**Interfaces:**
- Consumes: the release/debug `huck` binary at `$HUCK_BIN` (default `target/debug/huck`), same convention as `tests/scripts/alias_expand_diff_check.sh`.

- [ ] **Step 1: Write the harness** at `tests/scripts/alias_case_diff_check.sh`:

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v232: command-position-aware
# alias expansion (case patterns, reserved words, for-lists, [[ ]]).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
checkf() {
    local label="$1" body="$2" tmp b h
    tmp=$(mktemp "${TMPDIR:-/tmp}/huck-aliascase.XXXXXX")
    printf '%s\n' "$body" > "$tmp"
    b=$(bash "$tmp" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tmp" 2>&1; echo "EXIT:$?")
    rm -f "$tmp"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# The regression: an aliased name used as a case pattern must not break parsing.
checkf "case pattern after pipe" \
  'shopt -s expand_aliases; alias ls="ls --color"; x=ls; case "$x" in use | ls | list) echo HIT ;; *) echo MISS ;; esac'
checkf "case subject not expanded" \
  'shopt -s expand_aliases; alias ll="echo BAD"; case ll in ll) echo OK ;; *) echo NO ;; esac'
checkf "case body command expands" \
  'shopt -s expand_aliases; alias ll="echo LL"; case x in x) ll ;; esac'
checkf "nested case patterns" \
  'shopt -s expand_aliases; alias ls="echo BAD"; case a in a) case b in ls) echo IN ;; *) echo X ;; esac ;; esac'
checkf "expand after then" \
  'shopt -s expand_aliases; alias g="echo G"; if true; then g; fi'
checkf "expand after do" \
  'shopt -s expand_aliases; alias g="echo G"; for i in 1 2; do g; done'
checkf "for-list words not expanded" \
  'shopt -s expand_aliases; alias one="echo BAD"; for w in one two; do echo "$w"; done'
checkf "double bracket interior" \
  'shopt -s expand_aliases; alias ll="echo BAD"; if [[ ll == ll ]]; then echo OK; fi'
checkf "reserved word slot" \
  'shopt -s expand_aliases; alias then="echo BAD"; if true; then echo OK; fi'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 2: Make it executable**

Run: `chmod +x tests/scripts/alias_case_diff_check.sh`

- [ ] **Step 3: Build huck (debug) and run the harness**

Run: `cargo build -p huck && ./tests/scripts/alias_case_diff_check.sh`
Expected: `Total: 9, Pass: 9, Fail: 0` (exit 0). If any case FAILs, the diff is printed — fix the expander (Task 1) cause, not the harness expectation, unless the divergence is a genuine intentional one (then document it).

- [ ] **Step 4: Commit**

```bash
git add tests/scripts/alias_case_diff_check.sh
git commit -m "v232: alias_case_diff_check harness (bash<->huck)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Regression integration test (`.bashrc`-style case fragment)

**Files:**
- Create: `tests/alias_command_position_integration.rs`

**Interfaces:**
- Consumes: `env!("CARGO_BIN_EXE_huck")` and the `run_file` pattern from `tests/alias_expand_integration.rs` (file-mode, stdin null, returns `(stdout, stderr, code)`).

- [ ] **Step 1: Write the integration test** at `tests/alias_command_position_integration.rs`:

```rust
//! v232: command-position-aware alias expansion — regression coverage for
//! the v231 bug where a case-pattern word matching an alias broke parsing
//! when sourcing files like ~/.bashrc / nvm bash_completion.
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);
fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

/// Run `script` as a file arg (non-interactive). Returns (stdout, stderr, code).
fn run_file(script: &str) -> (String, String, i32) {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("huck_v232cp_{}_{}_.sh", std::process::id(), n));
    { let mut f = std::fs::File::create(&path).unwrap(); f.write_all(script.as_bytes()).unwrap(); }
    let out = Command::new(huck_bin()).arg(&path).stdin(Stdio::null()).output().unwrap();
    let _ = std::fs::remove_file(&path);
    (String::from_utf8_lossy(&out.stdout).into_owned(),
     String::from_utf8_lossy(&out.stderr).into_owned(),
     out.status.code().unwrap_or(-1))
}

#[test]
fn case_pattern_alias_does_not_break_parsing() {
    // The exact shape from nvm bash_completion: an aliased name (`ls`) used
    // as a case pattern after `|`. Must parse and run cleanly.
    let script = "shopt -s expand_aliases\n\
                  alias ls='ls --color'\n\
                  f() { case \"$1\" in use | ls | list) echo HIT ;; *) echo MISS ;; esac; }\n\
                  f ls\n\
                  f other\n";
    let (o, e, c) = run_file(script);
    assert_eq!(c, 0, "stderr: {e}");
    assert!(!e.contains("syntax error"), "unexpected syntax error: {e}");
    assert_eq!(o, "HIT\nMISS\n");
}

#[test]
fn alias_expands_in_case_body() {
    let script = "shopt -s expand_aliases\n\
                  alias greet='echo HELLO'\n\
                  case x in x) greet ;; esac\n";
    let (o, _, c) = run_file(script);
    assert_eq!(c, 0);
    assert_eq!(o, "HELLO\n");
}

#[test]
fn alias_expands_after_reserved_word() {
    let script = "shopt -s expand_aliases\n\
                  alias greet='echo HELLO'\n\
                  if true; then greet; fi\n";
    let (o, _, c) = run_file(script);
    assert_eq!(c, 0);
    assert_eq!(o, "HELLO\n");
}

#[test]
fn for_list_words_not_alias_expanded() {
    let script = "shopt -s expand_aliases\n\
                  alias one='echo BAD'\n\
                  for w in one two; do echo \"$w\"; done\n";
    let (o, _, c) = run_file(script);
    assert_eq!(c, 0);
    assert_eq!(o, "one\ntwo\n");
}
```

- [ ] **Step 2: Run the integration test**

Run: `cargo test --test alias_command_position_integration`
Expected: PASS — all four tests green.

- [ ] **Step 3: Commit**

```bash
git add tests/alias_command_position_integration.rs
git commit -m "v232: regression integration tests for case-pattern aliases

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Post-implementation (controller, after final review)

- Run `cargo test --workspace` and the bash-test sweep (`HUCK_BASH_TEST_CATEGORY=alias`) to measure whether `alias` shifts; record the measured residual.
- Update `docs/bash-divergences.md`: refine/replace the L-69 entry (the case-pattern regression is resolved; keep the genuinely-deferred residuals — redirect-prefix command position, `-c` mode unwired, alias2/4.sub infra).
- Record v232 in `MEMORY.md` + `project_huck_iterations.md`.

## Self-Review

- **Spec coverage:** Reserved-word table → `handle_reserved` (Task 1). Case context → `CaseSubject`/`CasePattern`/`CaseBody` arms. for/select → `ForName`/`ForList`. `[[ ]]` → `DoubleBracket`. Both call sites → unchanged public fns. Tests → unit (Task 1) + diff harness (Task 2) + integration (Task 3). All spec sections covered.
- **Placeholder scan:** none — all steps contain concrete code/commands.
- **Type consistency:** `Ctx` enum, `Expander` fields, and the three public fn signatures are consistent across tasks; `simple_word_text` signature unchanged; test helpers (`make_aliases`, `assert_tokens_eq`, `run_file`) match the existing files they extend.
