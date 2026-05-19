# huck v12: Parameter-Expansion Modifiers

**Date:** 2026-05-19
**Status:** Design

## Goal

Extend brace-form parameter expansion (`${var}`) to support the
default-value family, length, and prefix/suffix removal modifiers.
The result is substituted into the surrounding word just like a plain
`${var}` reference today.

## Scope

**In scope (13 modifiers):**
- Default-value family: `${var:-w}` / `${var-w}` / `${var:=w}` /
  `${var=w}` / `${var:?w}` / `${var?w}` / `${var:+w}` / `${var+w}`
- Length: `${#var}`
- Prefix/suffix removal: `${var#pat}` / `${var##pat}` / `${var%pat}` /
  `${var%%pat}`

**Semantics:**
- `:` variants treat both unset AND empty as null; non-`:` variants
  treat only unset as null
- The operand `w` (or `pat`) is recursively expanded — supports
  variables, arithmetic, command substitution, and tilde
- `${var:=}` / `${var=}` mutate the shell variable
- `${var:?}` / `${var?}` print to stderr, set `$?` to 1, and yield
  an empty expansion (the command still runs)
- Patterns use glob syntax via the `glob` crate, with
  `require_literal_separator: false` so `*` can cross `/`
- `${#var}` returns the Unicode character count (not byte count)

**Out of scope (deferred):**
- Pattern substitution `${var/pat/repl}` / `${var//pat/repl}`
- Substring `${var:offset:length}`
- Case modification `${var^^}` / `${var,,}` (bash 4+)
- Special parameters `$0` / `$#` / `$@` / `$$` / `$!` (separate
  iteration)
- Recursive `:?` error abort in non-interactive mode (bash aborts
  the script; we only print and set status)

## Architecture

A new `WordPart::ParamExpansion` variant carries a parameter name and
a `ParamModifier` enum. The lexer detects modifier syntax inside
`${...}` and parses the operand (a `Word`) using a recursive
`tokenize` call (same pattern as `parse_substitution_body` today).
Plain `${var}` (no modifier) keeps producing `WordPart::Var`, so
nothing existing changes.

A new `src/param_expansion.rs` module owns the evaluation: looking up
the variable, applying the modifier semantics, calling back into
`expand_assignment` for recursive operand expansion, and (for the
removal modifiers) running glob pattern matching against prefixes or
suffixes of the value.

### AST

```rust
pub enum WordPart {
    // ... existing variants ...
    ParamExpansion {
        name: String,
        modifier: ParamModifier,
        quoted: bool,
    },
}

pub enum ParamModifier {
    Length,                                              // ${#var}
    UseDefault    { word: Word, colon: bool },           // :- / -
    AssignDefault { word: Word, colon: bool },           // := / =
    ErrorIfUnset  { word: Word, colon: bool },           // :? / ?
    UseAlternate  { word: Word, colon: bool },           // :+ / +
    RemovePrefix  { pattern: Word, longest: bool },      // # / ##
    RemoveSuffix  { pattern: Word, longest: bool },      // % / %%
}
```

### Lexer

Today's `read_braced_var_name` (`src/lexer.rs:288`) reads identifier
chars and consumes a `}`. Replace it with `read_braced_param_expansion`,
which:

1. **Length form:** if the first char inside `{` is `#` followed by an
   identifier start char, parse as `${#name}` (consume `#`, read
   name, expect `}`).
2. **Read the name** (identifier chars).
3. **Inspect the next char:**
   - `}` → no modifier; emit `WordPart::Var { name, quoted }` (existing
     behavior — backward compatible).
   - `:` → consume; next char must be one of `-=?+`; build the
     corresponding modifier with `colon: true`.
   - `-` / `=` / `?` / `+` → build with `colon: false`.
   - `#` → check for a second `#`; build `RemovePrefix { longest }`.
   - `%` → check for a second `%`; build `RemoveSuffix { longest }`.
   - anything else → `LexError::InvalidBraceModifier(char)`.
4. **Scan the operand text** with a new helper `scan_braced_operand`
   that consumes until the matching `}`, tracking nested `{...}`
   depth and respecting `'...'` and `"..."` so a stray `}` inside a
   quoted span doesn't close the expansion.
5. **Recursively tokenize** the operand text as a `Word`, mirroring
   `parse_substitution_body` at `src/lexer.rs:392`. The resulting
   parts go into the `word` or `pattern` field of the modifier.
6. **Empty name guard:** `${:-foo}`, `${#}` → `LexError::EmptyParamName`.

#### `scan_braced_operand`

```rust
fn scan_braced_operand(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
) -> Result<String, LexError> {
    let mut body = String::new();
    let mut depth: u32 = 1; // already inside the outer `{`
    loop {
        match chars.next() {
            None => return Err(LexError::UnterminatedBrace),
            Some('\\') => {
                body.push('\\');
                if let Some(c) = chars.next() { body.push(c); }
            }
            Some('"') => {
                body.push('"');
                loop {
                    match chars.next() {
                        None => return Err(LexError::UnterminatedBrace),
                        Some('"') => { body.push('"'); break; }
                        Some('\\') => {
                            body.push('\\');
                            if let Some(c) = chars.next() { body.push(c); }
                        }
                        Some(c) => body.push(c),
                    }
                }
            }
            Some('\'') => {
                body.push('\'');
                loop {
                    match chars.next() {
                        None => return Err(LexError::UnterminatedBrace),
                        Some('\'') => { body.push('\''); break; }
                        Some(c) => body.push(c),
                    }
                }
            }
            Some('{') => { depth += 1; body.push('{'); }
            Some('}') => {
                if depth == 1 { return Ok(body); }
                depth -= 1;
                body.push('}');
            }
            Some(c) => body.push(c),
        }
    }
}
```

The operand text then passes through `tokenize`. Operands can contain
whitespace (`${X:-foo bar}`) which `tokenize` treats as a word
separator, so the result is `Vec<Token>` with potentially several
`Token::Word` items. We merge those back into a single `Word` by
walking the tokens and:

- For each `Token::Word(Word(parts))`, append its parts.
- Between consecutive `Token::Word`s, insert a single
  `WordPart::Literal { text: " ", quoted: false }` to preserve the
  whitespace (which becomes split-relevant if the outer
  `ParamExpansion` is unquoted).
- Any `Token::Op` at the operand top level is a syntax error:
  `LexError::InvalidBraceOperand`.

Helper sketch:

```rust
fn parse_braced_operand(body: &str) -> Result<Word, LexError> {
    let tokens = tokenize(body).map_err(|e|
        LexError::SubstitutionLexError(Box::new(e)))?;
    let mut parts: Vec<WordPart> = Vec::new();
    let mut first = true;
    for tok in tokens {
        match tok {
            Token::Word(Word(ps)) => {
                if !first {
                    parts.push(WordPart::Literal {
                        text: " ".to_string(),
                        quoted: false,
                    });
                }
                parts.extend(ps);
                first = false;
            }
            Token::Op(_) => return Err(LexError::InvalidBraceOperand),
        }
    }
    Ok(Word(parts))
}
```

This keeps a single uniform operand type (`Word`) for every modifier
that takes one, and defers IFS splitting to expand time where it's
governed by the outer `quoted` flag.

#### New `LexError` variants

```rust
pub enum LexError {
    // ... existing ...
    InvalidBraceModifier(String),  // includes the offending char(s) for diagnostics
    EmptyParamName,
    InvalidBraceOperand,           // operand contained a top-level operator
}
```

`UnterminatedBrace` already exists; reuse it for `${X:-w` with no
closing `}`.

### Evaluator

New module `src/param_expansion.rs`:

```rust
pub enum ExpansionResult {
    Value(String),  // push these chars (with the outer quoted flag)
    Empty,          // no chars; emit an empty field
}

pub fn expand_modifier(
    name: &str,
    modifier: &ParamModifier,
    shell: &mut Shell,
) -> ExpansionResult {
    match modifier {
        ParamModifier::Length => {
            let v = shell.get(name).unwrap_or("");
            ExpansionResult::Value(v.chars().count().to_string())
        }

        ParamModifier::UseDefault { word, colon } => {
            let raw = shell.get(name).map(|s| s.to_string());
            if condition_is_null(raw.as_deref(), *colon) {
                ExpansionResult::Value(expand_word_to_string(word, shell))
            } else {
                ExpansionResult::Value(raw.unwrap_or_default())
            }
        }

        ParamModifier::AssignDefault { word, colon } => {
            let raw = shell.get(name).map(|s| s.to_string());
            if condition_is_null(raw.as_deref(), *colon) {
                let v = expand_word_to_string(word, shell);
                shell.set(name, v.clone());
                ExpansionResult::Value(v)
            } else {
                ExpansionResult::Value(raw.unwrap_or_default())
            }
        }

        ParamModifier::ErrorIfUnset { word, colon } => {
            let raw = shell.get(name).map(|s| s.to_string());
            if condition_is_null(raw.as_deref(), *colon) {
                let msg = expand_word_to_string(word, shell);
                if msg.is_empty() {
                    let default = if *colon { "parameter null or not set" }
                                  else      { "parameter not set" };
                    eprintln!("huck: {}: {}", name, default);
                } else {
                    eprintln!("huck: {}: {}", name, msg);
                }
                shell.set_last_status(1);
                ExpansionResult::Empty
            } else {
                ExpansionResult::Value(raw.unwrap_or_default())
            }
        }

        ParamModifier::UseAlternate { word, colon } => {
            let raw = shell.get(name);
            if condition_is_null(raw, *colon) {
                ExpansionResult::Empty
            } else {
                ExpansionResult::Value(expand_word_to_string(word, shell))
            }
        }

        ParamModifier::RemovePrefix { pattern, longest } => {
            let v = shell.get(name).unwrap_or("").to_string();
            let p = expand_word_to_string(pattern, shell);
            ExpansionResult::Value(remove_prefix(&v, &p, *longest))
        }

        ParamModifier::RemoveSuffix { pattern, longest } => {
            let v = shell.get(name).unwrap_or("").to_string();
            let p = expand_word_to_string(pattern, shell);
            ExpansionResult::Value(remove_suffix(&v, &p, *longest))
        }
    }
}

fn condition_is_null(raw: Option<&str>, colon: bool) -> bool {
    match (raw, colon) {
        (None, _) => true,
        (Some(""), true) => true,
        (Some(_), _) => false,
    }
}

fn expand_word_to_string(word: &Word, shell: &mut Shell) -> String {
    // `expand_assignment` already concatenates a Word into a single
    // String without IFS splitting — exactly what we want for an
    // operand here.
    crate::expand::expand_assignment(word, shell)
}
```

### Pattern matching for prefix/suffix

```rust
fn remove_prefix(value: &str, pattern: &str, longest: bool) -> String {
    let opts = glob::MatchOptions {
        case_sensitive: true,
        require_literal_separator: false,
        require_literal_leading_dot: false,
    };
    let pat = match glob::Pattern::new(pattern) {
        Ok(p) => p,
        Err(_) => return value.to_string(), // invalid pattern → no-op
    };
    let mut boundaries: Vec<usize> = value
        .char_indices()
        .map(|(i, _)| i)
        .collect();
    boundaries.push(value.len());

    if longest {
        // Try longest prefix first, shrink toward empty.
        for &end in boundaries.iter().rev() {
            if pat.matches_with(&value[..end], opts) {
                return value[end..].to_string();
            }
        }
    } else {
        // Try shortest prefix first, grow.
        for &end in &boundaries {
            if pat.matches_with(&value[..end], opts) {
                return value[end..].to_string();
            }
        }
    }
    value.to_string()
}

fn remove_suffix(value: &str, pattern: &str, longest: bool) -> String {
    let opts = glob::MatchOptions {
        case_sensitive: true,
        require_literal_separator: false,
        require_literal_leading_dot: false,
    };
    let pat = match glob::Pattern::new(pattern) {
        Ok(p) => p,
        Err(_) => return value.to_string(),
    };
    let mut boundaries: Vec<usize> = value
        .char_indices()
        .map(|(i, _)| i)
        .collect();
    boundaries.push(value.len());

    if longest {
        // Try longest suffix first: start at index 0 (whole string).
        for &start in &boundaries {
            if pat.matches_with(&value[start..], opts) {
                return value[..start].to_string();
            }
        }
    } else {
        // Try shortest suffix first: start at the end.
        for &start in boundaries.iter().rev() {
            if pat.matches_with(&value[start..], opts) {
                return value[..start].to_string();
            }
        }
    }
    value.to_string()
}
```

The `boundaries` vector ensures we never slice between UTF-8 char
bytes. The empty prefix/suffix (boundary at 0 / `value.len()`) is
included — bash's empty-match semantics: `${var#}` returns `value`
unchanged (the empty pattern matches the empty prefix, which removes
nothing).

### Expand integration

In `src/expand.rs`, add an arm to the per-WordPart match in `expand`:

```rust
WordPart::ParamExpansion { name, modifier, quoted } => {
    match crate::param_expansion::expand_modifier(name, modifier, shell) {
        crate::param_expansion::ExpansionResult::Value(v) => {
            if *quoted {
                current.push_str(&v, true);
                has_emitted = true;
            } else {
                emit_split_fields(&v, &mut current, &mut result, &mut has_emitted);
            }
        }
        crate::param_expansion::ExpansionResult::Empty => {
            has_emitted = true; // produce one empty field if no other parts
        }
    }
}
```

Add a parallel arm in `expand_assignment` (no IFS splitting):

```rust
WordPart::ParamExpansion { name, modifier, .. } => {
    match crate::param_expansion::expand_modifier(name, modifier, shell) {
        crate::param_expansion::ExpansionResult::Value(v) => result.push_str(&v),
        crate::param_expansion::ExpansionResult::Empty => {}
    }
}
```

### Pattern-matcher sites

- `command.rs::word_is_identifier_so_far`: `ParamExpansion` parts are
  not identifier characters; the existing `if let WordPart::Literal`
  pattern naturally rejects them. No change needed.
- `command.rs::try_split_assignment`: same — only inspects the first
  Literal.
- `executor.rs::pipeline_is_pure_builtin`: same pattern; no change.
- `shell.rs::lex_error_message`: add arms for the three new variants
  (`InvalidBraceModifier`, `EmptyParamName`, `InvalidBraceOperand`).

## Data flow examples

`echo ${X:-default}` when X is unset:

1. Lex: one Word with one `ParamExpansion { name: "X", modifier:
   UseDefault { word: Word(["default"]), colon: true }, quoted: false }`.
2. Expand: shell.get("X") = None → condition_is_null returns true →
   expand the operand Word → "default". Push "default" with IFS
   splitting (no whitespace → one field).
3. Argv: `["echo", "default"]`.

`f=/path/to/file.txt; echo ${f##*/}`:

1. After the assignment, `f` = "/path/to/file.txt".
2. Lex `${f##*/}` → `ParamExpansion { name: "f", modifier:
   RemovePrefix { pattern: Word(["*/"]), longest: true } }`.
3. Expand: shell.get("f") = "/path/to/file.txt"; pattern expansion =
   "*/"; remove_prefix tries longest prefix first. The longest prefix
   matching `*/` is "/path/to/"; the remainder is "file.txt".
4. Argv: `["echo", "file.txt"]`.

`echo ${X:=default}; echo $X` when X is unset:

1. First echo: `ParamExpansion { ..., AssignDefault {...} }`. shell.set(X,
   "default") fires. Returns "default".
2. Second echo: `Var { name: "X" }`. shell.get(X) = "default".
3. Stdout: `default\ndefault`.

`echo ${UNSET:?missing}`:

1. shell.get("UNSET") = None. ErrorIfUnset fires.
2. Operand "missing" expanded; stderr: `huck: UNSET: missing`. shell
   last_status = 1. Result is Empty.
3. Expand: argv has one empty field. echo runs with empty arg.

`echo "${X:-a b c}"` when X is unset:

1. Quoted ParamExpansion. UseDefault returns "a b c". Outer
   `quoted: true` → push_str("a b c", true). No IFS splitting (because
   quoted).
2. Argv: `["echo", "a b c"]` (one element).

## Error handling summary

| Condition | Result |
|---|---|
| `${X:&Y}` | `LexError::InvalidBraceModifier("&")` at command parse |
| `${:-foo}` | `LexError::EmptyParamName` at command parse |
| `${X:-foo \| bar}` | `LexError::InvalidBraceOperand` (operand contained a top-level operator) |
| `${X` (no closing `}`) | `LexError::UnterminatedBrace` |
| `${X:?w}` with X unset/null | stderr `huck: X: w` (or default text if w empty); status=1; empty expansion |
| `${X##[}` (invalid glob) | Pattern matches nothing → value returned unchanged |
| Operand expansion fails (e.g. inner `$((1/0))`) | Inner error already prints; operand becomes whatever inner expansion produced (likely empty); modifier proceeds normally |

## Testing

**`src/param_expansion.rs` unit tests:**
- `condition_is_null` table coverage (None, Some(""), Some("x") × colon true/false)
- `remove_prefix` shortest vs longest with multiple match candidates
- `remove_suffix` shortest vs longest
- Empty pattern → no removal
- Glob `*` crossing `/`
- Invalid glob pattern → no-op
- UTF-8 boundary correctness (test value with multi-byte chars)
- Each modifier via `expand_modifier`:
  - Length on set/unset/empty
  - UseDefault, AssignDefault (verify shell.set), ErrorIfUnset (verify last_status), UseAlternate
  - Colon vs non-colon distinction for empty value
  - RemovePrefix / RemoveSuffix end-to-end

**`src/lexer.rs` tests:**
- Each modifier syntax → correct `ParamModifier` shape
- `${#name}` → Length
- `${name:-x}` and `${name-x}` distinguished by `colon`
- Nested `${X:-${Y}}` parses correctly (operand contains another ParamExpansion)
- Quoted operand: `${X:-"hello world"}` — operand Word has a quoted Literal
- `${X:&Y}` → InvalidBraceModifier
- `${:-foo}` → EmptyParamName
- `${X:-foo` → UnterminatedBrace
- `${name}` (no modifier) still produces `WordPart::Var` (regression)

**`src/expand.rs` tests:**
- ParamExpansion arm with quoted: true / false produces right field count
- IFS splitting on unquoted UseDefault result
- Single-field result for quoted UseDefault with spaces

**Integration tests (`tests/param_expansion_integration.rs`):**
- `echo ${X:-default}` (X unset) → `default`
- `X=value; echo ${X:-default}` → `value`
- `echo ${X:=default}; echo $X` → `default\ndefault`
- `echo ${UNSET:?missing}` → stderr contains `huck: UNSET: missing`; status check
- `f=/path/to/file.txt; echo ${f##*/}` → `file.txt`
- `f=/path/to/file.txt; echo ${f%.*}` → `/path/to/file`
- `s=hello; echo ${#s}` → `5`
- `echo "${X:-$(date +%Y)}"` — recursive command sub inside default
- `X=hello; echo ${X:+set}` → `set`
- `echo ${X:+set}` (X unset) → empty line

## File layout impact

- **New:** `src/param_expansion.rs` (~250 lines including tests)
- **New:** `tests/param_expansion_integration.rs`
- **Modify:** `src/lexer.rs` — replace `read_braced_var_name` with
  `read_braced_param_expansion`; add `scan_braced_operand`; add
  `WordPart::ParamExpansion` variant; add two `LexError` variants;
  update tests
- **Modify:** `src/expand.rs` — new `ParamExpansion` arm in `expand`
  and `expand_assignment`
- **Modify:** `src/shell.rs` — `lex_error_message` arms for
  `InvalidBraceModifier` and `EmptyParamName`
- **Modify:** `src/main.rs` — register `mod param_expansion`
- **Modify:** `Cargo.toml` — no new dependencies (reuses `glob`)
- **Modify:** `README.md` — v12 row, features section, test count

## Open questions

None at design time.

## References

- POSIX 2008 Shell Command Language §2.6.2 Parameter Expansion
- bash(1) Shell Parameter Expansion section
- `glob` crate `Pattern::matches_with` docs
