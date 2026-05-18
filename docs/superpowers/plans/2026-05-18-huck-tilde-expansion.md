# Tilde Expansion (v9) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add bash-style tilde expansion (`~`, `~/path`, `~user`, `~+`, `~-`) including the assignment-context rule (`PATH=~/bin:~/lib`), and teach `cd` to maintain `PWD`/`OLDPWD`.

**Architecture:** Extend the existing `WordPart::Tilde` (unit variant) to carry a `TildeSpec { Home, User(String), Pwd, OldPwd }`. The lexer is the source of truth for where a `~` is eligible to expand and what flavor it is; `expand.rs` resolves each spec against `Shell` state, falling back to the literal form on miss. `cd` writes `PWD` and `OLDPWD` via `export_set`.

**Tech Stack:** Rust 2024 edition. `libc` for `getpwnam_r` (already in `Cargo.toml`). No new dependencies.

**Branch:** `feature/tilde-expansion` off `main`.

---

## Pre-flight

- [ ] **Step 0a: Create the feature branch from main**

```bash
git checkout main
git pull --ff-only
git checkout -b feature/tilde-expansion
```

- [ ] **Step 0b: Baseline — confirm clean build and all tests pass**

Run: `cargo build && cargo test`
Expected: clean build, `test result: ok. 258 passed; 0 failed`.

---

## Task 1: `TildeSpec` enum + full expansion logic

**Files:**
- Modify: `src/lexer.rs` — change `WordPart::Tilde` to carry a `TildeSpec` payload; add the `TildeSpec` enum; update the one production site to emit `Tilde(TildeSpec::Home)`. Update two existing tests that pattern-match the old unit form.
- Modify: `src/expand.rs` — both `WordPart::Tilde` arms become exhaustive `match` over `TildeSpec`; add helpers `resolve_tilde`, `render_tilde_literal`, `lookup_home_for_user`. Update the one existing test that pattern-matches `WordPart::Tilde`.
- Test: `src/lexer.rs`, `src/expand.rs`

This task does NOT extend the lexer to recognize `~+`/`~-`/`~user` — only the existing bare-`~` recognition. Tasks 2–4 will extend recognition. Doing the AST change up front means subsequent tasks only touch the lexer.

- [ ] **Step 1: Add `TildeSpec` and change the variant**

In `src/lexer.rs`, just above the `WordPart` enum, add:

```rust
#[derive(Debug, PartialEq, Eq)]
pub enum TildeSpec {
    Home,
    User(String),
    Pwd,
    OldPwd,
}
```

Change `WordPart::Tilde` from a unit to a tuple variant:

```rust
pub enum WordPart {
    Literal(String),
    Tilde(TildeSpec),
    Var { name: String, quoted: bool },
    LastStatus { quoted: bool },
    CommandSub { sequence: crate::command::Sequence, quoted: bool },
}
```

- [ ] **Step 2: Update the one production site in the lexer**

Around `src/lexer.rs:122`, change:

```rust
parts.push(WordPart::Tilde);
```

to:

```rust
parts.push(WordPart::Tilde(TildeSpec::Home));
```

- [ ] **Step 3: Update the two existing lexer tests**

In `src/lexer.rs::tests`, find `tokenize_tilde_alone` (around line 838) and update its expected value:

```rust
#[test]
fn tokenize_tilde_alone() {
    assert_eq!(
        tokenize("~").unwrap(),
        vec![Token::Word(Word(vec![WordPart::Tilde(TildeSpec::Home)]))]
    );
}
```

And `tokenize_tilde_slash_path` (around line 846):

```rust
#[test]
fn tokenize_tilde_slash_path() {
    assert_eq!(
        tokenize("~/foo").unwrap(),
        vec![Token::Word(Word(vec![
            WordPart::Tilde(TildeSpec::Home),
            WordPart::Literal("/foo".to_string()),
        ]))]
    );
}
```

- [ ] **Step 4: Build — confirm the AST change cascades**

Run: `cargo build`
Expected: errors in `src/expand.rs` (the two `WordPart::Tilde =>` arms no longer match the new shape). That's the cue for the next step.

- [ ] **Step 5: Add the three expansion helpers**

In `src/expand.rs`, near the top (after existing `use` lines), add:

```rust
use crate::lexer::TildeSpec;

fn resolve_tilde(spec: &TildeSpec, shell: &Shell) -> Option<String> {
    match spec {
        TildeSpec::Home   => shell.get("HOME").map(str::to_string),
        TildeSpec::Pwd    => shell.get("PWD").map(str::to_string),
        TildeSpec::OldPwd => shell.get("OLDPWD").map(str::to_string),
        TildeSpec::User(name) => lookup_home_for_user(name),
    }
}

fn render_tilde_literal(spec: &TildeSpec) -> String {
    match spec {
        TildeSpec::Home       => "~".to_string(),
        TildeSpec::Pwd        => "~+".to_string(),
        TildeSpec::OldPwd     => "~-".to_string(),
        TildeSpec::User(name) => format!("~{name}"),
    }
}

fn lookup_home_for_user(name: &str) -> Option<String> {
    use std::ffi::{CStr, CString};
    use std::mem::MaybeUninit;
    use std::ptr;

    let cname = CString::new(name).ok()?;
    let mut buf: Vec<u8> = vec![0; 1024];
    loop {
        let mut pwd: MaybeUninit<libc::passwd> = MaybeUninit::uninit();
        let mut result: *mut libc::passwd = ptr::null_mut();
        let rc = unsafe {
            libc::getpwnam_r(
                cname.as_ptr(),
                pwd.as_mut_ptr(),
                buf.as_mut_ptr() as *mut libc::c_char,
                buf.len(),
                &mut result,
            )
        };
        if rc == 0 && !result.is_null() {
            let pwd = unsafe { pwd.assume_init() };
            if pwd.pw_dir.is_null() {
                return None;
            }
            let home = unsafe { CStr::from_ptr(pwd.pw_dir) };
            return home.to_str().ok().map(str::to_string);
        }
        if rc == libc::ERANGE && buf.len() < 16384 {
            buf.resize(buf.len() * 2, 0);
            continue;
        }
        return None;
    }
}
```

- [ ] **Step 6: Update both `WordPart::Tilde` arms in `expand.rs`**

Find the `WordPart::Tilde => { … }` arm inside `pub fn expand` (around `src/expand.rs:26`) and replace with:

```rust
WordPart::Tilde(spec) => {
    let text = resolve_tilde(spec, shell)
        .unwrap_or_else(|| render_tilde_literal(spec));
    current.push_str(&text);
    has_emitted = true;
}
```

Find the `WordPart::Tilde => { … }` arm inside `pub fn expand_assignment` (around `src/expand.rs:77`) and replace with:

```rust
WordPart::Tilde(spec) => {
    let text = resolve_tilde(spec, shell)
        .unwrap_or_else(|| render_tilde_literal(spec));
    result.push_str(&text);
}
```

- [ ] **Step 7: Update the existing expand test**

Find `expand_tilde_uses_home` in `src/expand.rs::tests` (around line 265):

```rust
#[test]
fn expand_tilde_uses_home() {
    let mut shell = Shell::new();
    shell.export_set("HOME", "/home/test".to_string());
    let word = Word(vec![
        WordPart::Tilde(TildeSpec::Home),
        WordPart::Literal("/x".to_string()),
    ]);
    let result = expand(&word, &mut shell);
    assert_eq!(result, vec!["/home/test/x"]);
}
```

(Add `use crate::lexer::TildeSpec;` to `mod tests` if it isn't already imported transitively.)

- [ ] **Step 8: Add new tests for the fallback path**

Add to `src/expand.rs::tests`:

```rust
#[test]
fn expand_tilde_home_unset_falls_back_to_literal() {
    let mut shell = Shell::new();
    shell.unset("HOME");
    let word = Word(vec![WordPart::Tilde(TildeSpec::Home)]);
    assert_eq!(expand(&word, &mut shell), vec!["~"]);
}

#[test]
fn expand_tilde_pwd_resolves_when_pwd_set() {
    let mut shell = Shell::new();
    shell.export_set("PWD", "/var/tmp".to_string());
    let word = Word(vec![WordPart::Tilde(TildeSpec::Pwd)]);
    assert_eq!(expand(&word, &mut shell), vec!["/var/tmp"]);
}

#[test]
fn expand_tilde_pwd_unset_falls_back_to_literal_plus() {
    let mut shell = Shell::new();
    shell.unset("PWD");
    let word = Word(vec![WordPart::Tilde(TildeSpec::Pwd)]);
    assert_eq!(expand(&word, &mut shell), vec!["~+"]);
}

#[test]
fn expand_tilde_oldpwd_unset_falls_back_to_literal_minus() {
    let mut shell = Shell::new();
    shell.unset("OLDPWD");
    let word = Word(vec![WordPart::Tilde(TildeSpec::OldPwd)]);
    assert_eq!(expand(&word, &mut shell), vec!["~-"]);
}

#[test]
fn expand_tilde_unknown_user_falls_back_to_literal() {
    let mut shell = Shell::new();
    let word = Word(vec![
        WordPart::Tilde(TildeSpec::User("definitely_not_a_real_user_xyz_42".to_string())),
        WordPart::Literal("/x".to_string()),
    ]);
    assert_eq!(
        expand(&word, &mut shell),
        vec!["~definitely_not_a_real_user_xyz_42/x"]
    );
}

#[test]
fn expand_assignment_tilde_home_resolves() {
    let mut shell = Shell::new();
    shell.export_set("HOME", "/h".to_string());
    let word = Word(vec![
        WordPart::Literal("PATH=".to_string()),
        WordPart::Tilde(TildeSpec::Home),
        WordPart::Literal("/bin".to_string()),
    ]);
    assert_eq!(expand_assignment(&word, &mut shell), "PATH=/h/bin");
}
```

- [ ] **Step 9: Build and run all tests**

Run: `cargo build`
Expected: clean (the `libc::passwd`, `getpwnam_r`, and `ERANGE` references all resolve via the existing `libc` dep).

Run: `cargo test`
Expected: 258 baseline + 6 new = 264 passed. (Pre-existing tilde tests updated, not added.)

- [ ] **Step 10: Commit**

```bash
git add src/lexer.rs src/expand.rs
git commit -m "feat: TildeSpec enum + expansion fallback for all forms"
```

---

## Task 2: lexer recognizes `~+` and `~-`

**Files:**
- Modify: `src/lexer.rs` — extend the `'~'` arm to peek for `+` or `-`; widen `tilde_at_word_start` (or replace its caller).
- Test: `src/lexer.rs::tests`

After this task the lexer can produce `Tilde(Pwd)` and `Tilde(OldPwd)`. Expansion already works (Task 1).

- [ ] **Step 1: Write failing tests**

Add to `src/lexer.rs::tests`:

```rust
#[test]
fn tokenize_tilde_plus_alone() {
    assert_eq!(
        tokenize("~+").unwrap(),
        vec![Token::Word(Word(vec![WordPart::Tilde(TildeSpec::Pwd)]))]
    );
}

#[test]
fn tokenize_tilde_minus_alone() {
    assert_eq!(
        tokenize("~-").unwrap(),
        vec![Token::Word(Word(vec![WordPart::Tilde(TildeSpec::OldPwd)]))]
    );
}

#[test]
fn tokenize_tilde_plus_slash_path() {
    assert_eq!(
        tokenize("~+/x").unwrap(),
        vec![Token::Word(Word(vec![
            WordPart::Tilde(TildeSpec::Pwd),
            WordPart::Literal("/x".to_string()),
        ]))]
    );
}

#[test]
fn tokenize_tilde_minus_slash_path() {
    assert_eq!(
        tokenize("~-/x").unwrap(),
        vec![Token::Word(Word(vec![
            WordPart::Tilde(TildeSpec::OldPwd),
            WordPart::Literal("/x".to_string()),
        ]))]
    );
}

#[test]
fn tokenize_tilde_plus_followed_by_letter_is_literal() {
    // ~+abc is not a valid form; falls back to literal.
    assert_eq!(tokenize("~+abc").unwrap(), words(&["~+abc"]));
}
```

- [ ] **Step 2: Run — confirm failure**

Run: `cargo test lexer::tests::tokenize_tilde_plus lexer::tests::tokenize_tilde_minus`
Expected: FAIL — current lexer doesn't recognize `~+`/`~-` and treats them as literals.

- [ ] **Step 3: Extend the `'~'` arm**

In `src/lexer.rs`, locate the existing match arm at line ~120:

```rust
'~' if !has_token && tilde_at_word_start(&chars) => {
    has_token = true;
    parts.push(WordPart::Tilde(TildeSpec::Home));
}
```

Replace with a more capable matcher that peeks at the next character(s) and chooses the right `TildeSpec` (or falls through to literal):

```rust
'~' if !has_token => {
    if let Some(spec) = try_parse_tilde(&mut chars) {
        has_token = true;
        parts.push(WordPart::Tilde(spec));
    } else {
        // Fall through: treat '~' as literal.
        current.push('~');
        has_token = true;
    }
}
```

And replace the `tilde_at_word_start` helper (around line 447) with a new `try_parse_tilde` that returns `Option<TildeSpec>` and consumes any extra characters from the iterator on success:

```rust
/// Tries to consume a tilde construct starting just after the `~`.
/// On success, returns the `TildeSpec` (consuming any extra chars, e.g.
/// the `+` in `~+`). On failure, leaves the iterator untouched and
/// returns `None` (the caller treats `~` as a literal).
fn try_parse_tilde(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
) -> Option<TildeSpec> {
    match chars.peek().copied() {
        // Bare ~ at end of word.
        None => Some(TildeSpec::Home),
        Some(c) if is_tilde_terminator(c) => Some(TildeSpec::Home),
        // ~+, ~- — must be followed by terminator (or nothing).
        Some('+') => {
            let mut lookahead = chars.clone();
            lookahead.next(); // consume the +
            match lookahead.peek().copied() {
                None => { chars.next(); Some(TildeSpec::Pwd) }
                Some(c) if is_tilde_terminator(c) => { chars.next(); Some(TildeSpec::Pwd) }
                _ => None,
            }
        }
        Some('-') => {
            let mut lookahead = chars.clone();
            lookahead.next();
            match lookahead.peek().copied() {
                None => { chars.next(); Some(TildeSpec::OldPwd) }
                Some(c) if is_tilde_terminator(c) => { chars.next(); Some(TildeSpec::OldPwd) }
                _ => None,
            }
        }
        // ~user — Task 3 will handle this; for now fall through.
        _ => None,
    }
}

fn is_tilde_terminator(c: char) -> bool {
    c == '/'
        || c.is_whitespace()
        || matches!(c, '|' | '<' | '>' | '&' | ';')
}
```

The old `tilde_at_word_start` helper is now unused — delete it.

- [ ] **Step 4: Run tests**

Run: `cargo test lexer::tests::tokenize_tilde`
Expected: all green (new + pre-existing tilde tests).

Run: `cargo test`
Expected: 269 passed (264 + 5 new).

- [ ] **Step 5: Commit**

```bash
git add src/lexer.rs
git commit -m "feat(lexer): recognize ~+ and ~- tilde forms"
```

---

## Task 3: lexer recognizes `~user`

**Files:**
- Modify: `src/lexer.rs::try_parse_tilde` — add the `~user` arm.
- Modify: `src/lexer.rs::tests` — update `tokenize_tilde_followed_by_name_is_literal` (its premise changes) and add new positive tests.
- Test: `src/lexer.rs::tests`

- [ ] **Step 1: Write failing tests**

Add to `src/lexer.rs::tests`:

```rust
#[test]
fn tokenize_tilde_user_alone() {
    assert_eq!(
        tokenize("~alice").unwrap(),
        vec![Token::Word(Word(vec![
            WordPart::Tilde(TildeSpec::User("alice".to_string())),
        ]))]
    );
}

#[test]
fn tokenize_tilde_user_slash_path() {
    assert_eq!(
        tokenize("~alice/bin").unwrap(),
        vec![Token::Word(Word(vec![
            WordPart::Tilde(TildeSpec::User("alice".to_string())),
            WordPart::Literal("/bin".to_string()),
        ]))]
    );
}

#[test]
fn tokenize_tilde_user_with_underscore_and_digits() {
    assert_eq!(
        tokenize("~alice_123").unwrap(),
        vec![Token::Word(Word(vec![
            WordPart::Tilde(TildeSpec::User("alice_123".to_string())),
        ]))]
    );
}
```

Also update the pre-existing `tokenize_tilde_followed_by_name_is_literal` (around line 862) — its claim no longer holds:

```rust
#[test]
fn tokenize_tilde_followed_by_name_is_user_form() {
    assert_eq!(
        tokenize("~foo").unwrap(),
        vec![Token::Word(Word(vec![
            WordPart::Tilde(TildeSpec::User("foo".to_string())),
        ]))]
    );
}
```

Rename the test from `tokenize_tilde_followed_by_name_is_literal`.

- [ ] **Step 2: Run — confirm failure**

Run: `cargo test lexer::tests::tokenize_tilde_user lexer::tests::tokenize_tilde_followed_by_name`
Expected: FAIL — current `try_parse_tilde` returns `None` for the `_ => None` arm.

- [ ] **Step 3: Add the `~user` arm**

In `src/lexer.rs::try_parse_tilde`, replace the catch-all `_ => None,` with:

```rust
Some(c) if is_user_name_start(c) => {
    // Scan a maximal identifier; the tail after must be a terminator.
    let mut lookahead = chars.clone();
    let mut name = String::new();
    while let Some(&nc) = lookahead.peek() {
        if is_user_name_continue(nc) {
            name.push(nc);
            lookahead.next();
        } else {
            break;
        }
    }
    let tail_ok = match lookahead.peek().copied() {
        None => true,
        Some(c) => is_tilde_terminator(c),
    };
    if tail_ok && !name.is_empty() {
        // Consume the scanned chars from the real iterator.
        for _ in 0..name.len() {
            chars.next();
        }
        Some(TildeSpec::User(name))
    } else {
        None
    }
}
_ => None,
```

Add the two character-class helpers near `is_tilde_terminator`:

```rust
fn is_user_name_start(c: char) -> bool {
    c == '_' || c.is_ascii_alphabetic()
}

fn is_user_name_continue(c: char) -> bool {
    c == '_' || c.is_ascii_alphanumeric()
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test lexer::tests::tokenize_tilde`
Expected: all green.

Run: `cargo test`
Expected: 272 passed (269 + 3 new — the pre-existing test was renamed, not added).

- [ ] **Step 5: Commit**

```bash
git add src/lexer.rs
git commit -m "feat(lexer): recognize ~user tilde form"
```

---

## Task 4: assignment-context tilde recognition

**Files:**
- Modify: `src/lexer.rs` — track whether the current word is an assignment-context word; recognize `~` after unquoted `:` or `=` when the flag is on.
- Test: `src/lexer.rs::tests`

- [ ] **Step 1: Write failing tests**

Add to `src/lexer.rs::tests`:

```rust
#[test]
fn tokenize_assignment_value_expands_first_tilde_after_equals() {
    assert_eq!(
        tokenize("PATH=~/bin").unwrap(),
        vec![Token::Word(Word(vec![
            WordPart::Literal("PATH=".to_string()),
            WordPart::Tilde(TildeSpec::Home),
            WordPart::Literal("/bin".to_string()),
        ]))]
    );
}

#[test]
fn tokenize_assignment_value_expands_each_tilde_after_colon() {
    assert_eq!(
        tokenize("PATH=~/bin:~/lib").unwrap(),
        vec![Token::Word(Word(vec![
            WordPart::Literal("PATH=".to_string()),
            WordPart::Tilde(TildeSpec::Home),
            WordPart::Literal("/bin:".to_string()),
            WordPart::Tilde(TildeSpec::Home),
            WordPart::Literal("/lib".to_string()),
        ]))]
    );
}

#[test]
fn tokenize_non_assignment_colon_tilde_stays_literal() {
    // `echo` is not an assignment, so `a:~/b` does NOT expand the tilde.
    assert_eq!(
        tokenize("echo a:~/b").unwrap(),
        words(&["echo", "a:~/b"])
    );
}

#[test]
fn tokenize_assignment_with_digit_first_is_not_assignment_context() {
    // `1ABC=~/x` doesn't match identifier-start; treated as literal.
    assert_eq!(
        tokenize("1ABC=~/x").unwrap(),
        words(&["1ABC=~/x"])
    );
}

#[test]
fn tokenize_assignment_value_tilde_user() {
    assert_eq!(
        tokenize("HOMES=~alice:~bob").unwrap(),
        vec![Token::Word(Word(vec![
            WordPart::Literal("HOMES=".to_string()),
            WordPart::Tilde(TildeSpec::User("alice".to_string())),
            WordPart::Literal(":".to_string()),
            WordPart::Tilde(TildeSpec::User("bob".to_string())),
        ]))]
    );
}
```

- [ ] **Step 2: Run — confirm failure**

Run: `cargo test lexer::tests::tokenize_assignment_value lexer::tests::tokenize_non_assignment_colon`
Expected: FAIL — current lexer only recognizes tilde at word start.

- [ ] **Step 3: Add assignment-context detection**

In `src/lexer.rs::tokenize`, near the top of the main `while` loop, add an `in_assignment_value` flag that starts as `false` each new word and flips to `true` once we've emitted the assignment-prefix `Literal` (the `NAME=` part).

The detection: at the start of a word, lookahead to see if the unquoted prefix matches `[A-Za-z_]\w*=`. If yes, mark a pending state; once the `=` is consumed, set `in_assignment_value = true`.

Concrete edit shape (the existing tokenize loop is long; this is the relevant addition):

a. Just above the `while let Some(c) = chars.next() { ... }` block, add:

```rust
let mut in_assignment_value = false;
```

(Variable scope: the outer fn `tokenize`. It resets on each word boundary — see steps c and d below.)

b. Replace the current top-of-word lookahead. When `has_token == false` and we're about to consume the first char of a new word, check for assignment shape:

```rust
if !has_token && looks_like_assignment_prefix(&chars) {
    // The lookahead inside looks_like_assignment_prefix doesn't consume;
    // we'll detect the `=` below and flip the flag.
    // No-op here; flag flips when we consume the `=`.
}
```

Actually the simpler design: track `in_assignment_value` purely from observed state. Maintain it as: starts `false`; flips to `true` the first time we consume an `=` *iff* the literal portion of the current word so far is a valid identifier (`[A-Za-z_]\w*`).

So inside the `'='` handling (currently `=` is consumed as a literal char), check the current state of the word:

```rust
'=' if !in_assignment_value => {
    // If `current + parts so far` represents exactly a valid identifier,
    // we're entering assignment-value context.
    if !in_quotes && word_is_identifier_so_far(&current, &parts) {
        in_assignment_value = true;
    }
    current.push('=');
    has_token = true;
}
```

(Note: `'='` is not a separator today; it's just a literal char. The `in_quotes` check is the existing single/double-quote tracking — adapt to whatever the surrounding code uses.)

Implement `word_is_identifier_so_far`:

```rust
/// True iff the unquoted text accumulated so far for the current word
/// forms a valid shell identifier (matches [A-Za-z_]\w*).
fn word_is_identifier_so_far(current: &str, parts: &[WordPart]) -> bool {
    // The word so far must be exactly `parts ++ current` where every
    // WordPart is a `Literal(s)` with no other variants, AND the
    // concatenation is a non-empty identifier.
    let mut joined = String::new();
    for p in parts {
        if let WordPart::Literal(s) = p {
            joined.push_str(s);
        } else {
            return false;
        }
    }
    joined.push_str(current);
    if joined.is_empty() {
        return false;
    }
    let mut iter = joined.chars();
    let first = iter.next().unwrap();
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return false;
    }
    iter.all(|c| c == '_' || c.is_ascii_alphanumeric())
}
```

c. Extend the `'~'` arm to also fire when we're in assignment-value context AND the immediately preceding character (in `current`) is `:` or `=`. Replace:

```rust
'~' if !has_token => {
    // ... existing try_parse_tilde call ...
}
```

with:

```rust
'~' if !has_token || tilde_eligible_in_assignment(in_assignment_value, &current) => {
    if let Some(spec) = try_parse_tilde(&mut chars) {
        if !current.is_empty() {
            parts.push(WordPart::Literal(std::mem::take(&mut current)));
        }
        has_token = true;
        parts.push(WordPart::Tilde(spec));
    } else {
        current.push('~');
        has_token = true;
    }
}
```

Add the helper:

```rust
fn tilde_eligible_in_assignment(in_assignment_value: bool, current: &str) -> bool {
    if !in_assignment_value {
        return false;
    }
    // Eligible immediately after an unquoted `:` or `=`.
    matches!(current.chars().last(), Some(':') | Some('='))
}
```

d. Reset `in_assignment_value = false` at every word boundary (where `tokens.push(Token::Word(...))` happens after whitespace or operator).

- [ ] **Step 4: Run tests**

Run: `cargo test lexer::tests::tokenize_assignment lexer::tests::tokenize_non_assignment lexer::tests::tokenize_tilde`
Expected: all green.

Run: `cargo test`
Expected: 277 passed (272 + 5 new).

- [ ] **Step 5: Commit**

```bash
git add src/lexer.rs
git commit -m "feat(lexer): expand ~ after : and = in assignment-context words"
```

---

## Task 5: `cd` maintains `PWD` and `OLDPWD`

**Files:**
- Modify: `src/builtins.rs::builtin_cd`
- Test: `src/builtins.rs::tests`

- [ ] **Step 1: Write failing tests**

Add to `src/builtins.rs::tests` (in the existing `mod tests` or a new submodule):

```rust
#[cfg(test)]
mod cd_pwd_tests {
    use super::*;
    use crate::shell_state::Shell;

    #[test]
    fn cd_sets_pwd_to_target_directory() {
        let mut shell = Shell::new();
        let prev = std::env::current_dir().unwrap();
        let outcome = builtin_cd(&["/tmp".to_string()], &mut shell);
        // Restore for any other tests.
        let _ = std::env::set_current_dir(&prev);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.get("PWD"), Some("/tmp"));
        assert!(shell.exported_env().any(|(k, _)| k == "PWD"));
    }

    #[test]
    fn cd_sets_oldpwd_to_previous_pwd() {
        let mut shell = Shell::new();
        shell.export_set("PWD", "/var".to_string());
        let prev = std::env::current_dir().unwrap();
        let outcome = builtin_cd(&["/tmp".to_string()], &mut shell);
        let _ = std::env::set_current_dir(&prev);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.get("OLDPWD"), Some("/var"));
        assert!(shell.exported_env().any(|(k, _)| k == "OLDPWD"));
    }

    #[test]
    fn cd_with_pwd_initially_unset_does_not_set_oldpwd() {
        let mut shell = Shell::new();
        shell.unset("PWD");
        shell.unset("OLDPWD");
        let prev = std::env::current_dir().unwrap();
        let outcome = builtin_cd(&["/tmp".to_string()], &mut shell);
        let _ = std::env::set_current_dir(&prev);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.get("OLDPWD"), None);
        assert_eq!(shell.get("PWD"), Some("/tmp"));
    }
}
```

Note: these tests change the process's current directory. They restore via `set_current_dir(prev)` after the call. They may interact with other tests that read `current_dir`; if cargo's test runner parallelism becomes a problem, mark them `#[serial]` (would require the `serial_test` crate — not in this iteration).

- [ ] **Step 2: Run — confirm failure**

Run: `cargo test builtins::tests::cd_pwd_tests`
Expected: FAIL — `cd` doesn't currently update PWD/OLDPWD.

- [ ] **Step 3: Update `builtin_cd`**

Replace the existing `builtin_cd` in `src/builtins.rs` (around line 49):

```rust
fn builtin_cd(args: &[String], shell: &mut Shell) -> ExecOutcome {
    if args.len() > 1 {
        eprintln!("huck: cd: too many arguments");
        return ExecOutcome::Continue(1);
    }
    let target = match args.first() {
        Some(dir) => dir.clone(),
        None => match shell.get("HOME") {
            Some(home) => home.to_string(),
            None => {
                eprintln!("huck: cd: HOME not set");
                return ExecOutcome::Continue(1);
            }
        },
    };
    if let Err(e) = env::set_current_dir(Path::new(&target)) {
        eprintln!("huck: cd: {target}: {e}");
        return ExecOutcome::Continue(1);
    }
    // chdir succeeded — maintain PWD/OLDPWD.
    let prev_pwd = shell.get("PWD").map(str::to_string);
    match env::current_dir() {
        Ok(new_pwd) => {
            if let Some(prev) = prev_pwd {
                shell.export_set("OLDPWD", prev);
            }
            shell.export_set("PWD", new_pwd.to_string_lossy().to_string());
        }
        Err(e) => {
            // chdir succeeded but we can't read it back — warn but
            // don't fail the command.
            eprintln!("huck: cd: warning: could not read current dir: {e}");
        }
    }
    ExecOutcome::Continue(0)
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test builtins::tests::cd_pwd_tests`
Expected: all 3 PASS.

Run: `cargo test`
Expected: 280 passed (277 + 3 new).

- [ ] **Step 5: Commit**

```bash
git add src/builtins.rs
git commit -m "feat(cd): maintain PWD and OLDPWD after successful chdir"
```

---

## Task 6: Smoke test against a real terminal

**Files:** none (manual verification only)

- [ ] **Step 1: Build the release binary**

Run: `cargo build --release`
Expected: clean build at `target/release/huck`.

- [ ] **Step 2: Walk the smoke-test checklist**

Run `target/release/huck` in a real terminal and verify each:

1. **Bare `~` expands.**
   `cd ~` → moves to `$HOME`.
   `echo ~` → prints `$HOME`.

2. **`~/path` expands.**
   `ls ~/Downloads` (or any existing subdir) → works.

3. **`~+` resolves to current dir.**
   `cd /tmp; echo ~+` → prints `/tmp`.

4. **`~-` resolves to previous dir.**
   `cd /var; cd /tmp; cd ~-` → moves back to `/var`. Then `echo ~-` → prints `/tmp`.

5. **`~user` resolves via passwd.**
   `echo ~root` → prints `/root` (or whatever root's home is on the system).

6. **Unknown user falls back to literal.**
   `echo ~definitely_not_a_real_user_xyz_42` → prints the literal text.

7. **Assignment-context expansion.**
   `PATH=~/bin:~/lib echo $PATH` → expands both tildes.

8. **Non-assignment colon is literal.**
   `echo a:~/b` → prints literal `a:~/b`.

9. **Quoted tilde stays literal.**
   `echo '~'` → prints `~`. `echo "~"` → prints `~`.

10. **`HOME` unset falls back to literal `~`.**
    `unset HOME; echo ~` → prints `~`.

- [ ] **Step 3: Note any failures**

If anything fails, file a fix task and address before merge. Do not commit in this task.

---

## Wrap-up

- [ ] **Final cross-branch review**

Dispatch `feature-dev:code-reviewer` over the full diff:

```bash
git diff main...HEAD
```

Focus areas:
- `try_parse_tilde` correctness on iterator clone/consume patterns.
- `lookup_home_for_user` unsafe handling, especially the ERANGE retry loop bound.
- Assignment-context flag lifetime: is it reset at every word boundary including operator-terminated ones?
- `cd` warning behavior when `current_dir()` fails mid-chdir.
- No regressions on existing lexer / expand tests.

- [ ] **Apply review fixes; merge to main**

Match the v6/v7/v8 merge flow: `--no-ff` merge, delete the branch, push.

---

## Self-review notes

**Spec coverage:**
- §1 (TildeSpec + lexer recognition) → Tasks 1, 2, 3, 4.
- §2 (Expansion) → Task 1 (covers all four spec variants since match must be exhaustive).
- §3 (cd PWD/OLDPWD) → Task 5.
- §4 (edge cases) → distributed across Tasks 1–5 tests + smoke test (Task 6).
- §5 (testing) → Tasks 1, 2, 3, 4, 5 each include unit tests; Task 6 covers manual smoke.
- §6 (files) → matches Tasks 1–5 file lists.
- §7 (future) → no implementation; deferral documented in spec.

**Placeholder scan:** No TBD/TODO. Every code step shows the actual code.

**Type consistency:**
- `TildeSpec` and `WordPart::Tilde(TildeSpec)` defined in Task 1; used unchanged in Tasks 2, 3, 4.
- `try_parse_tilde(&mut Peekable<Chars>) -> Option<TildeSpec>` defined in Task 2; extended in Task 3 with a new arm (catch-all replaced).
- `is_tilde_terminator`, `is_user_name_start`, `is_user_name_continue`, `tilde_eligible_in_assignment`, `word_is_identifier_so_far`, `lookup_home_for_user`, `resolve_tilde`, `render_tilde_literal` — each defined exactly once.

**Test-name collisions:** Pre-existing `tokenize_tilde_alone`, `tokenize_tilde_slash_path`, `tokenize_tilde_followed_by_name_is_literal`, `tokenize_tilde_in_quotes_is_literal`, and `expand_tilde_uses_home` are explicitly updated/renamed in Tasks 1 and 3. The `tokenize_tilde_mid_word_is_literal` test (`a~b`) keeps its current name and meaning (mid-word `~` doesn't expand even after assignment-context support).
