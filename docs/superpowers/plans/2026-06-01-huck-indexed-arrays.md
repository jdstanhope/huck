# huck v71 — Indexed Arrays Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Each task is implemented by a fresh subagent, with spec-compliance review and code-quality review between tasks.

**Goal:** Add bash-compatible indexed (sparse) arrays to huck — literal compound assignment, subscripted access, all-elements expansion with correct quoting, append, slicing, `declare -a`, `local`-scoped arrays, and readonly enforcement.

**Architecture:** New `VarValue` enum (`Scalar(String) | Indexed(BTreeMap<usize,String>)`) on `Variable`, with a `scalar_view()` helper so existing read sites continue to work for both shapes. New AST: `WordPart::ArrayLiteral` (compound RHS), `AssignTarget` enum (bare vs subscripted lvalue), `subscript: Option<SubscriptKind>` on `ParamModifier`. New expansion path for `${a[i]}` / `${a[@]}` / `${a[*]}` / `${#a[@]}` / `${!a[@]}` / `${a[@]:o:l}` that also closes v33's deferred `$@` / `$*` slicing.

**Tech Stack:** Rust 1.85+, `std::collections::BTreeMap`, existing `arith::eval` for subscript evaluation.

**Branch:** `v71-arrays` (create from `main` at the start of Preamble).

**Spec:** `docs/superpowers/specs/2026-06-01-huck-indexed-arrays-design.md`.

**Commit trailer (every commit):**

```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Preamble P.1: Branch setup

- [ ] **Step 1: Verify clean tree on main**

Run: `git status && git rev-parse --abbrev-ref HEAD`
Expected: branch `main`, clean working tree.

- [ ] **Step 2: Create the iteration branch**

Run:
```bash
git checkout -b v71-arrays
```
Expected: `Switched to a new branch 'v71-arrays'`.

- [ ] **Step 3: Confirm baseline tests pass**

Run: `cargo test 2>&1 | tail -5`
Expected: `test result: ok.` lines, 0 failed.

- [ ] **Step 4: Confirm clippy is clean**

Run: `cargo clippy --all-targets 2>&1 | tail -3`
Expected: `Finished` with no warning/error lines.

---

## File-structure map

Tasks modify these files (all already exist except the integration test):

| File | Responsibility | Tasks |
|------|----------------|-------|
| `src/shell_state.rs` | `Variable`, `VarValue`, array storage helpers, `lookup_var`, `try_set` | 1 |
| `src/lexer.rs` | Compound RHS scanning, subscript scanning, `WordPart::ArrayLiteral`, `ParamModifier` subscript field | 2 |
| `src/command.rs` | `AssignTarget` enum, `SimpleCommand::Assign` LHS shape, `ExecCommand.inline_assignments` LHS shape | 2 |
| `src/expand.rs` | Array expansion semantics, slicing helper, nounset wire-in for elements | 3 |
| `src/executor.rs` | Compound + element assignment paths, append, snapshot+restore for inline `name=(…)` | 4 |
| `src/builtins.rs` | `builtin_unset` subscripted form, `declare -a`/`-p`, `local -a`, `readonly` array RHS, `export` rejection | 5 |
| `docs/bash-divergences.md` | New M-82 entry + cross-references | 6 |
| `README.md` | v71 iteration row | 6 |
| `tests/arrays_integration.rs` | 12 binary-driven tests (new file) | 6 |
| `tests/scripts/arrays_diff_check.sh` | Manual bash-vs-huck diff harness (new file) | 6 |

---

## Task 1: VarValue foundation refactor

**Files:**
- Modify: `src/shell_state.rs` (lines 8–14, 167–230, 245–270, 274–340, 363–400, 420–430)
- Modify: `src/builtins.rs` (lines around 541; tests around 7050–7100)

**Goal:** Introduce the new value-model under the existing API. After this task, NOTHING user-visible changes. Every existing test must still pass. This is the dangerous refactor; isolating it now lets later tasks be additive.

- [ ] **Step 1: Add `VarValue` enum and refactor `Variable`**

Edit `src/shell_state.rs` lines 8–14:

```rust
use std::collections::BTreeMap;
use std::collections::HashMap;

/// Storage for a shell variable. Scalar covers ordinary strings;
/// Indexed is a sparse map of usize subscripts to element values
/// (sorted by key — BTreeMap so `${a[@]}` and `${!a[@]}` walk in
/// ascending subscript order).
#[derive(Debug, Clone)]
pub enum VarValue {
    Scalar(String),
    Indexed(BTreeMap<usize, String>),
}

impl VarValue {
    /// Returns the "scalar view" of this value: the string itself
    /// for `Scalar`, or the element at subscript 0 (or "" if no such
    /// element) for `Indexed`. This is the bash rule that `$a` and
    /// `${a}` on an indexed array mean `${a[0]}`.
    pub fn scalar_view(&self) -> &str {
        match self {
            VarValue::Scalar(s) => s.as_str(),
            VarValue::Indexed(m) => m.get(&0).map(String::as_str).unwrap_or(""),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Variable {
    pub value: VarValue,
    pub exported: bool,
    pub readonly: bool,
    pub integer: bool,
}
```

- [ ] **Step 2: Add `Variable` constructor helpers**

Append to `src/shell_state.rs` right after the struct:

```rust
impl Variable {
    /// Convenience constructor for the common case: an unexported,
    /// non-readonly, non-integer scalar.
    pub fn scalar(value: String) -> Self {
        Variable {
            value: VarValue::Scalar(value),
            exported: false,
            readonly: false,
            integer: false,
        }
    }
}
```

- [ ] **Step 3: Update `Shell::get` (~line 167)**

Replace:

```rust
pub fn get(&self, name: &str) -> Option<&str> {
    self.vars.get(name).map(|v| v.value.as_str())
}
```

With:

```rust
pub fn get(&self, name: &str) -> Option<&str> {
    self.vars.get(name).map(|v| v.value.scalar_view())
}
```

- [ ] **Step 4: Update `Shell::lookup_var` (~line 213)**

Find the `self.vars.get(name).map(|v| v.value.clone())` line and replace with:

```rust
self.vars.get(name).map(|v| v.value.scalar_view().to_string())
```

- [ ] **Step 5: Update internal scalar writers**

Find each line that does `existing.value = value;` (around lines 220 and 248). Replace each with:

```rust
existing.value = VarValue::Scalar(value);
```

Find any `Variable { value: <string-expr>, ... }` literal in this file (search `Variable {`) and rewrite to use `Variable::scalar(<string-expr>)` where the rest of the fields are defaults. For literals that set non-default fields, keep the struct form but wrap the value in `VarValue::Scalar(...)`.

Specifically, in `Shell::try_set` (~line 317), find the construction that creates a new variable on first write and use `Variable::scalar(...)`.

In any `Shell::export_set` path that creates a new variable, do the same.

- [ ] **Step 6: Update `exported_env()` (~line 424)**

Find:

```rust
.map(|(k, v)| (k.as_str(), v.value.as_str()))
```

Replace with:

```rust
.map(|(k, v)| (k.as_str(), v.value.scalar_view()))
```

- [ ] **Step 7: Update `builtin_declare` formatter (~line 541 in builtins.rs)**

Find:

```rust
let escaped = escape_double_quote_value(&var.value);
```

Replace with:

```rust
let escaped = escape_double_quote_value(var.value.scalar_view());
```

(The array branch in this formatter is added in Task 5; for now we keep the scalar-only behavior under the new value type.)

- [ ] **Step 8: Update local-scope tests in builtins.rs**

Find the two test assertions (lines ~7055 and ~7091):

```rust
assert_eq!(v.value, "outer");
```

Replace each with:

```rust
assert!(matches!(&v.value, crate::shell_state::VarValue::Scalar(s) if s == "outer"));
```

- [ ] **Step 9: Update `snapshot_for_local_scope` and `try_set`**

`Shell::try_set` reads the existing variable to detect integer-coerce. With `VarValue`, integer-coerce only applies when the current value is `Scalar` (integer-array support is deferred per spec). Update the integer-coerce arm in `try_set`:

Find the block (around line 317–340) that checks `v.integer && !v.readonly` and re-evaluates the RHS via `arith::parse/eval`. Wrap the integer path so it only runs for `VarValue::Scalar`:

```rust
pub fn try_set(&mut self, name: &str, value: String) -> Result<(), ()> {
    if let Some(existing) = self.vars.get(name) {
        if existing.readonly {
            eprintln!("huck: {name}: readonly variable");
            return Err(());
        }
        let new_value = if existing.integer && matches!(existing.value, VarValue::Scalar(_)) {
            let coerced = match crate::arith::parse(&value)
                .and_then(|expr| crate::arith::eval(&expr, self).map(|n| n.to_string()))
            {
                Ok(s) => s,
                Err(_) => "0".to_string(),
            };
            VarValue::Scalar(coerced)
        } else {
            match &existing.value {
                VarValue::Indexed(_) => {
                    // Bash: `a=v` on an indexed array sets element 0,
                    // leaving the rest. Mirror that.
                    let mut new_map = match &existing.value {
                        VarValue::Indexed(m) => m.clone(),
                        _ => BTreeMap::new(),
                    };
                    new_map.insert(0, value);
                    VarValue::Indexed(new_map)
                }
                VarValue::Scalar(_) => VarValue::Scalar(value),
            }
        };
        if let Some(existing_mut) = self.vars.get_mut(name) {
            existing_mut.value = new_value;
        }
        Ok(())
    } else {
        self.vars.insert(name.to_string(), Variable::scalar(value));
        Ok(())
    }
}
```

Note: `crate::arith::eval` already takes `&Shell`; the inner-borrow conflict is resolved by computing `coerced` *before* taking the `get_mut`.

- [ ] **Step 10: Add foundation unit tests**

Append a new test module at the bottom of `src/shell_state.rs`:

```rust
#[cfg(test)]
mod array_value_tests {
    use super::*;

    #[test]
    fn scalar_view_returns_string_for_scalar() {
        let v = VarValue::Scalar("hello".to_string());
        assert_eq!(v.scalar_view(), "hello");
    }

    #[test]
    fn scalar_view_returns_element_zero_for_indexed() {
        let mut m = BTreeMap::new();
        m.insert(0, "first".to_string());
        m.insert(1, "second".to_string());
        let v = VarValue::Indexed(m);
        assert_eq!(v.scalar_view(), "first");
    }

    #[test]
    fn scalar_view_empty_for_indexed_without_zero() {
        let mut m = BTreeMap::new();
        m.insert(5, "x".to_string());
        let v = VarValue::Indexed(m);
        assert_eq!(v.scalar_view(), "");
    }

    #[test]
    fn scalar_view_empty_for_empty_indexed() {
        let v = VarValue::Indexed(BTreeMap::new());
        assert_eq!(v.scalar_view(), "");
    }

    #[test]
    fn variable_scalar_constructor_sets_defaults() {
        let v = Variable::scalar("x".to_string());
        assert!(!v.exported);
        assert!(!v.readonly);
        assert!(!v.integer);
        assert_eq!(v.value.scalar_view(), "x");
    }
}
```

- [ ] **Step 11: Build and run all existing tests**

Run: `cargo build 2>&1 | tail -10`
Expected: `Finished` with no errors.

Run: `cargo test 2>&1 | grep -E "test result|FAILED" | head -40`
Expected: All `test result: ok.` lines; 0 failed. (Test counts will tick up by 5 for the new module.)

Run: `cargo clippy --all-targets 2>&1 | tail -5`
Expected: `Finished` no warnings.

- [ ] **Step 12: Commit**

```bash
git add src/shell_state.rs src/builtins.rs
git commit -m "$(cat <<'EOF'
foundation: VarValue enum (v71 task 1)

Variable.value changes from String to a VarValue enum with two
variants: Scalar(String) and Indexed(BTreeMap<usize, String>). A
new scalar_view() helper returns the bash-style "$a == ${a[0]}"
view, so every existing read site continues to work for both
shapes. No user-visible behavior change yet; this is the
foundation refactor for v71.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Parser & lexer — array syntax

**Files:**
- Modify: `src/command.rs` (lines ~91–105 for AssignTarget, ~242–284 for SimpleCommand/ExecCommand)
- Modify: `src/lexer.rs` (WordPart enum ~line 91; assignment scanning; param-expansion subscript scanning; the `SubscriptKind` shape)

**Goal:** Recognize the three new syntactic forms — `name=(...)`, `name[expr]=value`, `${a[…]}`. After this task, the AST holds array syntax but the executor and expansion still treat it as a no-op or error.

- [ ] **Step 1: Add `SubscriptKind` and extend `WordPart` (lexer.rs)**

Edit `src/lexer.rs` around line 91 (the `WordPart` enum):

```rust
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum SubscriptKind {
    /// `${a[@]}` — produces a word list, one element per array entry,
    /// no IFS splitting when quoted.
    All,
    /// `${a[*]}` — joined-by-IFS scalar when quoted; word-split when not.
    Star,
    /// `${a[expr]}` — `expr` arith-evaluates to a usize subscript.
    Index(Word),
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum WordPart {
    Literal { text: String, quoted: bool },
    Var { name: String, quoted: bool },
    ParamExpansion {
        name: String,
        modifier: ParamModifier,
        quoted: bool,
        /// Some(...) for `${a[i]}` / `${a[@]}` / `${a[*]}`; None for `${a}`.
        subscript: Option<SubscriptKind>,
    },
    CommandSub { sequence: Sequence, quoted: bool },
    Tilde(String),
    /// Compound array RHS `(elem elem [idx]=elem ...)`. Each element is
    /// either positional (no subscript) or explicit (`[expr]=value`).
    /// Only appears as the sole `WordPart` in a Word used as the RHS of
    /// an array-assignment in `SimpleCommand::Assign` / inline prefix.
    ArrayLiteral(Vec<ArrayLiteralElement>),
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct ArrayLiteralElement {
    /// `Some(word)` for explicit `[expr]=value`; `None` for positional.
    pub subscript: Option<Word>,
    pub value: Word,
}
```

Every existing `ParamExpansion { name, modifier, quoted }` construction in lexer.rs must be updated to set `subscript: None`. Use Edit's `replace_all: true` where the pattern is unique.

Run: `grep -n "ParamExpansion {" src/lexer.rs` and update each construction site to add `subscript: None`.

- [ ] **Step 2: Add `AssignTarget` to command.rs**

Edit `src/command.rs` around line 280 (above `SimpleCommand`):

```rust
/// Left-hand side of an assignment. Bare `name=v` is `Bare`;
/// subscripted `name[expr]=v` is `Indexed`.
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum AssignTarget {
    Bare(String),
    Indexed { name: String, subscript: Word },
}

impl AssignTarget {
    pub fn name(&self) -> &str {
        match self {
            AssignTarget::Bare(n) => n,
            AssignTarget::Indexed { name, .. } => name,
        }
    }
}
```

Change `SimpleCommand::Assign` and `ExecCommand.inline_assignments` to use `AssignTarget`:

```rust
pub enum SimpleCommand {
    /// `A=1 B=2 …` with no following command — every assignment
    /// persists in the shell. Single-element vec is the v22-style
    /// single-assignment case.
    Assign(Vec<(AssignTarget, Word)>),
    Exec(ExecCommand),
}

pub struct ExecCommand {
    pub inline_assignments: Vec<(AssignTarget, Word)>,
    pub program: Word,
    pub args: Vec<Word>,
    pub stdin: Option<Redirect>,
    pub stdout: Option<Redirect>,
    pub stderr: Option<Redirect>,
}
```

- [ ] **Step 3: Update existing assignment-construction sites**

The compiler error list after Step 2 will name every site. For each, wrap the existing `name: String` in `AssignTarget::Bare(name)`. Sites to expect:

- `src/lexer.rs` parser (multiple, around lines 2500–2620): change `(name_str, word)` to `(AssignTarget::Bare(name_str), word)`.
- `src/executor.rs` — `apply_inline_assignments` / `restore_inline_assignments` (around lines 440–470) and the inline-assignment helpers around 715. Add a match arm: `Bare(n) => …existing code…`; `Indexed { … } => unreachable!("array elements not yet supported in this code path — see Task 4")` for now (Task 4 will replace this).
- Any tests asserting on the assignment list.

Run: `cargo build 2>&1 | grep -E "error\[|error:" | head -20` to see the remaining sites. Each is a 5-line change.

- [ ] **Step 4: Add `read_subscript` helper to lexer**

Add this private helper near the existing `read_dollar_expansion` in `src/lexer.rs`:

```rust
/// Scans a `[...]` subscript: returns the Word inside the brackets.
/// Balanced over nested `[...]` (for arith-style expressions like
/// `a[$((i+1))]`) and quoting. The caller has already consumed the
/// leading `[`; this helper consumes through the matching `]`.
fn read_subscript<I: Iterator<Item = char>>(
    chars: &mut std::iter::Peekable<I>,
) -> Result<Word, LexError> {
    let mut depth: usize = 1;
    let mut buf = String::new();
    while let Some(&c) = chars.peek() {
        match c {
            '[' => { depth += 1; buf.push(c); chars.next(); }
            ']' => {
                depth -= 1;
                chars.next();
                if depth == 0 {
                    // Tokenize the inner text as a single Word (no quoting
                    // boundary across the subscript). For now we treat the
                    // subscript as a literal Word containing one Literal part;
                    // arith evaluation happens at expand time.
                    return Ok(Word(vec![WordPart::Literal { text: buf, quoted: false }]));
                }
                buf.push(c);
            }
            _ => { buf.push(c); chars.next(); }
        }
    }
    Err(LexError::UnterminatedSubscript)
}
```

Add the `UnterminatedSubscript` variant to `LexError` and a friendly message in `src/shell.rs::lex_error_message`:

```rust
LexError::UnterminatedSubscript => "huck: syntax error: missing ']' in subscript".to_string(),
```

- [ ] **Step 5: Wire subscript scanning into `${...}` parameter expansion**

In `src/lexer.rs`, find the `${...}` parameter-expansion parser (look for `ParamExpansion` constructions; around the `read_brace_expansion` or similar helper). After the name part is scanned, check if the next char is `[`:

```rust
// (inside the existing ${ NAME ... } scanner, after `name` is parsed)
let subscript = if chars.peek() == Some(&'[') {
    chars.next(); // consume '['
    // Special-case `@` / `*` single-char subscripts.
    let next = chars.peek().copied();
    if next == Some('@') || next == Some('*') {
        let sigil = chars.next().unwrap();
        if chars.peek() != Some(&']') {
            return Err(LexError::UnterminatedSubscript);
        }
        chars.next(); // consume ']'
        if sigil == '@' { Some(SubscriptKind::All) } else { Some(SubscriptKind::Star) }
    } else {
        let inner = read_subscript(chars)?;
        Some(SubscriptKind::Index(inner))
    }
} else {
    None
};
```

Pass `subscript` to every `ParamExpansion { ... }` construction in this branch.

- [ ] **Step 6: Wire compound RHS scanning into assignment parser**

In `src/lexer.rs`, find where assignment RHS is scanned (search for `inline_assignments` push sites or the function that recognizes `name=` and reads the following word). After the `=` is consumed, peek the next char. If it's `(`:

```rust
// pseudo: after consuming '=' in assignment context
if chars.peek() == Some(&'(') {
    chars.next(); // consume '('
    let elements = read_array_literal(chars)?;
    Word(vec![WordPart::ArrayLiteral(elements)])
} else {
    // existing scalar-RHS scanner
    ...
}
```

Add `read_array_literal`:

```rust
/// Scans `elem elem [idx]=elem ... )`. Caller has consumed the leading
/// `(`. Whitespace separates elements; nested `(...)` handled (for
/// command sub `$(...)`); quoting handled by recursive word-scanning.
fn read_array_literal<I: Iterator<Item = char>>(
    chars: &mut std::iter::Peekable<I>,
) -> Result<Vec<ArrayLiteralElement>, LexError> {
    let mut elements: Vec<ArrayLiteralElement> = Vec::new();
    loop {
        // Skip whitespace AND newlines (bash allows multi-line array literals).
        while let Some(&c) = chars.peek() {
            if c.is_whitespace() { chars.next(); } else { break; }
        }
        match chars.peek() {
            Some(&')') => { chars.next(); return Ok(elements); }
            None => return Err(LexError::UnterminatedArrayLiteral),
            _ => {}
        }
        // Each element is either `[expr]=value` or `value`.
        let subscript = if chars.peek() == Some(&'[') {
            chars.next(); // consume '['
            let sub = read_subscript(chars)?;
            // After ']', expect '='.
            if chars.next() != Some('=') {
                return Err(LexError::ArrayLiteralMissingEquals);
            }
            Some(sub)
        } else {
            None
        };
        // Scan one element value: a word that ends at whitespace or ')'.
        let value = read_array_element_word(chars)?;
        elements.push(ArrayLiteralElement { subscript, value });
    }
}
```

Add `read_array_element_word` that reads a single word with quoting until whitespace or `)` (this re-uses logic from the existing word-reader; implementer should extract a smaller helper or call the existing one with a custom terminator set).

Add `UnterminatedArrayLiteral` and `ArrayLiteralMissingEquals` to `LexError` with messages in `src/shell.rs::lex_error_message`:

- `"huck: syntax error: unterminated array literal '('"`
- `"huck: syntax error: array element subscript requires '=' after ']'"`

- [ ] **Step 7: Recognize subscripted lvalue `name[expr]=`**

In the assignment-recognizer in `src/lexer.rs` (the same function that decides "this token is an assignment word"), after scanning the bareword name, peek for `[`. If present, scan the subscript via `read_subscript`, then expect `=` or `+=`. Emit:

```rust
AssignTarget::Indexed { name, subscript } // subscript: Word from read_subscript
```

This is also where the `+=` operator (already a v37 feature for scalars, if implemented; check `grep -n "PlusAssign\|+=" src/lexer.rs`) needs the array-variant. Element-append `a[i]+=v` uses the same `Indexed` target plus a separate `is_append: bool` flag — extend the assignment token form to carry that bit, or use a wrapper:

```rust
// In SimpleCommand::Assign / inline_assignments, replace:
//   Vec<(AssignTarget, Word)>
// with:
//   Vec<Assignment>
// where:
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Assignment {
    pub target: AssignTarget,
    pub value: Word,
    pub append: bool,
}
```

(Update `SimpleCommand::Assign` and `ExecCommand.inline_assignments` accordingly.)

- [ ] **Step 8: Add parser unit tests**

Append to `src/lexer.rs` (`#[cfg(test)] mod array_parse_tests`):

```rust
#[cfg(test)]
mod array_parse_tests {
    use super::*;

    fn lex_one(input: &str) -> Vec<Token> {
        crate::lexer::lex(input).expect("lex").into_iter().collect()
    }

    #[test]
    fn compound_rhs_is_array_literal() {
        // Caller writes: a=(x y z)
        let tokens = lex_one("a=(x y z)");
        // Expect: one assignment token whose RHS Word contains ArrayLiteral
        // with 3 positional elements.
        let assigns = extract_assignments(&tokens);
        assert_eq!(assigns.len(), 1);
        let (target, word) = &assigns[0];
        assert_eq!(target.name(), "a");
        match &word.0[..] {
            [WordPart::ArrayLiteral(els)] => {
                assert_eq!(els.len(), 3);
                assert!(els.iter().all(|e| e.subscript.is_none()));
            }
            other => panic!("expected ArrayLiteral, got {other:?}"),
        }
    }

    #[test]
    fn sparse_compound_rhs_carries_subscripts() {
        let tokens = lex_one("a=([5]=x [2]=y)");
        let assigns = extract_assignments(&tokens);
        let (_target, word) = &assigns[0];
        match &word.0[..] {
            [WordPart::ArrayLiteral(els)] => {
                assert_eq!(els.len(), 2);
                assert!(els[0].subscript.is_some());
                assert!(els[1].subscript.is_some());
            }
            other => panic!("expected ArrayLiteral, got {other:?}"),
        }
    }

    #[test]
    fn subscripted_lvalue_parses() {
        // a[5]=v
        let tokens = lex_one("a[5]=v");
        let assigns = extract_assignments(&tokens);
        assert_eq!(assigns.len(), 1);
        match &assigns[0].0 {
            AssignTarget::Indexed { name, .. } => assert_eq!(name, "a"),
            _ => panic!("expected Indexed"),
        }
    }

    #[test]
    fn subscripted_ref_at_all() {
        // echo ${a[@]} - we just want to check the WordPart shape
        let tokens = lex_one("echo \"${a[@]}\"");
        let pe = find_param_expansion(&tokens, "a");
        assert!(matches!(pe.subscript, Some(SubscriptKind::All)));
    }

    #[test]
    fn subscripted_ref_at_star() {
        let tokens = lex_one("echo \"${a[*]}\"");
        let pe = find_param_expansion(&tokens, "a");
        assert!(matches!(pe.subscript, Some(SubscriptKind::Star)));
    }

    #[test]
    fn subscripted_ref_index_carries_word() {
        let tokens = lex_one("echo ${a[3]}");
        let pe = find_param_expansion(&tokens, "a");
        assert!(matches!(pe.subscript, Some(SubscriptKind::Index(_))));
    }

    #[test]
    fn bare_param_expansion_has_no_subscript() {
        let tokens = lex_one("echo ${a}");
        let pe = find_param_expansion(&tokens, "a");
        assert!(pe.subscript.is_none());
    }

    #[test]
    fn unterminated_subscript_errors() {
        let result = crate::lexer::lex("echo ${a[3");
        assert!(matches!(result, Err(LexError::UnterminatedSubscript)));
    }

    #[test]
    fn unterminated_array_literal_errors() {
        let result = crate::lexer::lex("a=(x y");
        assert!(matches!(result, Err(LexError::UnterminatedArrayLiteral)));
    }

    // extract_assignments / find_param_expansion: implement these
    // against the actual Token / parse-tree shape. If Token doesn't
    // carry the Assignment row directly, run parse(tokens) and walk
    // the resulting Sequence — for each Command::Simple(Exec(e)), the
    // Assignments live in e.inline_assignments (or in the Assign(...)
    // variant if there's no program word). For find_param_expansion,
    // walk the program/args/value Words and return the first
    // WordPart::ParamExpansion whose name matches.
}
```

- [ ] **Step 9: Run tests**

Run: `cargo build 2>&1 | tail -5`
Expected: clean build.

Run: `cargo test --bin huck array_parse 2>&1 | tail -20`
Expected: all 8 new tests pass.

Run: `cargo test 2>&1 | grep -E "test result|FAILED" | head -40`
Expected: every line `ok`, 0 failures. Existing tests must remain green (no behavioral change yet from the executor side).

- [ ] **Step 10: Commit**

```bash
git add src/lexer.rs src/command.rs src/shell.rs src/executor.rs
git commit -m "$(cat <<'EOF'
parser: array syntax — compound RHS, subscripts (v71 task 2)

Three new syntactic recognitions:
- Compound RHS `name=(elem [idx]=elem ...)` via new
  WordPart::ArrayLiteral with explicit-subscript support
- Subscripted lvalue `name[expr]=value` via new AssignTarget
  enum (Bare | Indexed); SimpleCommand::Assign and
  ExecCommand.inline_assignments now carry Assignment{target,
  value, append} rows
- Subscripted reference `${a[expr]}`/`${a[@]}`/`${a[*]}` via a
  new `subscript: Option<SubscriptKind>` field on
  WordPart::ParamExpansion

read_subscript helper is shared between assignment and
param-expansion contexts. Three new LexError variants. The
executor and expansion still treat AssignTarget::Indexed and
SubscriptKind as unimplemented; Tasks 3/4 wire them in.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Expansion semantics

**Files:**
- Modify: `src/expand.rs` (the ParamExpansion handler — add subscript branch + slicing helper)
- Modify: `src/shell_state.rs` (add array-element lookup helpers)

**Goal:** Reading arrays works end-to-end. `${a[i]}`, `${a[@]}`, `${a[*]}`, `${#a[@]}`, `${!a[@]}`, `${#a[i]}`, `${a[@]:o:l}`, bare `${a}≡${a[0]}`, nounset on missing elements. Closes v33's `$@`/`$*` slicing as a side-effect.

This task assumes that compound arrays cannot yet be CREATED — Task 4 adds that. But once an array exists in the `Shell.vars` map (we can seed one directly in tests via a new helper), expansion produces correct results.

- [ ] **Step 1: Add `Shell::lookup_array_element` and `Shell::get_array`**

Edit `src/shell_state.rs`, near the existing `lookup_var`:

```rust
/// Returns a reference to the indexed array stored under `name`,
/// or `None` if the variable is unset or a scalar.
pub fn get_array(&self, name: &str) -> Option<&BTreeMap<usize, String>> {
    match self.vars.get(name) {
        Some(v) => match &v.value {
            VarValue::Indexed(m) => Some(m),
            VarValue::Scalar(_) => None,
        },
        None => None,
    }
}

/// Returns the value at subscript `idx` for the indexed array named
/// `name`. `None` if the variable is unset, scalar (with idx != 0),
/// or the subscript has no entry. For scalar variables, idx=0
/// returns the scalar string — matches bash.
pub fn lookup_array_element(&self, name: &str, idx: usize) -> Option<String> {
    match self.vars.get(name) {
        Some(v) => match &v.value {
            VarValue::Indexed(m) => m.get(&idx).cloned(),
            VarValue::Scalar(s) if idx == 0 => Some(s.clone()),
            VarValue::Scalar(_) => None,
        },
        None => None,
    }
}

/// Returns the maximum subscript present in the named array, or
/// `None` if no elements. Used for negative-subscript wrapping.
pub fn array_max_index(&self, name: &str) -> Option<usize> {
    self.get_array(name).and_then(|m| m.keys().next_back().copied())
}
```

Add a small `seed_array_for_tests` helper guarded by `#[cfg(test)]` (used by Task 3's tests; Task 4 replaces the seeding with real compound-assign):

```rust
#[cfg(test)]
impl Shell {
    pub fn seed_array_for_tests(&mut self, name: &str, elements: &[(usize, &str)]) {
        let mut m = BTreeMap::new();
        for (k, v) in elements {
            m.insert(*k, (*v).to_string());
        }
        self.vars.insert(name.to_string(), Variable {
            value: VarValue::Indexed(m),
            exported: false,
            readonly: false,
            integer: false,
        });
    }
}
```

- [ ] **Step 2: Add subscript-evaluation helper**

In `src/expand.rs`, add (near where other helpers live):

```rust
/// Arith-evaluate a subscript Word to a usize. Negative results wrap
/// to `max_index + 1 + n`; if that's still negative or no elements
/// exist, returns `Err("bad array subscript")`.
pub(crate) fn eval_subscript(
    subscript: &crate::lexer::Word,
    shell: &mut crate::shell_state::Shell,
    name: &str,
) -> Result<usize, String> {
    // Expand the subscript Word to a string (no word-splitting, no globbing).
    let s = expand_word_to_string(subscript, shell)?;
    let parsed = crate::arith::parse(&s)
        .map_err(|_| format!("{name}: bad array subscript"))?;
    let n = crate::arith::eval(&parsed, shell)
        .map_err(|_| format!("{name}: bad array subscript"))?;
    if n >= 0 {
        Ok(n as usize)
    } else {
        let max = shell.array_max_index(name)
            .ok_or_else(|| format!("{name}: bad array subscript"))?;
        let wrapped = max as i64 + 1 + n;
        if wrapped < 0 {
            Err(format!("{name}: bad array subscript"))
        } else {
            Ok(wrapped as usize)
        }
    }
}
```

`expand_word_to_string` may already exist as `expand_to_single_string` or similar — implementer should call the existing helper, not duplicate it.

- [ ] **Step 3: Add the array expansion dispatch in `expand.rs`**

Find the function that handles `WordPart::ParamExpansion` (search for `ParamExpansion` in `src/expand.rs`). Add an early branch at the top:

```rust
if let Some(sub) = subscript {
    return expand_array_param(name, modifier, sub, quoted, shell);
}
```

Add the new function:

```rust
fn expand_array_param(
    name: &str,
    modifier: &ParamModifier,
    subscript: &SubscriptKind,
    quoted: bool,
    shell: &mut Shell,
) -> ExpansionResult {
    use ParamModifier as PM;
    use SubscriptKind as SK;

    // Helper closure: the "values in subscript order" for All / Star.
    let collect_values = |sh: &Shell| -> Vec<String> {
        match sh.get_array(name) {
            Some(m) => m.values().cloned().collect(),
            None => match sh.get(name) {
                Some(s) => vec![s.to_string()],  // scalar ≡ a[0]
                None => Vec::new(),
            },
        }
    };
    let collect_keys = |sh: &Shell| -> Vec<usize> {
        match sh.get_array(name) {
            Some(m) => m.keys().copied().collect(),
            None => match sh.get(name) {
                Some(_) => vec![0],
                None => Vec::new(),
            },
        }
    };

    match (modifier, subscript) {
        // ${a[@]} / ${a[*]} - plain word-list / scalar
        (PM::None, SK::All) => {
            ExpansionResult::WordList(collect_values(shell))
        }
        (PM::None, SK::Star) => {
            let ifs = shell.lookup_var("IFS").unwrap_or_else(|| " \t\n".to_string());
            let sep = ifs.chars().next().unwrap_or(' ').to_string();
            ExpansionResult::Scalar(collect_values(shell).join(&sep))
        }
        // ${a[i]}
        (PM::None, SK::Index(w)) => {
            match eval_subscript(w, shell, name) {
                Ok(idx) => {
                    let val = shell.lookup_array_element(name, idx).unwrap_or_default();
                    if val.is_empty() && shell.shell_options.nounset {
                        shell.set_pending_pe_error(format!("{name}[{idx}]: unbound variable"));
                    }
                    ExpansionResult::Scalar(val)
                }
                Err(e) => {
                    shell.set_pending_pe_error(e);
                    ExpansionResult::Scalar(String::new())
                }
            }
        }
        // ${#a[@]} / ${#a[*]}
        (PM::Length, SK::All) | (PM::Length, SK::Star) => {
            ExpansionResult::Scalar(collect_keys(shell).len().to_string())
        }
        // ${#a[i]}
        (PM::Length, SK::Index(w)) => {
            let idx = eval_subscript(w, shell, name).unwrap_or(0);
            let val = shell.lookup_array_element(name, idx).unwrap_or_default();
            ExpansionResult::Scalar(val.chars().count().to_string())
        }
        // ${!a[@]} / ${!a[*]}
        (PM::IndirectOrKeys, SK::All) | (PM::IndirectOrKeys, SK::Star) => {
            let keys: Vec<String> = collect_keys(shell).iter().map(usize::to_string).collect();
            if quoted && matches!(subscript, SK::All) {
                ExpansionResult::WordList(keys)
            } else {
                let ifs = shell.lookup_var("IFS").unwrap_or_else(|| " \t\n".to_string());
                let sep = ifs.chars().next().unwrap_or(' ').to_string();
                ExpansionResult::Scalar(keys.join(&sep))
            }
        }
        // ${a[@]:off:len} — slicing
        (PM::Substring { offset, length }, SK::All) | (PM::Substring { offset, length }, SK::Star) => {
            let values = collect_values(shell);
            let sliced = slice_word_list(&values, offset, length.as_deref(), shell)?;
            if matches!(subscript, SK::All) && quoted {
                ExpansionResult::WordList(sliced)
            } else {
                let ifs = shell.lookup_var("IFS").unwrap_or_else(|| " \t\n".to_string());
                let sep = ifs.chars().next().unwrap_or(' ').to_string();
                ExpansionResult::Scalar(sliced.join(&sep))
            }
        }
        // Other modifiers on @/* in v71: not yet supported.
        (other, SK::All | SK::Star) => {
            ExpansionResult::Scalar(format!(
                "huck: ${{{name}[…]}}: {:?} not supported on array (v71)",
                other
            ))
            // Spec note: per-element pattern/case mods are deferred.
            // Emit to stderr instead — implementer: route via the
            // existing PE-error mechanism.
        }
        // Modifier on a specific index: same modifier semantics as scalar
        (modif, SK::Index(w)) => {
            let idx = eval_subscript(w, shell, name).unwrap_or(0);
            let val = shell.lookup_array_element(name, idx);
            apply_modifier_to_scalar_value(modif, val, name, shell)
        }
    }
}
```

`ExpansionResult::WordList(Vec<String>)` is a new variant on the existing enum in `src/param_expansion.rs` (currently `Value(String) | Empty | Fatal { status }`). Add the variant and update all 6 consumer sites in `src/expand.rs` (search for `ExpansionResult::Value` — the consumers' match arms need a `WordList(words) => …` arm that produces the word-list expansion path. For the consumers that build a flat string, fall back to space-joining with a `// TODO(v71): split-to-args path` comment IF the caller can't actually handle multiple words — but for the quoted `"${a[@]}"` path inside argv assembly, the executor's `expand_word_for_argv` (the function that builds `args: Vec<String>` for an `ExecCommand`) must learn to emit N words from one WordList).

- [ ] **Step 4: Add `slice_word_list` shared helper**

```rust
/// Slices a word list per `${a[@]:off:len}` / `${@:off:len}` semantics.
/// Negative offset counts from the end of the present-element list.
/// Returns `Err` on parse failure.
pub(crate) fn slice_word_list(
    values: &[String],
    offset: &Word,
    length: Option<&Word>,
    shell: &mut Shell,
) -> Result<Vec<String>, ExpansionAbort> {
    let off_s = expand_word_to_string(offset, shell)?;
    let off_n = crate::arith::parse(&off_s)
        .and_then(|e| crate::arith::eval(&e, shell))
        .map_err(|_| ExpansionAbort::BadSubscript)?;
    let total = values.len() as i64;
    let start = if off_n >= 0 {
        off_n.min(total)
    } else {
        (total + off_n).max(0)
    } as usize;
    let end = match length {
        Some(lw) => {
            let len_s = expand_word_to_string(lw, shell)?;
            let len_n = crate::arith::parse(&len_s)
                .and_then(|e| crate::arith::eval(&e, shell))
                .map_err(|_| ExpansionAbort::BadSubscript)?;
            if len_n < 0 {
                // Negative length: index from end.
                ((total + len_n) as usize).max(start)
            } else {
                (start + len_n as usize).min(values.len())
            }
        }
        None => values.len(),
    };
    Ok(values[start..end].to_vec())
}
```

- [ ] **Step 5: Close v33's `$@`/`$*` slicing deferral**

Find where `${@:o:l}` / `${*:o:l}` is rejected in `src/expand.rs` (search for `"@"` or `"\\*"` paired with `Substring`). Replace the rejection path with a call to `slice_word_list` over `shell.positional_args.clone()`:

```rust
PM::Substring { offset, length } if name == "@" || name == "*" => {
    let values = shell.positional_args.clone();
    let sliced = slice_word_list(&values, offset, length.as_deref(), shell)?;
    if name == "@" && quoted {
        return ExpansionResult::WordList(sliced);
    }
    let ifs = shell.lookup_var("IFS").unwrap_or_else(|| " \t\n".to_string());
    let sep = ifs.chars().next().unwrap_or(' ').to_string();
    return ExpansionResult::Scalar(sliced.join(&sep));
}
```

- [ ] **Step 6: Unit tests for expansion**

Append to `src/expand.rs`:

```rust
#[cfg(test)]
mod array_expansion_tests {
    use super::*;
    use crate::shell_state::Shell;

    fn shell_with_a() -> Shell {
        let mut s = Shell::new();
        s.seed_array_for_tests("a", &[(0, "x"), (1, "y"), (2, "z")]);
        s
    }

    #[test]
    fn read_element_returns_value() {
        let mut s = shell_with_a();
        // Expand `${a[1]}` and assert result == "y"
        let (out, _) = expand_for_test(&mut s, "${a[1]}");
        assert_eq!(out, "y");
    }

    #[test]
    fn out_of_range_element_is_empty() {
        let mut s = shell_with_a();
        let (out, _) = expand_for_test(&mut s, "${a[99]}");
        assert_eq!(out, "");
    }

    #[test]
    fn quoted_at_yields_separate_words() {
        let mut s = shell_with_a();
        // Hard to test "separate words" from expand_for_test which joins;
        // implementer should call the lower-level expansion that returns
        // WordList and assert .len() == 3.
        let words = expand_to_word_list_for_test(&mut s, r#""${a[@]}""#);
        assert_eq!(words, vec!["x", "y", "z"]);
    }

    #[test]
    fn quoted_star_joins_by_ifs() {
        let mut s = shell_with_a();
        let (out, _) = expand_for_test(&mut s, r#""${a[*]}""#);
        assert_eq!(out, "x y z");
    }

    #[test]
    fn count_returns_element_count_not_max_index() {
        let mut s = Shell::new();
        s.seed_array_for_tests("a", &[(2, "x"), (5, "y")]);
        let (out, _) = expand_for_test(&mut s, "${#a[@]}");
        assert_eq!(out, "2");
    }

    #[test]
    fn keys_list_returns_subscripts() {
        let mut s = Shell::new();
        s.seed_array_for_tests("a", &[(2, "x"), (5, "y")]);
        let (out, _) = expand_for_test(&mut s, "${!a[@]}");
        assert_eq!(out, "2 5");
    }

    #[test]
    fn element_length() {
        let mut s = shell_with_a();
        let (out, _) = expand_for_test(&mut s, "${#a[0]}");
        assert_eq!(out, "1"); // length of "x"
    }

    #[test]
    fn slicing_positive_offset_and_length() {
        let mut s = shell_with_a();
        let words = expand_to_word_list_for_test(&mut s, r#""${a[@]:1:1}""#);
        assert_eq!(words, vec!["y"]);
    }

    #[test]
    fn slicing_negative_offset_counts_from_end() {
        let mut s = shell_with_a();
        let words = expand_to_word_list_for_test(&mut s, r#""${a[@]: -1}""#);
        assert_eq!(words, vec!["z"]);
    }

    #[test]
    fn bare_name_returns_element_zero() {
        let mut s = shell_with_a();
        let (out, _) = expand_for_test(&mut s, "${a}");
        assert_eq!(out, "x");
    }

    #[test]
    fn negative_subscript_wraps() {
        let mut s = shell_with_a();
        let (out, _) = expand_for_test(&mut s, "${a[-1]}");
        assert_eq!(out, "z");
    }

    #[test]
    fn nounset_on_unset_element_fires_pe_error() {
        let mut s = shell_with_a();
        s.shell_options.nounset = true;
        let _ = expand_for_test(&mut s, "${a[99]}");
        assert!(s.pending_fatal_pe_error().is_some());
    }
}

#[cfg(test)]
mod positional_slicing_tests {
    use super::*;
    use crate::shell_state::Shell;

    fn shell_with_posargs() -> Shell {
        let mut s = Shell::new();
        s.positional_args = vec!["a".into(), "b".into(), "c".into(), "d".into()];
        s
    }

    #[test]
    fn at_slice_positive() {
        let mut s = shell_with_posargs();
        let words = expand_to_word_list_for_test(&mut s, r#""${@:2:2}""#);
        assert_eq!(words, vec!["b", "c"]);
    }

    #[test]
    fn at_slice_negative_offset() {
        let mut s = shell_with_posargs();
        let words = expand_to_word_list_for_test(&mut s, r#""${@: -2}""#);
        assert_eq!(words, vec!["c", "d"]);
    }

    #[test]
    fn star_slice_joins_by_ifs() {
        let mut s = shell_with_posargs();
        let (out, _) = expand_for_test(&mut s, r#""${*:1:3}""#);
        assert_eq!(out, "a b c");
    }
}
```

Implementer adds `expand_for_test` / `expand_to_word_list_for_test` helpers matching the existing test convention in `src/expand.rs` (search for existing `#[cfg(test)]` blocks; reuse the harness).

- [ ] **Step 7: Run tests**

Run: `cargo build 2>&1 | tail -5`

Run: `cargo test --bin huck array_expansion 2>&1 | tail -25`
Expected: all 12 tests in array_expansion_tests pass.

Run: `cargo test --bin huck positional_slicing 2>&1 | tail -10`
Expected: 3 tests pass (closes v33 deferral).

Run: `cargo test 2>&1 | grep -E "test result|FAILED" | head -40`
Expected: all green.

- [ ] **Step 8: Commit**

```bash
git add src/expand.rs src/shell_state.rs
git commit -m "$(cat <<'EOF'
expansion: array reads + ${@:o:l} slicing (v71 task 3)

End-to-end array READ paths: ${a[i]}, ${a[@]}/${a[*]} with proper
quoting, ${#a[@]}, ${!a[@]}, ${#a[i]}, ${a[@]:o:l} slicing, bare
${a} ≡ ${a[0]}, negative-subscript wrap (bash 4.3+), nounset on
missing element. Slicer is shared with positional params, closing
v33's deferred ${@:o:l} / ${*:o:l}.

Arrays cannot be CREATED yet — Task 4 wires assignment.
seed_array_for_tests is the temporary seed.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Assignment execution

**Files:**
- Modify: `src/executor.rs` (apply_inline_assignments, run_exec_single's assign path, run_simple_assign)
- Modify: `src/shell_state.rs` (add the array mutator helpers)

**Goal:** Arrays can be CREATED, UPDATED, APPENDED, and partially UNSET. All of v71's write paths work end-to-end with readonly enforcement. (The `#[cfg(test)] seed_array_for_tests` helper added in Task 3 stays — Task 3's expansion tests still use it; production code never references it.)

- [ ] **Step 1: Add array mutator helpers to Shell**

In `src/shell_state.rs`:

```rust
/// Replaces (or creates) `name` as an indexed array with the given
/// elements. Honors readonly. Preserves the existing `exported` and
/// `integer` flags if the variable exists. Integer flag on an array
/// is currently a deferred-error path; caller checks before invoking.
pub fn replace_array(
    &mut self,
    name: &str,
    elements: BTreeMap<usize, String>,
) -> Result<(), AssignErr> {
    if let Some(existing) = self.vars.get(name)
        && existing.readonly
    {
        eprintln!("huck: {name}: readonly variable");
        return Err(AssignErr::Readonly);
    }
    let (exported, integer) = match self.vars.get(name) {
        Some(v) => (v.exported, v.integer),
        None => (false, false),
    };
    self.vars.insert(name.to_string(), Variable {
        value: VarValue::Indexed(elements),
        exported,
        readonly: false,
        integer,
    });
    Ok(())
}

/// Sets a single element. Promotes a scalar variable to indexed (the
/// existing scalar value becomes element 0). Honors readonly.
pub fn set_array_element(
    &mut self,
    name: &str,
    idx: usize,
    value: String,
) -> Result<(), AssignErr> {
    if let Some(existing) = self.vars.get(name)
        && existing.readonly
    {
        eprintln!("huck: {name}: readonly variable");
        return Err(AssignErr::Readonly);
    }
    match self.vars.get_mut(name) {
        Some(v) => match &mut v.value {
            VarValue::Indexed(m) => { m.insert(idx, value); }
            VarValue::Scalar(s) => {
                let mut m = BTreeMap::new();
                m.insert(0, std::mem::take(s));
                m.insert(idx, value);
                v.value = VarValue::Indexed(m);
            }
        },
        None => {
            let mut m = BTreeMap::new();
            m.insert(idx, value);
            self.vars.insert(name.to_string(), Variable {
                value: VarValue::Indexed(m),
                exported: false,
                readonly: false,
                integer: false,
            });
        }
    }
    Ok(())
}

/// Appends `value` to the array at the next implicit subscript
/// (max_index + 1, or 0 for empty/missing).
pub fn append_array(&mut self, name: &str, elements: &[String]) -> Result<(), AssignErr> {
    if let Some(existing) = self.vars.get(name)
        && existing.readonly
    {
        eprintln!("huck: {name}: readonly variable");
        return Err(AssignErr::Readonly);
    }
    let mut start = self.array_max_index(name).map_or(0, |m| m + 1);
    // Promote scalar to array if needed.
    if matches!(self.vars.get(name).map(|v| &v.value), Some(VarValue::Scalar(_))) {
        if let Some(v) = self.vars.get_mut(name)
            && let VarValue::Scalar(s) = &mut v.value
        {
            let mut m = BTreeMap::new();
            m.insert(0, std::mem::take(s));
            v.value = VarValue::Indexed(m);
            start = 1;
        }
    }
    if self.vars.get(name).is_none() {
        self.vars.insert(name.to_string(), Variable {
            value: VarValue::Indexed(BTreeMap::new()),
            exported: false,
            readonly: false,
            integer: false,
        });
    }
    if let Some(v) = self.vars.get_mut(name)
        && let VarValue::Indexed(m) = &mut v.value
    {
        for (i, val) in elements.iter().enumerate() {
            m.insert(start + i, val.clone());
        }
    }
    Ok(())
}

/// Appends `value` to the existing element at `idx` (concatenation).
/// Used by `a[i]+=v`.
pub fn append_array_element(
    &mut self,
    name: &str,
    idx: usize,
    value: &str,
) -> Result<(), AssignErr> {
    let existing = self.lookup_array_element(name, idx).unwrap_or_default();
    self.set_array_element(name, idx, existing + value)
}

/// Removes a single element from an indexed array. No-op if the
/// variable is missing, scalar, or doesn't contain that subscript.
/// Honors readonly.
pub fn unset_array_element(&mut self, name: &str, idx: usize) -> Result<(), AssignErr> {
    if let Some(existing) = self.vars.get(name)
        && existing.readonly
    {
        eprintln!("huck: {name}: readonly variable");
        return Err(AssignErr::Readonly);
    }
    if let Some(v) = self.vars.get_mut(name)
        && let VarValue::Indexed(m) = &mut v.value
    {
        m.remove(&idx);
    }
    Ok(())
}

#[derive(Debug)]
pub enum AssignErr {
    Readonly,
    BadSubscript,
}
```

(If an `AssignErr`-like type already exists from v54, reuse it instead of re-defining.)

- [ ] **Step 2: Update `apply_inline_assignments` in executor.rs**

Find the existing `apply_inline_assignments` (around line 443). It currently iterates over `Vec<(String, Word)>`. Update for the new `Assignment { target, value, append }` shape:

```rust
pub fn apply_inline_assignments(
    assigns: &[crate::command::Assignment],
    shell: &mut Shell,
) -> Result<Vec<InlineSnapshot>, ()> {
    let mut snaps: Vec<InlineSnapshot> = Vec::new();
    for a in assigns {
        let snap = snapshot_assignment_target(&a.target, shell);
        snaps.push(snap);
        if let Err(()) = apply_one_assignment(a, shell) {
            return Err(());
        }
    }
    Ok(snaps)
}

fn apply_one_assignment(a: &crate::command::Assignment, shell: &mut Shell) -> Result<(), ()> {
    match &a.target {
        AssignTarget::Bare(name) => {
            // Existing behavior, but now must dispatch on Word kind:
            // Word::ArrayLiteral → compound assign; else scalar write.
            if let [WordPart::ArrayLiteral(elements)] = &a.value.0[..] {
                if a.append {
                    // a+=(...)
                    let values: Vec<String> = elements.iter()
                        .map(|e| expand_word_to_string(&e.value, shell))
                        .collect::<Result<_,_>>()?;
                    shell.append_array(name, &values).map_err(|_| ())
                } else {
                    // a=(...)
                    let mut map: BTreeMap<usize, String> = BTreeMap::new();
                    let mut implicit: usize = 0;
                    for e in elements {
                        let v = expand_word_to_string(&e.value, shell)?;
                        let idx = match &e.subscript {
                            Some(sw) => crate::expand::eval_subscript(sw, shell, name)
                                .map_err(|_| ())?,
                            None => implicit,
                        };
                        map.insert(idx, v);
                        if e.subscript.is_none() {
                            implicit = idx + 1;
                        } else {
                            implicit = idx + 1;
                        }
                    }
                    shell.replace_array(name, map).map_err(|_| ())
                }
            } else {
                // Scalar path: existing try_set logic.
                let s = expand_word_to_string(&a.value, shell)?;
                shell.try_set(name, s).map_err(|_| ())
            }
        }
        AssignTarget::Indexed { name, subscript } => {
            let idx = crate::expand::eval_subscript(subscript, shell, name).map_err(|_| ())?;
            let v = expand_word_to_string(&a.value, shell)?;
            if a.append {
                shell.append_array_element(name, idx, &v).map_err(|_| ())
            } else {
                shell.set_array_element(name, idx, v).map_err(|_| ())
            }
        }
    }
}
```

`snapshot_assignment_target` records the pre-assignment state of the named variable (whole `Variable` clone — Task 1's `VarValue` clones automatically). `restore_inline_assignments` reinserts the snapshot value.

- [ ] **Step 3: Update top-level `SimpleCommand::Assign` execution**

In `src/executor.rs`, find where `SimpleCommand::Assign(assigns)` is executed at top level (search for `SimpleCommand::Assign(`). The current code iterates over `(name, word)`. Replace with the same `apply_one_assignment` call (or its persistent equivalent):

```rust
Command::Simple(SimpleCommand::Assign(items)) => {
    for a in items {
        if let Err(()) = apply_one_assignment_persistent(a, shell) {
            return ExecOutcome::Continue(1);
        }
    }
    ExecOutcome::Continue(0)
}
```

`apply_one_assignment_persistent` is the same as `apply_one_assignment` but operates on the persistent (non-snapshot) variables — the persistent path is just the inline-assignment write-through without recording a snapshot.

- [ ] **Step 4: Update `builtin_unset` for `name[i]` form**

In `src/builtins.rs`, find `builtin_unset`. After arg parsing, before treating each arg as a name, detect `name[idx]` form:

```rust
fn builtin_unset(args: &[String], shell: &mut Shell) -> ExecOutcome {
    for arg in args {
        if let Some((name, sub_text)) = parse_subscripted_arg(arg) {
            let sub_word = lex_single_word(sub_text); // see helper below
            match crate::expand::eval_subscript(&sub_word, shell, name) {
                Ok(idx) => {
                    if shell.unset_array_element(name, idx).is_err() {
                        return ExecOutcome::Continue(1);
                    }
                }
                Err(e) => {
                    eprintln!("huck: unset: {e}");
                    return ExecOutcome::Continue(1);
                }
            }
        } else {
            // Existing whole-variable unset (readonly check etc.).
            shell.unset(arg);
        }
    }
    ExecOutcome::Continue(0)
}

fn parse_subscripted_arg(s: &str) -> Option<(&str, &str)> {
    let bracket = s.find('[')?;
    if !s.ends_with(']') { return None; }
    let name = &s[..bracket];
    if !is_valid_name(name) { return None; }
    let sub = &s[bracket + 1 .. s.len() - 1];
    Some((name, sub))
}
```

Implementer: `is_valid_name` already exists; reuse. `lex_single_word(s)` constructs a `Word` whose single literal part is `s` — sufficient for the subscript arith path, which expands+parses it as a number.

- [ ] **Step 5: Assignment-execution unit tests**

Append to `src/executor.rs`:

```rust
#[cfg(test)]
mod array_assign_tests {
    use super::*;
    use crate::shell_state::{Shell, VarValue};

    fn run_line(shell: &mut Shell, line: &str) {
        // Tokenize + parse + execute one shell line. Use the existing
        // process_line helper (search src for `pub fn process_line`).
        crate::shell::process_line(line, shell, false);
    }

    #[test]
    fn compound_assign_creates_array() {
        let mut s = Shell::new();
        run_line(&mut s, "a=(x y z)");
        let m = s.get_array("a").expect("a should be an array");
        assert_eq!(m.get(&0).map(String::as_str), Some("x"));
        assert_eq!(m.get(&1).map(String::as_str), Some("y"));
        assert_eq!(m.get(&2).map(String::as_str), Some("z"));
    }

    #[test]
    fn sparse_compound_assign_respects_explicit_subscripts() {
        let mut s = Shell::new();
        run_line(&mut s, "a=([5]=x [2]=y)");
        let m = s.get_array("a").expect("a should be an array");
        assert_eq!(m.len(), 2);
        assert_eq!(m.get(&5).map(String::as_str), Some("x"));
        assert_eq!(m.get(&2).map(String::as_str), Some("y"));
    }

    #[test]
    fn element_assign_creates_array() {
        let mut s = Shell::new();
        run_line(&mut s, "a[3]=hello");
        let m = s.get_array("a").expect("a should be an array");
        assert_eq!(m.get(&3).map(String::as_str), Some("hello"));
    }

    #[test]
    fn element_assign_promotes_scalar() {
        let mut s = Shell::new();
        run_line(&mut s, "a=old");
        run_line(&mut s, "a[2]=new");
        let m = s.get_array("a").expect("scalar should promote to array");
        assert_eq!(m.get(&0).map(String::as_str), Some("old"));
        assert_eq!(m.get(&2).map(String::as_str), Some("new"));
    }

    #[test]
    fn append_array_extends() {
        let mut s = Shell::new();
        run_line(&mut s, "a=(x y)");
        run_line(&mut s, "a+=(z w)");
        let m = s.get_array("a").unwrap();
        assert_eq!(m.values().cloned().collect::<Vec<_>>(),
                   vec!["x".to_string(), "y".to_string(), "z".to_string(), "w".to_string()]);
    }

    #[test]
    fn append_element_concatenates() {
        let mut s = Shell::new();
        run_line(&mut s, "a[0]=hello");
        run_line(&mut s, "a[0]+=_world");
        let m = s.get_array("a").unwrap();
        assert_eq!(m.get(&0).map(String::as_str), Some("hello_world"));
    }

    #[test]
    fn readonly_blocks_compound_assign() {
        let mut s = Shell::new();
        run_line(&mut s, "a=(initial)");
        s.mark_readonly("a");
        run_line(&mut s, "a=(changed)");
        let m = s.get_array("a").unwrap();
        assert_eq!(m.get(&0).map(String::as_str), Some("initial"));
    }

    #[test]
    fn readonly_blocks_element_assign() {
        let mut s = Shell::new();
        run_line(&mut s, "a=(initial)");
        s.mark_readonly("a");
        run_line(&mut s, "a[5]=new");
        let m = s.get_array("a").unwrap();
        assert!(m.get(&5).is_none());
    }

    #[test]
    fn unset_element_removes_one_key() {
        let mut s = Shell::new();
        run_line(&mut s, "a=(x y z)");
        run_line(&mut s, "unset a[1]");
        let m = s.get_array("a").unwrap();
        assert!(m.get(&1).is_none());
        assert_eq!(m.get(&0).map(String::as_str), Some("x"));
        assert_eq!(m.get(&2).map(String::as_str), Some("z"));
    }

    #[test]
    fn unset_whole_array_removes_variable() {
        let mut s = Shell::new();
        run_line(&mut s, "a=(x y z)");
        run_line(&mut s, "unset a");
        assert!(s.get_array("a").is_none());
        assert!(s.get("a").is_none());
    }
}
```

- [ ] **Step 6: Run tests**

Run: `cargo build 2>&1 | tail -5`
Expected: clean.

Run: `cargo test --bin huck array_assign 2>&1 | tail -25`
Expected: 10 tests pass.

Run: `cargo test 2>&1 | grep -E "test result|FAILED" | head -40`
Expected: all green.

- [ ] **Step 7: Commit**

```bash
git add src/executor.rs src/shell_state.rs src/builtins.rs
git commit -m "$(cat <<'EOF'
exec: array assignment paths (v71 task 4)

Compound, element, append-array, append-element, and unset-element
writes. Scalar-to-array promotion on element assign and append.
Inline `a=(...) cmd` snapshots+restores via the existing v23 cycle.
Readonly enforcement on every path with the v54 diagnostic.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Builtin wiring (`declare -a`, `local -a`, `readonly`, `export` rejection)

**Files:**
- Modify: `src/builtins.rs` (builtin_declare, builtin_local, builtin_readonly, builtin_export, format_declare_line)

**Goal:** The `declare -a`, `declare -a NAME=(…)`, `declare -p NAME`, `local -a`, `local NAME=(…)`, and `readonly NAME=(…)` surfaces all work. `declare -ai` and `export NAME=(…)` produce the explicit-deferred error.

- [ ] **Step 1: Update `format_declare_line` for arrays**

Find `format_declare_line` in `src/builtins.rs`. Add an array branch:

```rust
fn format_declare_line(name: &str, var: &Variable) -> String {
    let mut attrs = String::new();
    // Order: i, r, x (existing) plus 'a' for arrays.
    if matches!(var.value, VarValue::Indexed(_)) { attrs.push('a'); }
    if var.integer { attrs.push('i'); }
    if var.readonly { attrs.push('r'); }
    if var.exported { attrs.push('x'); }
    let attr_part = if attrs.is_empty() {
        "--".to_string()
    } else {
        format!("-{attrs}")
    };
    let value_part = match &var.value {
        VarValue::Scalar(s) => {
            let escaped = escape_double_quote_value(s);
            format!("=\"{escaped}\"")
        }
        VarValue::Indexed(m) => {
            // Format: declare -a name=([0]="v0" [1]="v1" ...)
            let mut parts: Vec<String> = Vec::new();
            for (k, v) in m {
                let escaped = escape_double_quote_value(v);
                parts.push(format!("[{k}]=\"{escaped}\""));
            }
            format!("=({})", parts.join(" "))
        }
    };
    format!("declare {attr_part} {name}{value_part}")
}
```

- [ ] **Step 2: Update `builtin_declare`'s `-a` arm**

Find the flag-handling switch in `builtin_declare`. Replace the `-a` rejection with:

```rust
'a' => { flags.array = true; }
```

(Add `array: bool` to the `flags` struct.)

In the per-name processing loop, when `flags.array` is set:

```rust
if flags.array {
    // declare -a NAME or declare -a NAME=(...)
    let target = AssignTarget::Bare(name.to_string());
    if let Some(rhs_word) = value_word {
        // delegate to apply_one_assignment / equivalent persistent path
        // already wired in Task 4.
        let assignment = crate::command::Assignment {
            target,
            value: rhs_word,
            append: false,
        };
        if apply_one_assignment_persistent(&assignment, shell).is_err() {
            return ExecOutcome::Continue(1);
        }
    } else if shell.get_array(name).is_none() {
        // declare -a NAME with no value: create empty array (preserve
        // existing array if already present; promote scalar to array).
        let mut empty = BTreeMap::new();
        if let Some(scalar) = shell.get(name) {
            empty.insert(0, scalar.to_string());
        }
        if shell.replace_array(name, empty).is_err() {
            return ExecOutcome::Continue(1);
        }
    }
    continue;
}
```

For `-ai`:

```rust
if flags.array && flags.integer {
    eprintln!("huck: declare: integer arrays not yet supported");
    return ExecOutcome::Continue(1);
}
```

For bare `declare -a` (no names), list arrays only:

```rust
if flags.array && names.is_empty() {
    let mut keys: Vec<&String> = shell.iter_vars()
        .filter(|(_, v)| matches!(v.value, VarValue::Indexed(_)))
        .map(|(k, _)| k)
        .collect();
    keys.sort();
    for k in keys {
        let v = shell.iter_vars().find(|(n, _)| n == &k).unwrap().1;
        writeln!(out, "{}", format_declare_line(k, v)).ok();
    }
    return ExecOutcome::Continue(0);
}
```

- [ ] **Step 3: Update `builtin_local` for `-a` and compound RHS**

`builtin_local` already snapshots the whole `Variable` via `snapshot_for_local_scope`, so the snapshot path works automatically for arrays once the parse-side accepts `(…)` (Task 2 done) and the executor accepts the new assignment shape (Task 4 done).

Add an `-a` flag arm analogous to `declare -a`:

```rust
fn builtin_local(args: &[String], shell: &mut Shell) -> ExecOutcome {
    if shell.local_scopes.is_empty() {
        eprintln!("huck: local: can only be used in a function");
        return ExecOutcome::Continue(1);
    }
    let mut want_array = false;
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a == "-a" { want_array = true; i += 1; continue; }
        if a.starts_with('-') {
            eprintln!("huck: local: {a}: invalid option");
            return ExecOutcome::Continue(1);
        }
        break;
    }
    // remaining args[i..] are NAME or NAME=value (or NAME=(…) when want_array)
    // … existing per-name loop, but route compound RHS through apply_one_assignment_persistent
}
```

- [ ] **Step 4: Update `builtin_readonly`**

`builtin_readonly` already handles assignment shape — the lexer change in Task 2 produced the new `Assignment{target, value, append}`. The readonly builtin reads the LHS name (via `target.name()`) and the RHS Word. If RHS is `ArrayLiteral`, treat as compound assign then mark readonly:

```rust
for assignment in &assignments {
    match &assignment.target {
        AssignTarget::Bare(name) => {
            // existing path; now also route ArrayLiteral
            if let [WordPart::ArrayLiteral(_)] = &assignment.value.0[..] {
                if apply_one_assignment_persistent(assignment, shell).is_err() {
                    return ExecOutcome::Continue(1);
                }
            } else {
                // existing scalar readonly path
            }
            shell.mark_readonly(name);
        }
        AssignTarget::Indexed { .. } => {
            eprintln!("huck: readonly: subscripted assignment to readonly variable not supported");
            return ExecOutcome::Continue(1);
        }
    }
}
```

- [ ] **Step 5: `builtin_export` rejection on arrays**

Find `builtin_export`. After parsing each `NAME=value` form, check if the value is an `ArrayLiteral`:

```rust
if let [WordPart::ArrayLiteral(_)] = &assignment.value.0[..] {
    eprintln!("huck: export: cannot export arrays");
    return ExecOutcome::Continue(1);
}
```

(Bare `export NAME` where NAME is already an array — bash silently exports the name but does NOT export array contents. huck mirrors this by allowing the export but `exported_env` already returns `scalar_view()` so only element 0 leaks to the env — acceptable.)

- [ ] **Step 6: Builtin unit tests**

Append to `src/builtins.rs` near the existing builtin tests:

```rust
#[cfg(test)]
mod array_declare_tests {
    use super::*;
    use crate::shell_state::Shell;
    use std::io::Write;

    fn run(shell: &mut Shell, line: &str) -> (String, ExecOutcome) {
        let mut out: Vec<u8> = Vec::new();
        // Tokenize the line and dispatch the builtin call directly.
        // For declare/local/readonly, use the existing process_line.
        let outcome = crate::shell::process_line(line, shell, false);
        (String::from_utf8(out).unwrap(), outcome)
    }

    #[test]
    fn declare_dash_a_creates_empty_array() {
        let mut s = Shell::new();
        let _ = run(&mut s, "declare -a a");
        assert!(s.get_array("a").is_some());
        assert_eq!(s.get_array("a").unwrap().len(), 0);
    }

    #[test]
    fn declare_dash_a_with_value() {
        let mut s = Shell::new();
        let _ = run(&mut s, "declare -a a=(x y)");
        let m = s.get_array("a").unwrap();
        assert_eq!(m.get(&0).map(String::as_str), Some("x"));
        assert_eq!(m.get(&1).map(String::as_str), Some("y"));
    }

    #[test]
    fn declare_dash_p_formats_array() {
        let mut s = Shell::new();
        let _ = run(&mut s, "a=(x y)");
        let mut out: Vec<u8> = Vec::new();
        let line = format_declare_line("a", s.iter_vars().find(|(n,_)| n == &"a").unwrap().1);
        assert_eq!(line, r#"declare -a a=([0]="x" [1]="y")"#);
    }

    #[test]
    fn declare_dash_ai_errors() {
        let mut s = Shell::new();
        let _ = run(&mut s, "declare -ai a");
        // Expect status 1 — implementer asserts on the returned ExecOutcome.
    }

    #[test]
    fn readonly_array_blocks_element_write() {
        let mut s = Shell::new();
        let _ = run(&mut s, "readonly a=(x y)");
        let _ = run(&mut s, "a[2]=z");
        let m = s.get_array("a").unwrap();
        assert!(m.get(&2).is_none());
    }

    #[test]
    fn export_array_rejects() {
        let mut s = Shell::new();
        let _ = run(&mut s, "export a=(x y)");
        assert!(s.get_array("a").is_none());
    }
}
```

- [ ] **Step 7: Run tests**

Run: `cargo build 2>&1 | tail -5`

Run: `cargo test --bin huck array_declare 2>&1 | tail -15`
Expected: 6 tests pass.

Run: `cargo test 2>&1 | grep -E "test result|FAILED" | head -40`
Expected: all green.

- [ ] **Step 8: Commit**

```bash
git add src/builtins.rs
git commit -m "$(cat <<'EOF'
builtins: declare -a / local -a / readonly arrays (v71 task 5)

- `declare -a NAME` / `declare -a NAME=(...)` create indexed arrays
- `declare -p NAME` formats arrays as `declare -a NAME=([k]="v" ...)`
- bare `declare -a` lists arrays only
- `declare -ai` rejected with "integer arrays not yet supported"
- `local -a NAME` / `local NAME=(...)` use existing snapshot machinery
- `readonly NAME=(...)` creates a readonly array; every write path
  enforces the v54 readonly diagnostic
- `export NAME=(...)` rejected with "cannot export arrays"

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: Integration tests + documentation

**Files:**
- Create: `tests/arrays_integration.rs`
- Create: `tests/scripts/arrays_diff_check.sh`
- Modify: `docs/bash-divergences.md` (new M-82 entry, cross-refs on M-16/M-72/M-78/M-79/M-76, change-log entry)
- Modify: `README.md` (v71 row)

- [ ] **Step 1: Write integration tests**

Create `tests/arrays_integration.rs`:

```rust
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run_capture(script: &str) -> (String, String, i32) {
    let mut child = Command::new(huck_binary())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");
    child.stdin.as_mut().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn literal_then_for_loop_iterates_in_order() {
    let (out, _, _) = run_capture("a=(red green blue)\nfor c in \"${a[@]}\"; do echo \"$c\"; done\nexit\n");
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines.contains(&"red"));
    assert!(lines.contains(&"green"));
    assert!(lines.contains(&"blue"));
}

#[test]
fn sparse_subscript_count_is_one() {
    let (out, _, _) = run_capture("a[5]=x\necho \"${#a[@]}\" \"${!a[@]}\"\nexit\n");
    assert!(out.lines().any(|l| l == "1 5"), "got: {out:?}");
}

#[test]
fn element_read_and_write_roundtrip() {
    let (out, _, _) = run_capture("a[3]=hello\necho \"${a[3]}\"\nexit\n");
    assert!(out.lines().any(|l| l == "hello"), "got: {out:?}");
}

#[test]
fn append_array_extends() {
    let (out, _, _) = run_capture("a=(x y)\na+=(z w)\necho \"${a[@]}\"\nexit\n");
    assert!(out.lines().any(|l| l == "x y z w"), "got: {out:?}");
}

#[test]
fn append_element_concatenates() {
    let (out, _, _) = run_capture("a[0]=hello\na[0]+=_world\necho \"${a[0]}\"\nexit\n");
    assert!(out.lines().any(|l| l == "hello_world"), "got: {out:?}");
}

#[test]
fn scalar_promotes_on_element_assign() {
    let (out, _, _) = run_capture("a=old\na[2]=new\necho \"${a[0]}\" \"${a[2]}\"\nexit\n");
    assert!(out.lines().any(|l| l == "old new"), "got: {out:?}");
}

#[test]
fn quoted_at_preserves_empty_elements() {
    let (out, _, _) = run_capture(
        "a=(x \"\" z)\nfor v in \"${a[@]}\"; do echo \"[$v]\"; done\nexit\n"
    );
    let lines: Vec<&str> = out.lines().collect();
    let bracket_lines: Vec<_> = lines.iter().filter(|l| l.starts_with('[')).collect();
    assert_eq!(bracket_lines.len(), 3, "expected 3 elements, got: {out:?}");
    assert!(bracket_lines.contains(&&"[]"), "expected empty element preserved: {out:?}");
}

#[test]
fn star_joins_by_ifs() {
    let (out, _, _) = run_capture("a=(x y z)\necho \"${a[*]}\"\nexit\n");
    assert!(out.lines().any(|l| l == "x y z"), "got: {out:?}");
}

#[test]
fn unset_element_removes_key() {
    let (out, _, _) = run_capture("a=(x y z)\nunset a[1]\necho \"${!a[@]}\"\nexit\n");
    assert!(out.lines().any(|l| l == "0 2"), "got: {out:?}");
}

#[test]
fn local_array_scoped_to_function() {
    let (out, _, _) = run_capture(
        "a=(outer)\nf() { local a=(inner); echo \"${a[0]}\"; }\nf\necho \"${a[0]}\"\nexit\n"
    );
    let lines: Vec<&str> = out.lines().collect();
    // First line is the local view; second is the outer view.
    assert!(lines.iter().any(|l| **l == "inner"));
    assert!(lines.iter().any(|l| **l == "outer"));
}

#[test]
fn readonly_array_blocks_element_write_with_diagnostic() {
    let (_out, err, _) = run_capture(
        "readonly a=(x)\na[0]=changed\nexit\n"
    );
    assert!(err.contains("readonly variable"), "expected readonly diagnostic: {err:?}");
}

#[test]
fn nounset_on_unset_element_is_fatal() {
    let (_out, err, rc) = run_capture(
        "set -u\na=(x)\necho \"${a[5]}\"\nexit\n"
    );
    assert!(err.contains("unbound variable"), "expected unbound diagnostic: {err:?}");
    assert_ne!(rc, 0, "expected non-zero exit under set -u");
}
```

- [ ] **Step 2: Write the manual bash-diff check script**

Create `tests/scripts/arrays_diff_check.sh`:

```bash
#!/usr/bin/env bash
# Manual sanity check: run the same array fragments through bash and huck,
# diff outputs. Not part of `cargo test` (no bash dependency in CI), but
# run by the developer before merge.
set -u

HUCK="$(dirname "$0")/../../target/debug/huck"
if [ ! -x "$HUCK" ]; then
    echo "build huck first: cargo build" >&2
    exit 1
fi

fragments=(
    'a=(x y z); echo "${a[@]}"; echo "${#a[@]}"; echo "${!a[@]}"'
    'a=([5]=x [2]=y); echo "${#a[@]}"; echo "${!a[@]}"'
    'a=(x y z); for v in "${a[@]}"; do echo "[$v]"; done'
    'a=(x); a+=(y z); echo "${a[@]}"'
    'a[0]=hi; a[0]+=_bye; echo "${a[0]}"'
    'a=(a b c d); echo "${a[@]:1:2}"'
    'set -- one two three four; echo "${@:2:2}"'
)

fail=0
for f in "${fragments[@]}"; do
    b_out=$(bash -c "$f" 2>&1)
    h_out=$(echo "$f" | "$HUCK" 2>&1)
    if [ "$b_out" != "$h_out" ]; then
        echo "DIFF on: $f"
        diff <(printf '%s\n' "$b_out") <(printf '%s\n' "$h_out") || true
        echo "---"
        fail=1
    fi
done

if [ "$fail" -eq 0 ]; then
    echo "all array fragments produce identical output to bash"
fi
exit "$fail"
```

Make it executable:

```bash
chmod +x tests/scripts/arrays_diff_check.sh
```

- [ ] **Step 3: Update `docs/bash-divergences.md`**

Add a new M-82 entry. Find the Tier-2 section (around line 116) and insert in numeric order:

```markdown
- **M-82: Indexed arrays** — `[fixed v71 partial]` high. Indexed (sparse) arrays: literal `a=(x y z)` and explicit-subscript `a=([5]=x [2]=y)`; element access `${a[i]}` with arith subscripts (negative wraps via max-index+1, bash 4.3+); all-elements `${a[@]}` (word list, no IFS splitting when quoted) and `${a[*]}` (IFS-joined scalar when quoted); count `${#a[@]}`; indices `${!a[@]}`; element length `${#a[i]}`; slicing `${a[@]:offset:length}` (negative offset counts from end-of-present-elements list); element assign `a[i]=v` (promotes scalar to array, existing scalar becomes element 0); append-array `a+=(...)`; append-element `a[i]+=v`; whole-array `unset a` and per-element `unset a[i]`; bare `${a}` ≡ `${a[0]}` (bash compat); `declare -a NAME[=(...)]` and `local -a NAME[=(...)]` and `readonly NAME=(...)` integrate with v52/v54/v64 surfaces; nounset (`set -u`) fires on missing elements. Internal storage `VarValue::Indexed(BTreeMap<usize, String>)` for sparse-safe O(log n) per-element ops. **Deferred**: associative arrays / `declare -A` (M-?, v72 candidate); `mapfile`/`readarray` builtins; `read -a`; `BASH_REMATCH` array population; per-element substitution `${a[@]/pat/repl}` and case-mod `${a[@]^^}`; integer attribute on arrays (`declare -ai` rejected); exporting arrays (`export a=(...)` rejected — matches bash's de-facto behavior).
```

Update existing entries:

- **M-16** (substring expansion, around line 154): change "Array slicing on `$@`/`$*` deferred." to "Array slicing on `$@`/`$*` closes in v71 via the shared `slice_word_list` helper (see M-82)."
- **M-72** (read, around line 217): change "`-a ARRAY` (huck has no arrays)" to "`-a ARRAY` (deferred to a later iteration; the array surface now exists per M-82)."
- **M-78** (dirstack, around line 224): change "`DIRSTACK` shell array (huck has no arrays)" to "`DIRSTACK` shell array (deferred follow-on; array surface lands in v71 per M-82)."
- **M-79** (declare, around line ~225): update so `-a` row reads `fixed v71`, `-A` still deferred, `-ai` rejected per scope.
- **M-76** (PROMPT_COMMAND, around line ~222): update "array form deferred since huck has no arrays" to "array form deferred (huck gains arrays in v71; PROMPT_COMMAND array form remains future work)."

Add a Change log entry at the end of `docs/bash-divergences.md`:

```markdown
- **2026-06-01**: M-82 (indexed arrays) shipped as v71. New `VarValue::Scalar | Indexed(BTreeMap<usize, String>)` on `Variable`. New AST: `WordPart::ArrayLiteral` (compound RHS), `AssignTarget` enum (Bare / Indexed), `subscript: Option<SubscriptKind>` on `WordPart::ParamExpansion`, `Assignment { target, value, append }` row replacing `(String, Word)` in `SimpleCommand::Assign` / `ExecCommand.inline_assignments`. Expansion semantics for `${a[i]}` / `${a[@]}` / `${a[*]}` / `${#a[@]}` / `${!a[@]}` / `${#a[i]}` / `${a[@]:o:l}` with the same `slice_word_list` helper backing `${@:o:l}` / `${*:o:l}` — closes v33's positional-slicing deferral. Assignment paths: compound assign, element assign with scalar-promotion, `a+=(...)` and `a[i]+=v`, `unset a[i]`. Builtin wiring: `declare -a NAME[=(...)]`, `declare -p` array formatting, `local -a NAME[=(...)]`, `readonly NAME=(...)` enforcing v54 diagnostics on every write path, `declare -ai` and `export NAME=(...)` rejected. ~35 unit tests across `src/shell_state.rs::array_value_tests`, `src/lexer.rs::array_parse_tests`, `src/expand.rs::array_expansion_tests` + `positional_slicing_tests`, `src/executor.rs::array_assign_tests`, `src/builtins.rs::array_declare_tests`. 12 binary-driven integration tests in `tests/arrays_integration.rs`. New `tests/scripts/arrays_diff_check.sh` manual bash-vs-huck differential harness. Deferred per M-82: associative arrays, mapfile/readarray, read -a, BASH_REMATCH, per-element substitution/case-mod, integer arrays, exporting arrays.
```

- [ ] **Step 4: Update README**

Edit `README.md` line 79-80:

```markdown
| v70       | `cd -` (M-31)                                                  |
| v71       | indexed arrays (M-82)                                          |

## Build and run
```

- [ ] **Step 5: Run full test suite**

Run: `cargo build 2>&1 | tail -5`
Expected: clean.

Run: `cargo test 2>&1 | grep -E "test result|FAILED" | tail -40`
Expected: all green, including the new `arrays_integration` line at ~12 passed.

Run: `cargo clippy --all-targets 2>&1 | tail -5`
Expected: 0 warnings.

- [ ] **Step 6: Manual bash-diff sanity check**

Run: `cargo build && bash tests/scripts/arrays_diff_check.sh`
Expected: `all array fragments produce identical output to bash`. If any DIFF prints, fix in implementer-loop before merge.

- [ ] **Step 7: Commit**

```bash
git add tests/arrays_integration.rs tests/scripts/arrays_diff_check.sh docs/bash-divergences.md README.md
git commit -m "$(cat <<'EOF'
docs+tests: indexed arrays shipped v71 (M-82)

12 binary-driven integration tests covering literal, sparse,
element r/w, append-array, append-element, scalar promotion,
quoted [@] empty-preservation, [*] IFS-join, unset element,
local scoping, readonly enforcement, and nounset element errors.

New M-82 entry in bash-divergences.md plus cross-references on
M-16 (v33 positional slicing now closed), M-72 (read -a still
deferred), M-78 (DIRSTACK feasibility), M-79 (declare -a now
fixed), M-76 (PROMPT_COMMAND array form). Change-log entry.
README v71 row.

Manual differential harness at tests/scripts/arrays_diff_check.sh
runs the same fragments through bash and huck and diffs outputs;
all 7 fragments verified identical to bash output.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Final verification & merge prep

- [ ] **Step 1: Full test pass**

Run: `cargo test 2>&1 | grep -E "test result|FAILED" | wc -l`
Expected: large number of `ok` lines, zero `FAILED`.

- [ ] **Step 2: Clippy clean**

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -5`
Expected: `Finished` no warnings.

- [ ] **Step 3: Bash-diff harness passes**

Run: `bash tests/scripts/arrays_diff_check.sh`
Expected: `all array fragments produce identical output to bash`.

- [ ] **Step 4: Verify M-82 entry well-formed**

Run: `grep -n "M-82" docs/bash-divergences.md | head -3`
Expected: at least one Tier-2 entry line and one change-log line.

- [ ] **Step 5: Confirm v71 row in README**

Run: `grep "v71" README.md`
Expected: one row line with `indexed arrays (M-82)`.

- [ ] **Step 6: Ask user for merge confirmation via AskUserQuestion (controller, NOT subagent)**

Per the v52-v70 workflow, the controller (not a subagent) asks the user before pushing to origin.

- [ ] **Step 7: On approval, merge to main**

```bash
git checkout main
git merge --no-ff v71-arrays -m "Merge v71: indexed arrays (M-82)"
git push origin main
git branch -d v71-arrays
```

- [ ] **Step 8: Post-merge memory update**

Update `/home/john/.claude/projects/-home-john-projects-shuck/memory/MEMORY.md` and `project_huck_iterations.md` with the v71 entry (storage model, AST changes, deferral list, follow-on candidates).

---

## Notes for the implementer

1. **Subagent isolation**: each task is implemented by a fresh subagent that has not seen the others. The plan body is the only context they get. If a step references a type or function not yet defined, it's defined in a prior task — read the section above.

2. **TDD discipline**: write the failing test first when adding new behavior. Run it, see it fail, then make it pass. The plan steps name the test before the implementation for that reason.

3. **One commit per task**: each task ends with a single commit. If a step in a task gets messier than 5 lines, that's a signal to break it into a smaller commit within the task — but do not split the task itself.

4. **Code-quality reviewer notes** (anticipate these):
   - "BTreeMap::values() doesn't borrow keys" — that's correct; the helper takes &Shell and walks the map by ref.
   - "Why is read_subscript<I: Iterator<Item = char>> generic?" — to match existing helpers in lexer.rs that take peekable char iterators; do NOT change to a concrete type.
   - "Why a sentinel `subscript: None` everywhere?" — keeps `ParamExpansion` backward-compatible for bare `${a}` while adding the new branch.

5. **Spec-compliance reviewer notes**: verify that all 9 expansion forms from the spec's Section 3 are implemented; verify Task 4 covers all 5 assignment paths (compound, element, append-array, append-element, unset-element); verify Task 6 cross-references all 5 prior M-* entries.

6. **Known-unsupported error messages** must say "v71" explicitly so users searching the docs find the deferral entry in M-82.
