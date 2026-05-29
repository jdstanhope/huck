# huck v48 — Aliases Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add bash-style aliases to huck: `alias name=value`,
`unalias`, recursive expansion with cycle protection, trailing-space
rule, interactive-only expansion.

**Architecture:** `Shell.aliases: HashMap<String, String>` storage.
New `src/alias_expand.rs` module performs post-tokenize substitution
at command positions, with recursive cycle-protected expansion. New
`alias`/`unalias` builtins. `process_line` gains an `expand_aliases:
bool` parameter; the REPL passes `shell.is_interactive`, trap firings
pass `false`.

**Tech Stack:** Rust. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-05-29-huck-aliases-design.md`

**Branch:** `v48-aliases` (created in preamble step P.1).

**Commit trailer convention**:

```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Preamble: Create the working branch

- [ ] **Step P.1: Create branch from main and check it out**

```bash
git checkout main
git pull --ff-only
git checkout -b v48-aliases
```

Expected: `Switched to a new branch 'v48-aliases'`.

The spec + this plan are committed as the first commit on this branch
(handled by the controller before Task 1 begins).

---

## Task 1: Foundation + builtins + unit tests

**Files:**
- Modify: `src/shell_state.rs` — add `pub aliases: HashMap` field.
- Create: `src/alias_expand.rs` — new module with
  `expand_aliases_in_tokens` + `simple_word_text` + 8 unit tests.
- Modify: `src/main.rs` — add `mod alias_expand;`.
- Modify: `src/builtins.rs` — add `alias` and `unalias` builtins +
  dispatch + BUILTIN_NAMES update + 8 unit tests.
- Modify: `src/shell.rs::process_line` — add `expand_aliases: bool`
  parameter; update REPL caller (line ~71).
- Modify: `src/traps.rs` — update 3 trap-firing callers (lines 70,
  82, 116) to pass `false`.

### Step 1.1: Add `aliases` field to `Shell` in `src/shell_state.rs`

In `src/shell_state.rs`, find the `pub struct Shell { ... }` block
(starts at line 19). Inside the struct, alongside the other public
fields (vars, functions, jobs, etc.), add:

```rust
    /// User-defined aliases. `name` → expansion text. Populated by
    /// the `alias` builtin; consumed by `expand_aliases_in_tokens`
    /// during interactive REPL input.
    pub aliases: std::collections::HashMap<String, String>,
```

A natural position: after `pub functions: HashMap<...>` (it's a
similar "user-defined name table"). Add it there.

Then find `impl Shell { pub fn new() -> Self { ... } }`. Inside
`new`, the struct literal at the end initializes every field. Add:

```rust
            aliases: std::collections::HashMap::new(),
```

immediately after the `functions` initialization.

- [ ] **Step 1.1: Add aliases field**

### Step 1.2: Build to confirm `src/shell_state.rs` compiles

Run: `cargo build`
Expected: clean.

- [ ] **Step 1.2: Build clean**

### Step 1.3: Create `src/alias_expand.rs` module

Create `src/alias_expand.rs` with this content:

```rust
//! Alias expansion. Runs after tokenize, before parse. Substitutes
//! aliases at command position with cycle protection and the bash
//! trailing-space rule.

use std::collections::{HashMap, HashSet};

use crate::lexer::{LexError, Operator, Token, Word, WordPart};

/// Walks `tokens`, substituting alias definitions at command
/// position. Recursive substitution is cycle-protected via a
/// per-input `active` set. The trailing-space rule applies: if an
/// alias body ends with whitespace, the token immediately following
/// the expansion is itself alias-eligible.
pub fn expand_aliases_in_tokens(
    tokens: Vec<Token>,
    aliases: &HashMap<String, String>,
) -> Result<Vec<Token>, LexError> {
    let mut out: Vec<Token> = Vec::new();
    let mut next_eligible = true;
    let mut active: HashSet<String> = HashSet::new();
    for token in tokens {
        next_eligible = process_token(token, &mut out, next_eligible, aliases, &mut active)?;
    }
    Ok(out)
}

fn process_token(
    token: Token,
    out: &mut Vec<Token>,
    eligible: bool,
    aliases: &HashMap<String, String>,
    active: &mut HashSet<String>,
) -> Result<bool, LexError> {
    match &token {
        Token::Word(w) => {
            if eligible {
                if let Some(name) = simple_word_text(w) {
                    if !active.contains(&name) {
                        if let Some(body) = aliases.get(&name).cloned() {
                            active.insert(name.clone());
                            let inner_tokens = crate::lexer::tokenize(&body)?;
                            let mut inner_eligible = true;
                            for inner in inner_tokens {
                                inner_eligible = process_token(
                                    inner,
                                    out,
                                    inner_eligible,
                                    aliases,
                                    active,
                                )?;
                            }
                            active.remove(&name);
                            let trailing = body
                                .chars()
                                .last()
                                .is_some_and(|c| c.is_whitespace());
                            return Ok(trailing);
                        }
                    }
                }
            }
            out.push(token);
            Ok(false)
        }
        Token::Op(op) => {
            let separator = matches!(
                op,
                Operator::Pipe
                    | Operator::And
                    | Operator::Or
                    | Operator::Semi
                    | Operator::Background
                    | Operator::LParen
            );
            out.push(token);
            Ok(separator)
        }
        Token::Newline => {
            out.push(token);
            Ok(true)
        }
        _ => {
            out.push(token);
            Ok(eligible)
        }
    }
}

/// Returns the concatenated literal text of a Word iff every part is
/// an unquoted Literal. Returns None for any quoted, Var, Arith,
/// CommandSub, or Tilde part — aliases only expand from plain
/// unquoted identifiers.
fn simple_word_text(w: &Word) -> Option<String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::tokenize;

    fn make_aliases(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    /// Compare two token streams by re-tokenizing the expected source
    /// (avoids hand-constructing complex Token::Word values).
    fn assert_tokens_eq(actual: &[Token], expected_source: &str) {
        let expected = tokenize(expected_source).expect("expected source must tokenize");
        assert_eq!(actual, &expected[..], "actual:\n  {:?}\nexpected:\n  {:?}", actual, expected);
    }

    #[test]
    fn simple_expansion() {
        let aliases = make_aliases(&[("ll", "ls -l")]);
        let toks = tokenize("ll").unwrap();
        let out = expand_aliases_in_tokens(toks, &aliases).unwrap();
        assert_tokens_eq(&out, "ls -l");
    }

    #[test]
    fn no_expansion_outside_command_position() {
        let aliases = make_aliases(&[("ll", "ls -l")]);
        let toks = tokenize("echo ll").unwrap();
        let out = expand_aliases_in_tokens(toks, &aliases).unwrap();
        assert_tokens_eq(&out, "echo ll");
    }

    #[test]
    fn recursive_expansion() {
        let aliases = make_aliases(&[("ls", "ls --color"), ("ll", "ls -l")]);
        let toks = tokenize("ll").unwrap();
        let out = expand_aliases_in_tokens(toks, &aliases).unwrap();
        assert_tokens_eq(&out, "ls --color -l");
    }

    #[test]
    fn cycle_protection() {
        let aliases = make_aliases(&[("ls", "ls --color")]);
        let toks = tokenize("ls").unwrap();
        let out = expand_aliases_in_tokens(toks, &aliases).unwrap();
        // Only one substitution — the inner `ls` is in `active` and
        // does not re-expand.
        assert_tokens_eq(&out, "ls --color");
    }

    #[test]
    fn expansion_after_pipe() {
        let aliases = make_aliases(&[("ll", "ls -l")]);
        let toks = tokenize("cat | ll").unwrap();
        let out = expand_aliases_in_tokens(toks, &aliases).unwrap();
        assert_tokens_eq(&out, "cat | ls -l");
    }

    #[test]
    fn expansion_after_semi() {
        let aliases = make_aliases(&[("ll", "ls -l")]);
        let toks = tokenize("true; ll").unwrap();
        let out = expand_aliases_in_tokens(toks, &aliases).unwrap();
        assert_tokens_eq(&out, "true; ls -l");
    }

    #[test]
    fn trailing_space_chains_expansion() {
        // Note the trailing space in the `sudo` body.
        let aliases = make_aliases(&[("sudo", "sudo "), ("ll", "ls -l")]);
        let toks = tokenize("sudo ll").unwrap();
        let out = expand_aliases_in_tokens(toks, &aliases).unwrap();
        assert_tokens_eq(&out, "sudo ls -l");
    }

    #[test]
    fn quoted_word_not_expanded() {
        let aliases = make_aliases(&[("ll", "ls -l")]);
        let toks = tokenize("'ll'").unwrap();
        let out = expand_aliases_in_tokens(toks, &aliases).unwrap();
        // `'ll'` is a quoted Literal — `simple_word_text` returns None
        // because `quoted: true`. So no expansion fires.
        assert_eq!(out, tokenize("'ll'").unwrap());
    }
}
```

- [ ] **Step 1.3: Create the module file**

### Step 1.4: Wire `mod alias_expand;` into `src/main.rs`

In `src/main.rs`, add `mod alias_expand;` between `mod arith;` and
`mod brace_expand;` (alphabetical order):

```rust
mod arith;
mod alias_expand;
mod brace_expand;
mod builtins;
// ... rest unchanged
```

Actually wait — alphabetically `alias` comes before `arith`. Use:

```rust
mod alias_expand;
mod arith;
mod brace_expand;
mod builtins;
// ... rest unchanged
```

- [ ] **Step 1.4: Add `mod alias_expand;`**

### Step 1.5: Build + run alias_expand tests

Run: `cargo build`
Expected: clean.

Run: `cargo test --bin huck alias_expand:: -- --nocapture`
Expected: 8 tests pass.

If any test fails: most likely culprits are the `simple_word_text`
return value (the `quoted_word_not_expanded` test depends on the
lexer's single-quote arm producing `Literal{quoted: true}`), or
the operator separator list missing a case. Iterate until 8/8.

- [ ] **Step 1.5: 8 alias_expand tests pass**

### Step 1.6: Add `alias` and `unalias` to BUILTIN_NAMES

In `src/builtins.rs:18-22`, replace:

```rust
pub const BUILTIN_NAMES: &[&str] = &[
    "cd", "exit", "pwd", "echo", "export", "unset", "jobs",
    "wait", "fg", "bg", "kill", "disown", "history", "test", "[",
    "break", "continue", "return", "trap",
];
```

With:

```rust
pub const BUILTIN_NAMES: &[&str] = &[
    "cd", "exit", "pwd", "echo", "export", "unset", "jobs",
    "wait", "fg", "bg", "kill", "disown", "history", "test", "[",
    "break", "continue", "return", "trap", "alias", "unalias",
];
```

- [ ] **Step 1.6: Update BUILTIN_NAMES**

### Step 1.7: Add `alias`/`unalias` dispatch arms in `run_builtin`

In `src/builtins.rs::run_builtin` (around line 46-61), find the
`match name { ... }` block. Add two new arms before `"break"`:

```rust
        "alias" => builtin_alias(args, out, shell),
        "unalias" => builtin_unalias(args, shell),
```

Position them somewhere natural — e.g., after `"trap" =>
builtin_trap(args, out, shell),`.

- [ ] **Step 1.7: Add dispatch arms**

### Step 1.8: Add `is_valid_alias_name`, `escape_alias_value`, `builtin_alias`, `builtin_unalias`

In `src/builtins.rs`, find a natural insertion point — adjacent to
the other builtins. Insert these helpers and builtins (after
`builtin_trap`, for example, which is around line 765+):

```rust
fn is_valid_alias_name(s: &str) -> bool {
    !s.is_empty()
        && !s.contains('=')
        && s.chars().all(|c| !c.is_whitespace() && !"|&;<>()$`\\\"'*?[]#~{}".contains(c))
}

fn escape_alias_value(v: &str) -> String {
    // Bash format: alias name='value' with single quotes inside
    // the value rewritten as '\''.
    v.replace('\'', r#"'\''"#)
}

fn builtin_alias(args: &[String], out: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    if args.is_empty() {
        let mut names: Vec<&String> = shell.aliases.keys().collect();
        names.sort();
        for name in names {
            let value = &shell.aliases[name];
            let _ = writeln!(out, "alias {}='{}'", name, escape_alias_value(value));
        }
        return ExecOutcome::Continue(0);
    }
    let mut any_err = false;
    for arg in args {
        if let Some(eq) = arg.find('=') {
            let name = &arg[..eq];
            let value = &arg[eq + 1..];
            if !is_valid_alias_name(name) {
                eprintln!("huck: alias: `{name}': invalid alias name");
                any_err = true;
                continue;
            }
            shell.aliases.insert(name.to_string(), value.to_string());
        } else {
            match shell.aliases.get(arg) {
                Some(v) => {
                    let _ = writeln!(out, "alias {}='{}'", arg, escape_alias_value(v));
                }
                None => {
                    eprintln!("huck: alias: {arg}: not found");
                    any_err = true;
                }
            }
        }
    }
    ExecOutcome::Continue(if any_err { 1 } else { 0 })
}

fn builtin_unalias(args: &[String], shell: &mut Shell) -> ExecOutcome {
    if args.is_empty() {
        eprintln!("huck: unalias: usage: unalias [-a] name [name ...]");
        return ExecOutcome::Continue(2);
    }
    if args[0] == "-a" {
        shell.aliases.clear();
        return ExecOutcome::Continue(0);
    }
    let mut any_err = false;
    for name in args {
        if shell.aliases.remove(name).is_none() {
            eprintln!("huck: unalias: {name}: not found");
            any_err = true;
        }
    }
    ExecOutcome::Continue(if any_err { 1 } else { 0 })
}
```

- [ ] **Step 1.8: Add builtins + helpers**

### Step 1.9: Build

Run: `cargo build`
Expected: clean.

- [ ] **Step 1.9: Build clean**

### Step 1.10: Add 8 unit tests for the builtins

In `src/builtins.rs`, find a place to add a new test module. The
file has several existing `#[cfg(test)] mod` blocks (tests at line
1404, fg_bg_tests at 2004, kill_tests at 2304, etc.). Add a new
mod at the end of the file (after the last `mod` block):

```rust
#[cfg(test)]
mod alias_tests {
    use super::*;
    use crate::shell_state::Shell;

    #[test]
    fn alias_no_args_lists_empty() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("alias", &[], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert!(buf.is_empty(), "expected empty output, got {:?}", String::from_utf8_lossy(&buf));
    }

    #[test]
    fn alias_no_args_lists_sorted() {
        let mut shell = Shell::new();
        shell.aliases.insert("ll".to_string(), "ls -l".to_string());
        shell.aliases.insert("la".to_string(), "ls -A".to_string());
        shell.aliases.insert("l".to_string(), "ls".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("alias", &[], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(
            lines,
            vec![
                "alias l='ls'",
                "alias la='ls -A'",
                "alias ll='ls -l'",
            ]
        );
    }

    #[test]
    fn alias_defines_simple() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "alias",
            &["ll=ls -l".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.aliases.get("ll").map(|s| s.as_str()), Some("ls -l"));
    }

    #[test]
    fn alias_lookup_existing_prints() {
        let mut shell = Shell::new();
        shell.aliases.insert("ll".to_string(), "ls -l".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("alias", &["ll".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        assert_eq!(out, "alias ll='ls -l'\n");
    }

    #[test]
    fn alias_lookup_missing_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("alias", &["xyz".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn unalias_removes_existing() {
        let mut shell = Shell::new();
        shell.aliases.insert("ll".to_string(), "ls -l".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("unalias", &["ll".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert!(!shell.aliases.contains_key("ll"));
    }

    #[test]
    fn unalias_missing_errors_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("unalias", &["xyz".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn unalias_dash_a_clears_all() {
        let mut shell = Shell::new();
        shell.aliases.insert("ll".to_string(), "ls -l".to_string());
        shell.aliases.insert("la".to_string(), "ls -A".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("unalias", &["-a".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert!(shell.aliases.is_empty());
    }

    #[test]
    fn unalias_no_args_returns_usage_status_2() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("unalias", &[], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }
}
```

Wait — that's 9 tests, not 8. Drop the `unalias_no_args_returns_usage_status_2`
test if you want exactly 8, OR keep it (it's a useful coverage of
the args-required path). Recommend keeping it; the 8/9 count
discrepancy is minor.

- [ ] **Step 1.10: Add the test module**

### Step 1.11: Change `process_line` signature in `src/shell.rs`

Find `pub fn process_line(line: &str, shell: &mut Shell) -> ExecOutcome` at `src/shell.rs:238`. Change the signature to:

```rust
pub fn process_line(line: &str, shell: &mut Shell, expand_aliases: bool) -> ExecOutcome {
```

Inside the function body, between the existing `tokenize` block and
the `parse` block, insert the alias-expansion stage:

Current (around lines 239-247):

```rust
pub fn process_line(line: &str, shell: &mut Shell) -> ExecOutcome {
    let tokens = match lexer::tokenize(line) {
        Ok(tokens) => tokens,
        Err(e) => {
            eprintln!("huck: syntax error{}", lex_error_message(e));
            return ExecOutcome::Continue(2);
        }
    };

    match command::parse(tokens) {
```

New:

```rust
pub fn process_line(line: &str, shell: &mut Shell, expand_aliases: bool) -> ExecOutcome {
    let tokens = match lexer::tokenize(line) {
        Ok(tokens) => tokens,
        Err(e) => {
            eprintln!("huck: syntax error{}", lex_error_message(e));
            return ExecOutcome::Continue(2);
        }
    };
    let tokens = if expand_aliases {
        match crate::alias_expand::expand_aliases_in_tokens(tokens, &shell.aliases) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("huck: syntax error{}", lex_error_message(e));
                return ExecOutcome::Continue(2);
            }
        }
    } else {
        tokens
    };

    match command::parse(tokens) {
```

- [ ] **Step 1.11: Update process_line**

### Step 1.12: Update REPL caller in `src/shell.rs:71`

Find the REPL loop's call to `process_line` (around line 71). It
currently reads:

```rust
                match process_line(&buffer, &mut shell) {
```

Change to:

```rust
                let do_alias = shell.is_interactive;
                match process_line(&buffer, &mut shell, do_alias) {
```

The intermediate `let do_alias` avoids a borrow-checker error
(can't read `shell.is_interactive` while `&mut shell` is being
constructed for the call).

- [ ] **Step 1.12: Update REPL caller**

### Step 1.13: Update 3 trap-firing callers in `src/traps.rs`

In `src/traps.rs`, find the three `process_line` call sites (lines
70, 82, 116). Each currently reads:

```rust
        let _ = crate::shell::process_line(&action, shell);
```

Change each to:

```rust
        let _ = crate::shell::process_line(&action, shell, false);
```

(The `false` ensures trap actions do NOT undergo alias expansion,
matching bash's behavior.)

- [ ] **Step 1.13: Update trap callers**

### Step 1.14: Build + run full unit suite

Run: `cargo build`
Expected: clean.

Run: `cargo test --bin huck`
Expected: all unit tests pass (existing + 8 alias_expand + 9 alias
builtin tests).

If any existing test fails because it relied on the old
`process_line` signature: this is unlikely because existing tests
mostly call builtins directly via `run_builtin`, not `process_line`.
If a test does call `process_line` it needs the new third arg.

- [ ] **Step 1.14: Full unit suite green**

### Step 1.15: Clippy

Run: `cargo clippy --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 1.15: Clippy clean**

### Step 1.16: Commit

```bash
git add src/shell_state.rs src/alias_expand.rs src/main.rs src/builtins.rs src/shell.rs src/traps.rs
git commit -m "$(cat <<'EOF'
builtin: aliases (v48 task 1)

Add bash-style aliases to huck.

Foundation:
- New `pub aliases: HashMap<String, String>` field on Shell in
  src/shell_state.rs.
- New src/alias_expand.rs module with `expand_aliases_in_tokens`
  algorithm: walks tokens tracking command position, recursively
  substitutes aliases (cycle-protected via per-input `active`
  HashSet), supports bash trailing-space rule (alias body ending in
  whitespace makes the next word alias-eligible).

Builtins:
- `alias` (src/builtins.rs): no-args lists sorted; `name=value`
  defines; bare `name` shows one or errors "not found" + status 1;
  invalid names error.
- `unalias`: removes by name; `-a` clears all; missing name errors;
  no-args returns usage status 2.
- Both added to BUILTIN_NAMES and run_builtin dispatch.

Plumbing:
- process_line gains an `expand_aliases: bool` parameter. REPL
  caller in src/shell.rs:71 passes `shell.is_interactive`; the
  three trap-firing callers in src/traps.rs pass false. Matches
  bash defaults: aliases expand only for interactive REPL input,
  not for trap actions or non-interactive scripts.

8 unit tests in src/alias_expand.rs cover simple, post-pipe,
post-semi, recursive, cycle-protection, trailing-space, and
quoted-suppression cases plus no-expand-outside-command-position.
9 builtin tests in src/builtins.rs mod alias_tests cover the
alias / unalias paths including sorted listing, missing-name
errors, `-a` clear, and usage error.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 1.16: Commit Task 1**

---

## Task 2: Integration tests

**Files:**
- Create: `tests/aliases_integration.rs`

Three binary-driven tests.

### Step 2.1: Create the integration test file

Create `tests/aliases_integration.rs` with this content:

```rust
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run_capture(script: &str) -> (String, String) {
    let mut child = Command::new(huck_binary())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn alias_expansion_via_repl() {
    let script = "alias ll='echo HELLO'\nll\nexit\n";
    let (out, _) = run_capture(script);
    assert!(
        out.lines().any(|l| l == "HELLO"),
        "expected HELLO line in: {:?}",
        out
    );
}

#[test]
fn unalias_removes_expansion() {
    // After unalias, `ll` should NOT expand; the bare `ll` command
    // is not in PATH so it fails. Either status nonzero or stderr
    // contains "not found"/"command".
    let script =
        "alias ll='echo HELLO'\nunalias ll\nll\nrc=$?\necho rc=$rc\nexit\n";
    let (out, err) = run_capture(script);
    // rc=0 would mean the alias still expanded; we want rc != 0.
    let rc_line = out.lines().find(|l| l.starts_with("rc="));
    assert!(rc_line.is_some(), "no rc= line in: {:?}", out);
    let rc = rc_line.unwrap();
    assert_ne!(rc, "rc=0", "expected non-zero rc, got {rc}; stderr {:?}", err);
}

#[test]
fn recursive_alias_chain() {
    let script = "alias l='ll'\nalias ll='echo INNER'\nl\nexit\n";
    let (out, _) = run_capture(script);
    assert!(
        out.lines().any(|l| l == "INNER"),
        "expected INNER line in: {:?}",
        out
    );
}
```

- [ ] **Step 2.1: Create the file**

### Step 2.2: Run the integration suite

Run: `cargo test --test aliases_integration -- --nocapture`
Expected: all 3 tests pass.

Important: huck must detect interactive vs non-interactive stdin
correctly. `Command::new(...).stdin(Stdio::piped())` likely makes
stdin a pipe, not a TTY, so `shell.is_interactive == false` and
aliases would NOT expand in this test scenario.

If this is the case (tests fail because aliases don't expand): the
integration tests need a workaround. Options:
- Force interactive: pass an env var or flag to huck.
- Change the gate from `is_interactive` to always-on for now and
  document the divergence.
- Skip these integration tests and rely solely on the unit-level
  alias_expand tests + interactive smoke testing.

If `is_interactive` IS detected from stdin TTY (most likely),
modify Task 1 step 1.12 to pass `true` UNCONDITIONALLY for now,
OR introduce a `--interactive` huck flag that the integration test
can use, OR introduce an env-var override.

The cleanest path: introduce an env-var override `HUCK_EXPAND_ALIASES=1`
that forces alias expansion regardless of `is_interactive`. This
is a small addition to Task 1 — see step 2.2a below if you hit
this issue.

If tests pass without intervention, proceed to step 2.3.

- [ ] **Step 2.2: Tests pass**

### Step 2.2a (only if step 2.2 fails): Add env-var override

If integration tests fail because pipe-stdin disables alias
expansion: do NOT relax assertions. Instead, modify the REPL caller
in `src/shell.rs:71` to:

```rust
                let do_alias = shell.is_interactive
                    || std::env::var("HUCK_EXPAND_ALIASES").is_ok();
                match process_line(&buffer, &mut shell, do_alias) {
```

Then prefix each integration test script with the env var by
modifying `run_capture` to:

```rust
    let mut child = Command::new(huck_binary())
        .env("HUCK_EXPAND_ALIASES", "1")
        .stdin(Stdio::piped())
        ...
```

This is a test-only escape hatch — document it in the commit
message. Re-run step 2.2.

- [ ] **Step 2.2a: Env-var override (if needed)**

### Step 2.3: Full integration suite

Run: `cargo test --tests`
Expected: all integration tests pass. PTY flake tolerated.

- [ ] **Step 2.3: Full integration suite green**

### Step 2.4: Clippy

Run: `cargo clippy --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 2.4: Clippy clean**

### Step 2.5: Commit

```bash
git add tests/aliases_integration.rs
# If you took step 2.2a, also: git add src/shell.rs

git commit -m "$(cat <<'EOF'
test: aliases integration coverage (v48 task 2)

Three binary-driven tests verifying alias expansion end-to-end:
- alias_expansion_via_repl: `alias ll='echo HELLO'; ll` prints
  HELLO.
- unalias_removes_expansion: after `unalias ll`, the bare `ll`
  command no longer expands and exits with non-zero status.
- recursive_alias_chain: `alias l='ll'; alias ll='echo INNER'; l`
  prints INNER, exercising recursive expansion through the
  cycle-protected algorithm.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 2.5: Commit Task 2**

---

## Task 3: Docs

**Files:**
- Modify: `docs/bash-divergences.md` — add new M-63 entry,
  change-log entry.
- Modify: `README.md` — v48 row + trim "aliases" from "Not yet
  implemented" stanza.

### Step 3.1: Add M-63 entry in `docs/bash-divergences.md`

Find an appropriate section. Aliases are usually grouped under
"Builtins" or "Word expansion" — search the file for an existing
subsection that fits. If none exists, add a new `### Aliases`
subsection in Tier 2.

Add this entry:

```markdown
- **M-63: Aliases** — `[fixed v48]` medium. `alias name=value` defines; bare `alias` lists; `alias name` shows one. `unalias name` removes; `unalias -a` clears all. Aliases expand at command position before parsing (after pipes, `&&`, `||`, `;`, `&`, `(`, newlines). Recursive expansion supported with per-input cycle protection (alias `ls='ls --color'` doesn't infinite-loop). Bash trailing-space rule honored: `alias sudo='sudo '` makes the next word also alias-eligible. Expansion only fires for interactive REPL input (`shell.is_interactive == true`); trap actions, function bodies, and non-interactive script execution are unchanged (matches bash defaults).
```

- [ ] **Step 3.1: Add M-63 entry**

### Step 3.2: Add v48 change-log entry

In `docs/bash-divergences.md`, find `## Change log` and the most
recent `**2026-05-29**` entry (v47, M-62). Add IMMEDIATELY after
it:

```markdown
- **2026-05-29**: M-63 (aliases) shipped as v48. New `src/alias_expand.rs` module with `expand_aliases_in_tokens` algorithm — tracks command position, recursively substitutes via per-input cycle-protected `active` HashSet, supports the bash trailing-space rule. New `Shell.aliases: HashMap<String, String>` field. New `alias` and `unalias` builtins. `process_line` gained an `expand_aliases: bool` parameter so the REPL can pass `shell.is_interactive` while trap firings pass `false`. No new L-* divergences.
```

- [ ] **Step 3.2: Add change-log entry**

### Step 3.3: Add v48 row to README

In `README.md`, find the version table. After the v47 row (search
for `| v47       |`), add IMMEDIATELY after it:

```markdown
| v48       | Aliases (M-63)                                                 |
```

Match column padding to v47 (count actual trailing spaces in the
file).

- [ ] **Step 3.3: Add README v48 row**

### Step 3.4: Trim `aliases` from "Not yet implemented"

In `README.md`, find the block around lines ~233-238. Post-v47
should read:

```markdown
**Not yet implemented:**
backgrounded multi-pipeline sequences (`cmd1 && cmd2 &`), aliases.
```

Replace with:

```markdown
**Not yet implemented:**
backgrounded multi-pipeline sequences (`cmd1 && cmd2 &`).
```

- [ ] **Step 3.4: Trim README stanza**

### Step 3.5: Full suite

Run: `cargo test --all-targets`
Expected: all tests pass (modulo PTY flake).

- [ ] **Step 3.5: Full suite green**

### Step 3.6: Clippy

Run: `cargo clippy --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 3.6: Clippy clean**

### Step 3.7: Commit

```bash
git add docs/bash-divergences.md README.md
git commit -m "$(cat <<'EOF'
docs: add M-63 (aliases) fixed v48; trim stale entry

New M-63 entry in docs/bash-divergences.md tracks aliases as
[fixed v48]. Covers define/list/remove, recursive expansion with
cycle protection, the bash trailing-space rule, and the
interactive-only expansion gate.

Change log: 2026-05-29 v48 entry summarizing the new
alias_expand module, Shell.aliases storage, the alias / unalias
builtins, and the process_line signature change.

README: v48 row added to the version table; "Not yet implemented"
stanza trimmed to remove aliases (shipped this iteration).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 3.7: Commit Task 3**

---

## Final verification (controller, not a task)

After the three task commits land:

1. Run `cargo test --all-targets` once more.
2. Run `cargo clippy --all-targets -- -D warnings`.
3. Confirm the branch has exactly four commits ahead of `main`:
   docs preamble (spec + plan), task 1, task 2, task 3.
4. Dispatch a final cross-task code-reviewer subagent over the
   full diff (`main..v48-aliases`).
5. Merge to `main` with `--no-ff`, push, delete the branch, update
   the `huck iterations` memory with v48.
