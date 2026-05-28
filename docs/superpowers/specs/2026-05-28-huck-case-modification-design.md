# huck v37 — `${var^^}` / `${var,,}` / `${var^}` / `${var,}` Case Modification Design

**Goal:** Close M-17 by implementing bash's case-modification parameter
expansion: `${var^^}` (upper-case all), `${var^}` (upper-case first
char), `${var,,}` (lower-case all), `${var,}` (lower-case first char),
plus the optional `pattern` operand (`${var^^[aeiou]}` etc.) that
filters which chars get modified.

**Why:** Last `[deferred]` Tier-2 item in the parameter-expansion
modifier family started by v32 (`${var/pat/repl}`), v33
(`${var:off:len}`), v34 (`${#1}` + fatal PE errors), v36 (trap pseudo-
signals — unrelated to this family but the same iteration cadence).
Closing M-17 finishes the family entirely; every `${var<modifier>}`
form bash supports will work in huck.

## Forms

| Syntax | Meaning |
|---|---|
| `${var^^}` | Upper-case every char in `$var`. |
| `${var^}` | Upper-case the first char only. |
| `${var,,}` | Lower-case every char. |
| `${var,}` | Lower-case the first char only. |
| `${var^^pattern}` | Upper-case every char that matches the glob `pattern`. |
| `${var^pattern}` | Upper-case the first char that matches `pattern`. |
| `${var,,pattern}` | Lower-case every char that matches `pattern`. |
| `${var,pattern}` | Lower-case the first char that matches `pattern`. |

`pattern` is a bash glob, evaluated by the existing `glob::Pattern`
crate (same engine as `${var/pat/repl}`, `${var#pat}`, `${var%pat}`).
Each character in `$var` is matched against the pattern individually
— so `${str^^[aeiou]}` upper-cases each vowel.

## Semantics

Unicode-aware case conversion via Rust's `char::to_uppercase()` /
`char::to_lowercase()`. These return iterators that handle multi-char
expansions (e.g. `'ß'.to_uppercase()` yields `'S'`,`'S'`) — matches
bash's behavior in UTF-8 locale.

- If `$var` is unset or empty, the result is empty (same as the
  existing remove-prefix/suffix/substitute behavior on unset).
- If `pattern` fails to compile as a glob, return `$var` unchanged.
  Matches v32's `substitute` and v33's `substring` conventions.
- `pattern` is a `Word`, expanded via `expand_word_to_string` before
  glob compilation. So `${var^^$letters}` works.
- For the "all" forms (`^^` / `,,`): walk every char in `$var`. If
  the char matches `pattern` (or `pattern` is None), apply case
  conversion; otherwise pass through unchanged.
- For the "first" forms (`^` / `,`): walk chars in order; for the
  FIRST char that matches `pattern` (or any char if `pattern` is
  None), apply case conversion; then pass the rest through unchanged.
  If no char matches, return `$var` unchanged.
- Empty pattern operand (`${var^^}` with no chars between `^^` and
  `}`) is the same as "no pattern" — every char matches.

### Worked examples

| Input | Modifier | Result |
|---|---|---|
| `hello` | `^^` | `HELLO` |
| `hello` | `^` | `Hello` |
| `HELLO` | `,,` | `hello` |
| `HELLO` | `,` | `hELLO` |
| `hello world` | `^^[aeiou]` | `hEllO wOrld` |
| `hello` | `^[aeiou]` | `hEllo` (only first matching char) |
| `xyz` | `^[aeiou]` | `xyz` (no chars match → unchanged) |
| `café` | `^^` | `CAFÉ` (Unicode-aware) |
| `straße` | `^^` | `STRASSE` (Rust `'ß'.to_uppercase()` yields `'S'`,`'S'`) |
| `` | `^^` | `` |
| `${unset^^}` | (unset var) | `` |
| `hello` | `^^[abc` (malformed) | `hello` (silent unchanged) |

## AST

New variant in `ParamModifier` (`src/lexer.rs`, alongside `Substitute`):

```rust
ParamModifier::Case {
    direction: CaseDirection,
    all: bool,                // true for ^^ / ,, ; false for ^ / ,
    pattern: Option<Word>,    // None = match every char
}
```

New enum:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseDirection {
    Upper,  // ^ / ^^
    Lower,  // , / ,,
}
```

Shape matches v32's `Substitute { anchor, all, ... }` convention.
`pattern: Option<Word>` carries `None` when the operand is empty (no
filter) — distinct from `Some(Word(vec![]))` which would be a Word
containing zero parts and parsed as a literal-empty pattern.

## Lexer

Two new arms in `dispatch_braced_modifier` (`src/lexer.rs:1114`),
slotted after the existing `Some('/')` arm at line 1198:

```rust
Some('^') => {
    let all = chars.peek() == Some(&'^');
    if all { chars.next(); }
    let pattern = scan_optional_braced_operand(chars)?;
    parts.push(WordPart::ParamExpansion {
        name,
        modifier: ParamModifier::Case { direction: CaseDirection::Upper, all, pattern },
        quoted,
    });
    Ok(())
}
Some(',') => {
    let all = chars.peek() == Some(&',');
    if all { chars.next(); }
    let pattern = scan_optional_braced_operand(chars)?;
    parts.push(WordPart::ParamExpansion {
        name,
        modifier: ParamModifier::Case { direction: CaseDirection::Lower, all, pattern },
        quoted,
    });
    Ok(())
}
```

New helper `scan_optional_braced_operand` reuses
`scan_braced_operand` + `parse_braced_operand` (same as v32's
substitute operand scan):

```rust
fn scan_optional_braced_operand(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
) -> Result<Option<Word>, LexError> {
    let body = scan_braced_operand(chars)?;
    if body.is_empty() {
        Ok(None)
    } else {
        Ok(Some(parse_braced_operand(&body)?))
    }
}
```

The empty-body check is what distinguishes `${var^^}` (no pattern)
from `${var^^x}` (pattern = "x"). `scan_braced_operand` consumes
through the closing `}` regardless.

## Evaluator

### `case_modify` helper

Pure-function helper in `src/param_expansion.rs`:

```rust
fn case_modify(
    value: &str,
    direction: CaseDirection,
    all: bool,
    pattern: Option<&str>,
) -> String
```

Algorithm:

1. Compile pattern if `Some(non_empty)` via `glob::Pattern::new`. On
   compile failure, return `value.to_string()` (unchanged).

2. `should_modify(c: char) -> bool`:
   - `pattern.is_none()` → `true`.
   - Else → `pat.matches_with(&c.to_string(), opts)` with the same
     `MatchOptions` used elsewhere in the file (case-sensitive,
     `require_literal_separator: false`, `require_literal_leading_dot:
     false`).

3. `apply(c: char) -> impl Iterator<Item = char>`:
   - `Upper` → `c.to_uppercase()` (Rust's Unicode-aware iterator).
   - `Lower` → `c.to_lowercase()`.

4. Build output:
   - `all = true`: for each char `c`, extend output with `apply(c)` if
     `should_modify(c)`, else push `c` verbatim.
   - `all = false`: track a `done: bool`. For each char `c`, if
     `!done && should_modify(c)` → extend with `apply(c)`, set
     `done = true`; else push `c` verbatim. If `done` is still false
     at end (no char matched the pattern), the loop already emitted
     `value` verbatim — return as-is.

### `Case` arm in `expand_modifier`

```rust
ParamModifier::Case { direction, all, pattern } => {
    let v = shell.lookup_var(name).unwrap_or_default();
    let pat_string = pattern.as_ref().map(|w| expand_word_to_string(w, shell));
    ExpansionResult::Value(case_modify(&v, *direction, *all, pat_string.as_deref()))
}
```

Uses `shell.lookup_var` (v33's switch from `shell.get`) so digit
names and special params resolve correctly through `positional_args`.

## Error handling

| Condition | Behavior |
|---|---|
| Unset `$var` | Result = empty string (matches existing modifiers). |
| Empty `$var` | Result = empty string. |
| `pattern` fails to compile as glob | Return `$var` unchanged (matches v32/v33 conventions). |
| No char in `$var` matches `pattern` (first-form) | Return `$var` unchanged. |
| Unterminated brace (`${var^^`) | `LexError::UnterminatedBrace` at lex time. |
| `${var^x` (no closing brace) | `LexError::UnterminatedBrace`. |

No new fatal-error paths. Case modification can't fail in a way that
should abort the shell — silent fallthrough on glob compile errors
matches the family convention.

## Scope (in)

- All four bare forms: `${var^^}`, `${var^}`, `${var,,}`, `${var,}`.
- All four pattern forms: `${var^^pat}`, `${var^pat}`, `${var,,pat}`,
  `${var,pat}`.
- Pattern is a `Word` (full expansion: `$var`, `${var}`, `$(cmd)`,
  etc.). Same shape as v32's substitute pattern operand.
- Unicode-aware case mapping via Rust's `char::to_uppercase` /
  `char::to_lowercase` iterators.
- Positional params: `${1^^}` etc. work (digit-only brace path from
  v33).
- Quoted contexts: `"${var^^}"` produces a single field — no
  word-splitting on the result (same as every other `${…}` modifier).
- Pipeline / heredoc / subshell composition — all inherited for free
  from the `ParamExpansion` codepath.

## Scope (out)

- **Locale-aware case mapping** — Rust's `char::to_uppercase` is
  Unicode-based, locale-independent. Bash with non-UTF-8 locales may
  differ.
- **`declare -u VAR` / `declare -l VAR`** — variables that
  auto-upper/lower on assignment. Separate concern; deferred.

## Testing

### Lexer unit tests (`src/lexer.rs` tests module, ~7 tests)

- `brace_case_upper_all` — `${name^^}` → `Case { Upper, all: true, pattern: None }`.
- `brace_case_upper_first` — `${name^}` → `Case { Upper, all: false, pattern: None }`.
- `brace_case_lower_all` — `${name,,}` → `Case { Lower, all: true, pattern: None }`.
- `brace_case_lower_first` — `${name,}` → `Case { Lower, all: false, pattern: None }`.
- `brace_case_upper_all_with_pattern` — `${name^^[aeiou]}` → `pattern: Some(...)` with literal `[aeiou]`.
- `brace_case_positional` — `${1^^}` → `ParamExpansion` (not `Var`).
- `brace_case_unterminated_is_error` — `${name^^` → `UnterminatedBrace`.

### Evaluator unit tests (`src/param_expansion.rs` tests module, ~13 tests)

`case_modify` helper (~10):
- `case_modify_upper_all_no_pattern`.
- `case_modify_upper_first_no_pattern`.
- `case_modify_lower_all_no_pattern`.
- `case_modify_lower_first_no_pattern`.
- `case_modify_upper_all_with_pattern_filters_chars`.
- `case_modify_upper_first_with_pattern_picks_first_match`.
- `case_modify_unicode_handles_multichar_uppercase` — `"straße"` →
  `"STRASSE"`.
- `case_modify_empty_value_returns_empty`.
- `case_modify_invalid_glob_returns_value_unchanged` — `"[abc"` (unclosed).
- `case_modify_no_match_first_form_returns_unchanged` — `"hello"` +
  `[xyz]` + `all=false` → `"hello"`.

`expand_modifier` Case arm (~3):
- `expand_modifier_case_upper_all_named_var`.
- `expand_modifier_case_upper_positional_lookup` — uses `lookup_var`.
- `expand_modifier_case_unset_var_returns_empty`.

### Integration tests (new file `tests/param_case_integration.rs`, ~9 tests)

- `case_upper_all_basic` — `s=hello; echo ${s^^}` → `HELLO`.
- `case_upper_first_basic` — `s=hello; echo ${s^}` → `Hello`.
- `case_lower_all_basic` — `s=HELLO; echo ${s,,}` → `hello`.
- `case_lower_first_basic` — `s=HELLO; echo ${s,}` → `hELLO`.
- `case_upper_with_pattern_filters` — `s=hello; echo ${s^^[aeiou]}` →
  `hEllO`.
- `case_upper_unicode` — `s=café; echo ${s^^}` → `CAFÉ`.
- `case_pattern_uses_other_var` — `s=hello; p=[ae]; echo ${s^^$p}` →
  `hEllo`.
- `case_in_function_with_positional` — `f() { echo ${1^^}; }; f hello`
  → `HELLO`.
- `case_in_pipeline_stage` — `s=hello; echo ${s^^} | cat` → `HELLO`.

**Total new tests**: ~29. Baseline goes from 1397 → ~1426.

## Documentation

- `docs/bash-divergences.md`:
  - M-17 status flips from `[deferred]` to `[fixed v37]` with notes
    on all four forms + pattern operand + Unicode case mapping.
  - L-04 sub-bullet extended to mention `${var^^}` / `${var,,}` use
    Rust's `char::to_uppercase` / `char::to_lowercase` (multi-char
    expansions like `ß`→`SS` handled correctly).
  - Changelog row.
- `README.md`: new v37 row in the version table.

## Implementation tasks (handoff to writing-plans)

| # | Task | Notes |
|---|---|---|
| 1 | AST scaffold: `CaseDirection` enum + `Case` variant + placeholder evaluator arm | Compile-clean baseline. |
| 2 | Lexer: `^`/`,` arms + `scan_optional_braced_operand` helper + 7 lexer unit tests | TDD: write tests first. |
| 3 | Evaluator: `case_modify()` helper + 10 unit tests | Pure-function tests. |
| 4 | Evaluator: wire `Case` arm into `expand_modifier` + 3 through-the-arm tests | Connects modifier to Shell. |
| 5 | Integration tests (binary-driven, 9 tests) | Same harness as v33/v34/v36. |
| 6 | Docs (M-17 → fixed v37 + L-04 + changelog + README) + full-suite verify | Mechanical close-out. |

Process: subagent-driven per `[[huck-iteration-workflow]]` on
`v37-case-modification` branch. Final code-reviewer pass over the
whole branch diff before `merge --no-ff` into `main`.
