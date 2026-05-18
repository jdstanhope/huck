# huck v10: Pathname Expansion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add POSIX basic pathname expansion (`*`, `?`, `[abc]`) to huck command arguments, with per-character quoting tracking so quoted metacharacters stay literal.

**Architecture:** Lexer tracks per-`Literal` quoting source. `expand` returns `Vec<Field>` where each `Field` carries chars + parallel `quoted: Vec<bool>`. A new `glob_expand_fields` post-step converts fields to final argv strings using the `glob` crate, with bash-compatible defaults (no-match→literal, dotfile exclusion, no separator crossing).

**Tech Stack:** Rust 2024 edition, `glob = "0.3"` crate, `tempfile` (dev-dep for tests).

**Reference:** Design spec at `docs/superpowers/specs/2026-05-18-huck-pathname-expansion-design.md`.

---

## File Map

- `Cargo.toml` — add `glob` runtime dep, `tempfile` dev-dep
- `src/lexer.rs` — change `WordPart::Literal(String)` → `WordPart::Literal { text: String, quoted: bool }`; flush at quote boundaries
- `src/expand.rs` — new `Field` struct, `expand` returns `Vec<Field>`, new `glob_expand_fields` function
- `src/executor.rs` — call `glob_expand_fields` after `expand` when assembling argv
- `src/builtins.rs` — no change to dispatch (it receives `Vec<String>`); only call-site updates if any
- `tests/glob_integration.rs` — new end-to-end test file
- `README.md` — v10 row, features text

---

## Task 1: Add `glob` dependency

**Files:**
- Modify: `Cargo.toml`

- [ ] **Step 1: Add the runtime dependency**

Edit `Cargo.toml` `[dependencies]` table:

```toml
glob = "0.3"
```

Add `[dev-dependencies]` section if absent:

```toml
[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 2: Verify build**

Run: `cargo build`
Expected: success, downloads `glob` and `tempfile`.

- [ ] **Step 3: Run full test suite**

Run: `cargo test`
Expected: all existing tests pass (no behavior change yet).

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "v10 task 1: add glob and tempfile dependencies"
```

---

## Task 2: Add `quoted` flag to `WordPart::Literal`

Change the variant shape and update the lexer to flush at every quote boundary so each `Literal` is purely quoted or purely unquoted. Expand is updated mechanically to read `.text` and ignore `.quoted` for now.

**Files:**
- Modify: `src/lexer.rs` (variant definition, `flush_literal`, quote handlers, every test that constructs Literal)
- Modify: `src/expand.rs` (mechanical: read `.text` instead of `.0`)

- [ ] **Step 1: Write a failing lexer test for quote boundary flush**

Add to `src/lexer.rs` `#[cfg(test)] mod tests`:

```rust
#[test]
fn tokenize_mixed_quoted_unquoted_flushes_at_boundaries() {
    let tokens = tokenize("foo\"bar\"baz").unwrap();
    assert_eq!(tokens.len(), 1);
    let Token::Word(Word(parts)) = &tokens[0] else { panic!() };
    assert_eq!(parts.len(), 3);
    assert_eq!(parts[0], WordPart::Literal { text: "foo".to_string(), quoted: false });
    assert_eq!(parts[1], WordPart::Literal { text: "bar".to_string(), quoted: true });
    assert_eq!(parts[2], WordPart::Literal { text: "baz".to_string(), quoted: false });
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test tokenize_mixed_quoted_unquoted_flushes_at_boundaries`
Expected: FAIL (compile error — `Literal` variant shape mismatch).

- [ ] **Step 3: Change the variant definition**

Edit `src/lexer.rs` around line 35:

```rust
pub enum WordPart {
    Literal { text: String, quoted: bool },
    Tilde(TildeSpec),
    Var { name: String, quoted: bool },
    LastStatus { quoted: bool },
    CommandSub { sequence: crate::command::Sequence, quoted: bool },
}
```

- [ ] **Step 4: Update `flush_literal` to take a `quoted` flag**

Find `flush_literal` (around `src/lexer.rs:247`). Change signature so callers pass quotedness:

```rust
fn flush_literal(parts: &mut Vec<WordPart>, current: &mut String, quoted: bool) {
    if !current.is_empty() {
        parts.push(WordPart::Literal {
            text: std::mem::take(current),
            quoted,
        });
    }
}
```

(Remove the empty-literal fallback at line 251 if present — only flush non-empty buffers. If any caller relied on a guaranteed-empty literal, audit and adjust.)

- [ ] **Step 5: Add a separate quoted-text buffer in `tokenize`**

In `tokenize`, introduce a separate `String` buffer used inside `'...'` and `"..."` regions. Before entering either quote, call `flush_literal(&mut parts, &mut current, false)`. On quote exit, call `flush_literal(&mut parts, &mut quoted_current, true)`. The existing mid-quote `parts.push(WordPart::Literal(...))` flushes for `$` / backtick (lines ~97-98, 104-105) become `flush_literal(&mut parts, &mut quoted_current, true)`.

Concretely for the single-quote arm (around line 71):

```rust
'\'' => {
    has_token = true;
    flush_literal(&mut parts, &mut current, false);
    loop {
        match chars.next() {
            Some('\'') => break,
            Some(ch) => quoted_current.push(ch),
            None => return Err(LexError::UnterminatedQuote),
        }
    }
    flush_literal(&mut parts, &mut quoted_current, true);
}
```

And for the double-quote arm, replace every `current.push(...)` inside the loop with `quoted_current.push(...)`, replace the mid-loop `parts.push(WordPart::Literal(std::mem::take(&mut current)))` with `flush_literal(&mut parts, &mut quoted_current, true)`, and after the loop call `flush_literal(&mut parts, &mut quoted_current, true)` to drain trailing quoted text.

- [ ] **Step 6: Update every remaining literal-construction call site**

Run: `grep -n 'WordPart::Literal' src/lexer.rs`

For each non-test site, decide quotedness from context (lexer code outside the quote arms is processing bare text → `quoted: false`). For example line 283:

```rust
parts.push(WordPart::Literal { text: "$".to_string(), quoted: false });
```

Update all `flush_literal(&mut parts, &mut current)` calls (lines 62, 126, 133, 146) to `flush_literal(&mut parts, &mut current, false)`.

- [ ] **Step 7: Update every existing lexer test**

Run: `grep -n 'WordPart::Literal(' src/lexer.rs`

Mechanically rewrite each `WordPart::Literal("text".to_string())` to `WordPart::Literal { text: "text".to_string(), quoted: false }` for bare text and `quoted: true` for tests that exercise quoted input. Use the test's input string to decide — text taken from inside `"..."` or `'...'` is quoted.

- [ ] **Step 8: Update `expand.rs` to read `.text`**

Run: `grep -n 'WordPart::Literal' src/expand.rs`

Each `WordPart::Literal(s)` pattern becomes `WordPart::Literal { text, .. }`. The `quoted` flag is unused this task.

- [ ] **Step 9: Verify build and full test suite**

Run: `cargo build && cargo test`
Expected: all tests pass, including the new boundary-flush test.

- [ ] **Step 10: Commit**

```bash
git add src/lexer.rs src/expand.rs
git commit -m "v10 task 2: add quoted flag to WordPart::Literal, flush at quote boundaries"
```

---

## Task 3: Introduce `Field` type

Define the `Field` struct in `expand.rs`. Not yet returned by `expand`; isolated unit tests only.

**Files:**
- Modify: `src/expand.rs`

- [ ] **Step 1: Write failing tests for `Field`**

Add to `src/expand.rs` `#[cfg(test)] mod tests`:

```rust
#[test]
fn field_from_unquoted_str_marks_all_chars_unquoted() {
    let f = Field::from_unquoted("abc");
    assert_eq!(f.chars, "abc");
    assert_eq!(f.quoted, vec![false, false, false]);
}

#[test]
fn field_from_quoted_str_marks_all_chars_quoted() {
    let f = Field::from_quoted("xy");
    assert_eq!(f.chars, "xy");
    assert_eq!(f.quoted, vec![true, true]);
}

#[test]
fn field_push_str_appends_chars_with_quoted_flag() {
    let mut f = Field::from_unquoted("a");
    f.push_str("bc", true);
    assert_eq!(f.chars, "abc");
    assert_eq!(f.quoted, vec![false, true, true]);
}

#[test]
fn field_quoted_vec_uses_char_count_not_byte_count() {
    // Multi-byte char: should produce 1 quoted entry, not the UTF-8 byte count.
    let f = Field::from_unquoted("é");
    assert_eq!(f.chars.chars().count(), 1);
    assert_eq!(f.quoted.len(), 1);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test field_`
Expected: FAIL (compile error — `Field` does not exist).

- [ ] **Step 3: Define `Field` and constructors**

Add near the top of `src/expand.rs` (above `pub fn expand`):

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub chars: String,
    pub quoted: Vec<bool>,
}

impl Field {
    pub fn new() -> Self {
        Self { chars: String::new(), quoted: Vec::new() }
    }

    pub fn from_unquoted(s: &str) -> Self {
        let count = s.chars().count();
        Self { chars: s.to_string(), quoted: vec![false; count] }
    }

    pub fn from_quoted(s: &str) -> Self {
        let count = s.chars().count();
        Self { chars: s.to_string(), quoted: vec![true; count] }
    }

    pub fn push_str(&mut self, s: &str, quoted: bool) {
        let count = s.chars().count();
        self.chars.push_str(s);
        self.quoted.extend(std::iter::repeat(quoted).take(count));
    }

    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }
}

impl Default for Field {
    fn default() -> Self {
        Self::new()
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test field_`
Expected: PASS (4 new tests).

- [ ] **Step 5: Commit**

```bash
git add src/expand.rs
git commit -m "v10 task 3: add Field type with per-char quoting"
```

---

## Task 4: Change `expand` signature to `Vec<Field>`

Rewrite `expand` internally to build `Vec<Field>` and update callers. Quoting propagation correctness lands in Task 5; this task only changes the return type and makes the simplest possible per-kind quoting choices (all unquoted) — existing tests still need to pass with the `chars` projection.

**Files:**
- Modify: `src/expand.rs` (signature, internals, all internal tests)
- Modify: `src/executor.rs` (call sites at lines 297, 307, 316)

- [ ] **Step 1: Change the signature**

Edit `src/expand.rs` line 63:

```rust
pub fn expand(word: &Word, shell: &mut Shell) -> Vec<Field> {
```

- [ ] **Step 2: Rewrite the body to produce `Vec<Field>`**

Replace the existing `expand` body and the `emit_split` helper so that:
- The `current` accumulator becomes a `Field` (not a `String`).
- `emit_split` becomes `emit_split_fields` and pushes `Field`s, not `String`s.
- `WordPart::Literal { text, .. }` extends `current` via `current.push_str(&text, false)`. (The `.quoted` flag is wired up properly in Task 5.)
- `WordPart::Tilde(spec)` resolves to a string and is appended with `quoted: false`.
- `WordPart::Var { name, quoted: true }` and `LastStatus { quoted: true }` append with `quoted: false` for now.
- `WordPart::Var { name, quoted: false }` and unquoted `LastStatus` / `CommandSub` still go through split-emit, producing one `Field` per IFS fragment (all chars unquoted).
- `WordPart::CommandSub { sequence, quoted: true }` appends with `quoted: false` for now.

Sketch of the new `emit_split_fields`:

```rust
fn emit_split_fields(
    value: &str,
    current: &mut Field,
    result: &mut Vec<Field>,
    has_emitted: &mut bool,
) {
    let fragments: Vec<&str> = value.split_ascii_whitespace().collect();
    if fragments.is_empty() {
        return;
    }
    // First fragment continues the in-progress field.
    current.push_str(fragments[0], false);
    // Each subsequent fragment closes the field and starts a new one.
    for frag in &fragments[1..] {
        let finished = std::mem::take(current);
        result.push(finished);
        *has_emitted = true;
        current.push_str(frag, false);
    }
}
```

End-of-word logic: push `current` if it is non-empty OR if no field has been emitted yet (preserves the existing "empty literal still emits one empty string" behavior for `expand(&lit(""), ...)`).

- [ ] **Step 3: Update internal `expand` tests to assert against `Field`s**

Each test like `assert_eq!(expand(...), vec!["foo".to_string()])` becomes:

```rust
let fields = expand(...);
let strings: Vec<String> = fields.into_iter().map(|f| f.chars).collect();
assert_eq!(strings, vec!["foo".to_string()]);
```

Add a tiny helper inside the test module:

```rust
fn expand_strings(word: &Word, shell: &mut Shell) -> Vec<String> {
    expand(word, shell).into_iter().map(|f| f.chars).collect()
}
```

then rewrite the existing tests to use it. Mechanical change.

- [ ] **Step 4: Update executor call sites**

`src/executor.rs:297`, `:307`, `:316` currently bind `let fields = expand(word, shell)` and use them as `Vec<String>`. For this task, project back to `Vec<String>`:

```rust
let fields: Vec<String> = expand(word, shell).into_iter().map(|f| f.chars).collect();
```

(In Task 9 we replace this projection with `glob_expand_fields`.)

- [ ] **Step 5: Verify build and full test suite**

Run: `cargo build && cargo test`
Expected: all tests pass.

- [ ] **Step 6: Commit**

```bash
git add src/expand.rs src/executor.rs
git commit -m "v10 task 4: expand returns Vec<Field>, callers project to strings for now"
```

---

## Task 5: Quoting propagation per WordPart kind

Wire the actual quoting source from each `WordPart` into the emitted `Field`s, per the spec table.

**Files:**
- Modify: `src/expand.rs`

- [ ] **Step 1: Write failing tests for propagation**

Add to `src/expand.rs` test module:

```rust
#[test]
fn expand_literal_unquoted_marks_chars_unquoted() {
    let mut shell = Shell::new();
    let word = Word(vec![WordPart::Literal { text: "abc".to_string(), quoted: false }]);
    let fields = expand(&word, &mut shell);
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].quoted, vec![false, false, false]);
}

#[test]
fn expand_literal_quoted_marks_chars_quoted() {
    let mut shell = Shell::new();
    let word = Word(vec![WordPart::Literal { text: "abc".to_string(), quoted: true }]);
    let fields = expand(&word, &mut shell);
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].quoted, vec![true, true, true]);
}

#[test]
fn expand_mixed_quoted_unquoted_literal_parts() {
    let mut shell = Shell::new();
    let word = Word(vec![
        WordPart::Literal { text: "foo".to_string(), quoted: false },
        WordPart::Literal { text: "*".to_string(), quoted: true },
        WordPart::Literal { text: "bar".to_string(), quoted: false },
    ]);
    let fields = expand(&word, &mut shell);
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].chars, "foo*bar");
    assert_eq!(fields[0].quoted, vec![false, false, false, true, false, false, false]);
}

#[test]
fn expand_quoted_var_marks_chars_quoted() {
    let mut shell = Shell::new();
    shell.export_set("HUCK_Q", "val".to_string());
    let word = Word(vec![WordPart::Var { name: "HUCK_Q".to_string(), quoted: true }]);
    let fields = expand(&word, &mut shell);
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].quoted, vec![true, true, true]);
}

#[test]
fn expand_unquoted_var_marks_chars_unquoted() {
    let mut shell = Shell::new();
    shell.export_set("HUCK_Q", "val".to_string());
    let word = Word(vec![WordPart::Var { name: "HUCK_Q".to_string(), quoted: false }]);
    let fields = expand(&word, &mut shell);
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].quoted, vec![false, false, false]);
}

#[test]
fn expand_tilde_marks_chars_unquoted() {
    let mut shell = Shell::new();
    shell.export_set("HOME", "/h".to_string());
    let word = Word(vec![WordPart::Tilde(TildeSpec::Home)]);
    let fields = expand(&word, &mut shell);
    assert_eq!(fields[0].chars, "/h");
    assert_eq!(fields[0].quoted, vec![false, false]);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test expand_literal_quoted expand_mixed_quoted expand_quoted_var`
Expected: FAIL (all chars currently marked unquoted).

- [ ] **Step 3: Update expand to propagate quoting from each part**

In the `expand` body, replace the `WordPart::Literal { text, .. }` arm with `WordPart::Literal { text, quoted }` and use `current.push_str(&text, quoted)`.

For `WordPart::Var { name, quoted: true }`: look up the value; `current.push_str(&value, true)`. (No splitting on quoted vars.)

For `WordPart::LastStatus { quoted: true }`: same — `current.push_str(&status_str, true)`.

For `WordPart::CommandSub { sequence, quoted: true }`: capture output; `current.push_str(&output, true)`.

For unquoted Var/LastStatus/CommandSub: keep `emit_split_fields` (which always pushes with `quoted: false`).

For `WordPart::Tilde(spec)`: keep `current.push_str(&resolved, false)`.

- [ ] **Step 4: Run all tests**

Run: `cargo test`
Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/expand.rs
git commit -m "v10 task 5: propagate per-WordPart quoting into Field"
```

---

## Task 6: `glob_expand_fields` skeleton (no filesystem)

Add the function with the no-metachar fast path and the literal-fallback path. No glob crate calls yet; this isolates the metachar-detection logic.

**Files:**
- Modify: `src/expand.rs`

- [ ] **Step 1: Write failing tests for metachar detection**

Add to test module:

```rust
#[test]
fn glob_expand_no_metachar_returns_chars_as_string() {
    let f = Field::from_unquoted("plain.txt");
    let out = glob_expand_fields(vec![f]);
    assert_eq!(out, vec!["plain.txt".to_string()]);
}

#[test]
fn glob_expand_quoted_metachar_treated_as_literal() {
    // All chars quoted including the `*` → no globbing.
    let f = Field::from_quoted("*.txt");
    let out = glob_expand_fields(vec![f]);
    assert_eq!(out, vec!["*.txt".to_string()]);
}

#[test]
fn glob_expand_question_mark_metachar_detected() {
    let mut f = Field::from_unquoted("a");
    f.push_str("?", false);
    // Detection only — actual matching arrives in task 7. For now we
    // assert the no-match literal-fallback path returns the chars.
    let out = glob_expand_fields(vec![f]);
    // With no real files matching `a?` in CWD, expect literal fallback.
    // CWD here is the workspace root; "a?" is extremely unlikely to match.
    assert_eq!(out, vec!["a?".to_string()]);
}

#[test]
fn glob_expand_preserves_field_order() {
    let f1 = Field::from_unquoted("first");
    let f2 = Field::from_unquoted("second");
    let out = glob_expand_fields(vec![f1, f2]);
    assert_eq!(out, vec!["first".to_string(), "second".to_string()]);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test glob_expand_`
Expected: FAIL (function does not exist).

- [ ] **Step 3: Implement detection and the fast path**

Add to `src/expand.rs`:

```rust
pub fn glob_expand_fields(fields: Vec<Field>) -> Vec<String> {
    let mut out = Vec::new();
    for field in fields {
        if !has_unquoted_metachar(&field) {
            out.push(field.chars);
            continue;
        }
        // Glob path lands in Task 7. For now, literal fallback.
        out.push(field.chars);
    }
    out
}

fn has_unquoted_metachar(field: &Field) -> bool {
    field
        .chars
        .chars()
        .zip(field.quoted.iter())
        .any(|(c, &q)| !q && matches!(c, '*' | '?' | '['))
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test glob_expand_`
Expected: PASS (4 new tests).

- [ ] **Step 5: Commit**

```bash
git add src/expand.rs
git commit -m "v10 task 6: glob_expand_fields skeleton with metachar detection"
```

---

## Task 7: Wire `glob` crate matching

Build the glob pattern string (escaping quoted metachars), invoke `glob::glob_with`, collect matches, sort is implicit. Tests use a `TempDir` and set CWD.

**Files:**
- Modify: `src/expand.rs`

- [ ] **Step 1: Write failing tests using TempDir**

Add to test module:

```rust
use std::sync::Mutex;

// CWD is process-global; serialize tests that mutate it.
static CWD_LOCK: Mutex<()> = Mutex::new(());

fn touch(dir: &std::path::Path, name: &str) {
    std::fs::write(dir.join(name), b"").unwrap();
}

#[test]
fn glob_star_matches_files_in_cwd() {
    let _g = CWD_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    touch(tmp.path(), "a.txt");
    touch(tmp.path(), "b.txt");
    let saved = std::env::current_dir().unwrap();
    std::env::set_current_dir(tmp.path()).unwrap();

    let mut f = Field::from_unquoted("*");
    f.push_str(".txt", false);
    let out = glob_expand_fields(vec![f]);

    std::env::set_current_dir(saved).unwrap();

    assert_eq!(out, vec!["a.txt".to_string(), "b.txt".to_string()]);
}

#[test]
fn glob_star_excludes_dotfiles_by_default() {
    let _g = CWD_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    touch(tmp.path(), "visible");
    touch(tmp.path(), ".hidden");
    let saved = std::env::current_dir().unwrap();
    std::env::set_current_dir(tmp.path()).unwrap();

    let f = Field::from_unquoted("*");
    let out = glob_expand_fields(vec![f]);

    std::env::set_current_dir(saved).unwrap();

    assert_eq!(out, vec!["visible".to_string()]);
}

#[test]
fn glob_dot_star_matches_dotfiles_but_excludes_dot_and_dotdot() {
    let _g = CWD_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    touch(tmp.path(), ".hidden");
    let saved = std::env::current_dir().unwrap();
    std::env::set_current_dir(tmp.path()).unwrap();

    let mut f = Field::from_unquoted(".");
    f.push_str("*", false);
    let out = glob_expand_fields(vec![f]);

    std::env::set_current_dir(saved).unwrap();

    assert!(out.contains(&".hidden".to_string()));
    assert!(!out.contains(&".".to_string()));
    assert!(!out.contains(&"..".to_string()));
}

#[test]
fn glob_bracket_class_matches_listed_chars() {
    let _g = CWD_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    touch(tmp.path(), "a.txt");
    touch(tmp.path(), "b.txt");
    touch(tmp.path(), "c.txt");
    let saved = std::env::current_dir().unwrap();
    std::env::set_current_dir(tmp.path()).unwrap();

    let mut f = Field::from_unquoted("[ab]");
    f.push_str(".txt", false);
    let out = glob_expand_fields(vec![f]);

    std::env::set_current_dir(saved).unwrap();

    assert_eq!(out, vec!["a.txt".to_string(), "b.txt".to_string()]);
}

#[test]
fn glob_no_match_returns_literal_pattern() {
    let _g = CWD_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let saved = std::env::current_dir().unwrap();
    std::env::set_current_dir(tmp.path()).unwrap();

    let mut f = Field::from_unquoted("nonex");
    f.push_str("*", false);
    f.push_str(".xyz", false);
    let out = glob_expand_fields(vec![f]);

    std::env::set_current_dir(saved).unwrap();

    assert_eq!(out, vec!["nonex*.xyz".to_string()]);
}

#[test]
fn glob_partial_quoting_keeps_literal_prefix() {
    let _g = CWD_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    touch(tmp.path(), "fooA");
    touch(tmp.path(), "fooB");
    touch(tmp.path(), "barA");
    let saved = std::env::current_dir().unwrap();
    std::env::set_current_dir(tmp.path()).unwrap();

    // `"foo"*` — first three chars quoted, then unquoted `*`.
    let mut f = Field::from_quoted("foo");
    f.push_str("*", false);
    let out = glob_expand_fields(vec![f]);

    std::env::set_current_dir(saved).unwrap();

    assert_eq!(out, vec!["fooA".to_string(), "fooB".to_string()]);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test glob_star_ glob_dot_star_ glob_bracket_ glob_no_match_ glob_partial_quoting_`
Expected: FAIL — current implementation returns literal for unmatched patterns and the new tests have actual files to match.

- [ ] **Step 3: Replace skeleton with real glob invocation**

In `src/expand.rs`:

```rust
use glob::{glob_with, MatchOptions};

pub fn glob_expand_fields(fields: Vec<Field>) -> Vec<String> {
    let mut out = Vec::new();
    for field in fields {
        if !has_unquoted_metachar(&field) {
            out.push(field.chars);
            continue;
        }
        let pattern = build_glob_pattern(&field);
        let opts = MatchOptions {
            case_sensitive: true,
            require_literal_separator: true,
            require_literal_leading_dot: true,
        };
        match glob_with(&pattern, opts) {
            Ok(paths) => {
                let mut matched = Vec::new();
                for entry in paths {
                    let Ok(path) = entry else { continue };
                    match path.into_os_string().into_string() {
                        Ok(s) => matched.push(s),
                        Err(_) => eprintln!("huck: skipping non-UTF8 path"),
                    }
                }
                if matched.is_empty() {
                    out.push(field.chars);
                } else {
                    out.extend(matched);
                }
            }
            Err(_) => {
                // Invalid pattern → literal fallback.
                out.push(field.chars);
            }
        }
    }
    out
}

fn build_glob_pattern(field: &Field) -> String {
    let mut p = String::new();
    for (c, &q) in field.chars.chars().zip(field.quoted.iter()) {
        if q && matches!(c, '*' | '?' | '[' | ']') {
            // Escape via bracket expression — works for all four chars
            // since `glob` treats `[*]` etc. as a one-char literal class.
            p.push('[');
            p.push(c);
            p.push(']');
        } else {
            p.push(c);
        }
    }
    p
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test`
Expected: all tests pass.

If the dotfile test fails because the `glob` crate includes `.` and `..` in `.*` matches, add an explicit filter:

```rust
matched.retain(|p| {
    let last = std::path::Path::new(p).file_name().and_then(|s| s.to_str());
    !matches!(last, Some(".") | Some(".."))
});
```

- [ ] **Step 5: Commit**

```bash
git add src/expand.rs
git commit -m "v10 task 7: wire glob crate matching with bash-compatible defaults"
```

---

## Task 8: Edge cases — invalid patterns and negation

The `glob` crate's `Pattern::new` is forgiving for some inputs and strict for others. Tighten the literal-fallback path and add `[!...]` (POSIX negation) coverage.

**Files:**
- Modify: `src/expand.rs`

- [ ] **Step 1: Write failing tests**

Add to test module:

```rust
#[test]
fn glob_negation_bracket_excludes_listed() {
    let _g = CWD_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    touch(tmp.path(), "a.txt");
    touch(tmp.path(), "b.txt");
    touch(tmp.path(), "c.txt");
    let saved = std::env::current_dir().unwrap();
    std::env::set_current_dir(tmp.path()).unwrap();

    let mut f = Field::from_unquoted("[!a]");
    f.push_str(".txt", false);
    let out = glob_expand_fields(vec![f]);

    std::env::set_current_dir(saved).unwrap();

    assert_eq!(out, vec!["b.txt".to_string(), "c.txt".to_string()]);
}

#[test]
fn glob_unterminated_bracket_falls_back_to_literal() {
    let _g = CWD_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let saved = std::env::current_dir().unwrap();
    std::env::set_current_dir(tmp.path()).unwrap();

    let f = Field::from_unquoted("[abc"); // no closing ]
    let out = glob_expand_fields(vec![f]);

    std::env::set_current_dir(saved).unwrap();

    assert_eq!(out, vec!["[abc".to_string()]);
}

#[test]
fn glob_star_does_not_cross_path_separator() {
    let _g = CWD_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join("sub")).unwrap();
    touch(&tmp.path().join("sub"), "deep.txt");
    touch(tmp.path(), "top.txt");
    let saved = std::env::current_dir().unwrap();
    std::env::set_current_dir(tmp.path()).unwrap();

    let mut f = Field::from_unquoted("*");
    f.push_str(".txt", false);
    let out = glob_expand_fields(vec![f]);

    std::env::set_current_dir(saved).unwrap();

    assert_eq!(out, vec!["top.txt".to_string()]);
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test glob_negation_ glob_unterminated_ glob_star_does_not_cross_`
Expected: `negation` and `path_separator` likely PASS; `unterminated` depends on glob crate behavior. If `unterminated` already passes (because `glob` raises a `PatternError`), great. If it FAILS, examine the actual output and adjust the fallback path: catch the `Err` from `glob_with` (already done in Task 7) — verify the test is hitting that branch.

- [ ] **Step 3: Add invalid-pattern detection if needed**

If Task 7's `Err(_) => out.push(field.chars)` already covers the unterminated case, no code change is needed. If not, add an explicit `glob::Pattern::new(&pattern).is_err()` precheck before calling `glob_with`, with literal fallback on error.

- [ ] **Step 4: Run full test suite**

Run: `cargo test`
Expected: all tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/expand.rs
git commit -m "v10 task 8: edge cases — negation, invalid patterns, separator boundary"
```

---

## Task 9: Wire `glob_expand_fields` into executor argv build

Replace the `.chars` projections in `executor.rs` with calls to `glob_expand_fields`. Assignments still bypass globbing (handled by `expand_assignment` which returns `String` directly — confirm this path doesn't touch the new function).

**Files:**
- Modify: `src/executor.rs`

- [ ] **Step 1: Write a failing integration-style test**

Add to `src/executor.rs` `#[cfg(test)]` if a suitable test scaffold exists, otherwise defer real end-to-end coverage to Task 10. As a unit-level sanity check, add to `src/expand.rs` test module:

```rust
#[test]
fn expand_then_glob_end_to_end_for_literal() {
    let mut shell = Shell::new();
    let word = Word(vec![WordPart::Literal { text: "hello".to_string(), quoted: false }]);
    let argv = glob_expand_fields(expand(&word, &mut shell));
    assert_eq!(argv, vec!["hello".to_string()]);
}
```

Run: `cargo test expand_then_glob_end_to_end_for_literal`
Expected: PASS (sanity check before wiring executor).

- [ ] **Step 2: Update executor call sites**

In `src/executor.rs`, find the three `expand(...)` call sites. Replace:

```rust
let fields: Vec<String> = expand(word, shell).into_iter().map(|f| f.chars).collect();
```

with:

```rust
let fields = glob_expand_fields(expand(word, shell));
```

For the program-name site (line 307):

```rust
let prog_fields = glob_expand_fields(expand(&cmd.program, shell));
```

For the args extension site (line 316):

```rust
args.extend(glob_expand_fields(expand(word, shell)));
```

Import: add `use crate::expand::{expand, glob_expand_fields};` at the top of `executor.rs` if not already imported.

- [ ] **Step 3: Run full test suite**

Run: `cargo test`
Expected: all tests pass.

- [ ] **Step 4: Manual smoke test**

```bash
cargo build --release
cd /tmp && mkdir huck-smoke && cd huck-smoke
touch a.txt b.txt c.rs
~/projects/shuck/target/release/huck
# In the REPL:
echo *.txt       # expect: a.txt b.txt
echo "*.txt"     # expect: *.txt
echo *.nope      # expect: *.nope
echo [ab].txt    # expect: a.txt b.txt
exit
```

(Document any deviation from expected; fix before commit.)

- [ ] **Step 5: Commit**

```bash
git add src/expand.rs src/executor.rs
git commit -m "v10 task 9: wire glob_expand_fields into executor argv build"
```

---

## Task 10: Integration tests via shell binary

End-to-end tests spawn the built binary, feed stdin, assert stdout. Mirrors the smoke test from Task 9.

**Files:**
- Create: `tests/glob_integration.rs`

- [ ] **Step 1: Scaffold the test file**

Create `tests/glob_integration.rs`:

```rust
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run_in_cwd(cwd: &std::path::Path, script: &str) -> String {
    let mut child = Command::new(huck_binary())
        .current_dir(cwd)
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
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn touch(dir: &std::path::Path, name: &str) {
    std::fs::write(dir.join(name), b"").unwrap();
}

#[test]
fn echo_star_matches_cwd_files_sorted() {
    let tmp = tempfile::tempdir().unwrap();
    touch(tmp.path(), "a.txt");
    touch(tmp.path(), "b.txt");
    let out = run_in_cwd(tmp.path(), "echo *.txt\nexit\n");
    assert!(out.lines().any(|l| l == "a.txt b.txt"), "stdout: {out}");
}

#[test]
fn echo_quoted_star_is_literal() {
    let tmp = tempfile::tempdir().unwrap();
    touch(tmp.path(), "a.txt");
    let out = run_in_cwd(tmp.path(), "echo \"*.txt\"\nexit\n");
    assert!(out.lines().any(|l| l == "*.txt"), "stdout: {out}");
}

#[test]
fn echo_no_match_passes_pattern_literally() {
    let tmp = tempfile::tempdir().unwrap();
    let out = run_in_cwd(tmp.path(), "echo *.nope\nexit\n");
    assert!(out.lines().any(|l| l == "*.nope"), "stdout: {out}");
}

#[test]
fn echo_bracket_class() {
    let tmp = tempfile::tempdir().unwrap();
    touch(tmp.path(), "a.txt");
    touch(tmp.path(), "b.txt");
    touch(tmp.path(), "c.txt");
    let out = run_in_cwd(tmp.path(), "echo [ab].txt\nexit\n");
    assert!(out.lines().any(|l| l == "a.txt b.txt"), "stdout: {out}");
}

#[test]
fn echo_tilde_glob_combo() {
    let tmp = tempfile::tempdir().unwrap();
    touch(tmp.path(), "x.dat");
    touch(tmp.path(), "y.dat");
    // Set HOME via env on the child so ~/*.dat expands to the temp dir.
    let mut child = Command::new(huck_binary())
        .env("HOME", tmp.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"echo ~/*.dat\nexit\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let s = String::from_utf8_lossy(&out.stdout);
    let expected_a = format!("{}/x.dat", tmp.path().display());
    let expected_b = format!("{}/y.dat", tmp.path().display());
    let expected = format!("{expected_a} {expected_b}");
    assert!(s.lines().any(|l| l == expected), "stdout: {s}");
}
```

- [ ] **Step 2: Run the integration tests**

Run: `cargo test --test glob_integration`
Expected: all 5 tests pass.

- [ ] **Step 3: Run the full suite to confirm nothing regressed**

Run: `cargo test`
Expected: all tests pass.

- [ ] **Step 4: Commit**

```bash
git add tests/glob_integration.rs
git commit -m "v10 task 10: end-to-end glob integration tests via shell binary"
```

---

## Task 11: README update

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Add v10 row to the status table**

Find the status table and append:

```
| v10       | Pathname expansion (`*`, `?`, `[abc]`)                  |
```

- [ ] **Step 2: Add a Pathname expansion subsection in Features**

After the **Tilde expansion (v9):** block, add:

```markdown
**Pathname expansion (v10):**
`*` matches any run of characters, `?` matches one character, `[abc]`
and `[a-z]` match a single character from a class (`[!abc]` negates).
Metacharacters do not cross `/` and do not match a leading `.` (use
`.*` for dotfiles). Quoted metacharacters (`"*"`, `'*'`) stay literal.
A pattern with no matches is passed through unchanged (bash default).
Redirect targets do not yet glob-expand.
```

- [ ] **Step 3: Update the Not-yet-implemented section**

Remove the `pathname expansion (...) — coming in v10` bullet; renumber `v11` references if any.

- [ ] **Step 4: Update test count in Build and run**

Run `cargo test 2>&1 | tail -5` to get the new total, then update the comment `# full test suite (NNN tests)`.

- [ ] **Step 5: Commit**

```bash
git add README.md
git commit -m "v10 task 11: README — add v10 row and pathname expansion section"
```

---

## Final review checkpoint

After Task 11:

- [ ] Run full suite one more time: `cargo test`
- [ ] Run `cargo llvm-cov --summary-only` and confirm coverage is still >85%
- [ ] Run `cargo clippy -- -D warnings`
- [ ] Manual REPL smoke session covering: `*`, `?`, `[abc]`, `[!abc]`, no-match, `"*"`, `~/*.txt`, `*/foo` (sub-dir, expect literal fallback since `*` doesn't cross `/` and there's no specific `*/foo` file unless one happens to exist).
- [ ] Final-review the whole branch as a single diff before merging to `main`.
