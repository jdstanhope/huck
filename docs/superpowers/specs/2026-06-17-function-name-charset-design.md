# v175: permissive function-name charset (bash-compatible) — Design

**Status:** approved 2026-06-17
**Iteration:** v175
**Origin:** Second cluster from the parse-compat sweep (`tools/parse_sweep.sh`): 11
real scripts (fzf `key-bindings.bash` `fzf-file-widget()`, completion functions,
etc.) rejected by `huck -n` with "syntax error: invalid function name" — all using
function names containing characters bash allows but huck does not.

## Problem

Function-name validation reuses the strict POSIX identifier validator
`valid_identifier_text` (`src/command.rs:1326`), which requires the name to be a
single unquoted `Literal` whose first char is `_`/alpha and whose remaining chars
are `_`/alphanumeric. That validator is **shared** with for-loop variable names
(`for_variable_name`, `command.rs:1355`) — correct for loop variables (bash also
rejects `for a-b in …`), but wrong for function names: bash accepts `foo-bar`,
`foo.bar`, `foo:bar`, `a+b`, `2foo`, and many more.

Confirmed against bash (all accepted by bash, all rejected by huck today):
`foo-bar() { :; }`, `foo.bar() { :; }`, `foo:bar() { :; }`, `a+b() { :; }`,
`2foo() { :; }`, `function foo-bar { :; }`. (The `function NAME ()` combo and
plain identifier names already work in huck — the only defect is the charset.)

## Goal

Accept bash's function-name set so real scripts (and bash-completion-style
`_pkg-cmd` functions, framework hooks like `fzf-file-widget`) parse, without
loosening for-loop variable validation.

## Design

Add a dedicated function-name validator and use it at the two function-definition
name sites; leave the identifier validator and the loop-variable path untouched.

```rust
/// Returns the function name if `word` is a single, unquoted, non-empty
/// `Literal` that is not a reserved keyword. Unlike `valid_identifier_text`,
/// this does NOT restrict the character set: bash accepts almost any single word
/// as a function name (`foo-bar`, `a.b`, `2foo`, …), and the lexer already
/// guarantees a single `Literal` contains no metacharacters or whitespace, so the
/// trailing `()` (or the `function` keyword) — not the name's spelling — is what
/// makes it a definition.
fn valid_function_name_text(word: &Word) -> Option<String> {
    if word.0.len() != 1 {
        return None;
    }
    let WordPart::Literal { text, quoted: false } = &word.0[0] else {
        return None;
    };
    if text.is_empty() {
        return None;
    }
    // Reject reserved keywords (bash: `if() { :; }` is a syntax error).
    let tok = Token::Word(Word(vec![WordPart::Literal {
        text: text.clone(),
        quoted: false,
    }]));
    if keyword_of(&tok).is_some() {
        return None;
    }
    Some(text.clone())
}
```

Swap the two call sites:
- `parse_function_def` (`command.rs:1183`): `valid_identifier_text(&name_word)` →
  `valid_function_name_text(&name_word)`.
- `parse_function_keyword_def` (`command.rs:1218`): same swap.

`valid_identifier_text` (still used by `for_variable_name`) and every for-loop
path are unchanged.

### Why fully permissive (rather than an extended-but-finite charset)

The function-definition parse only fires when a `Word` is followed by `()` (the
`name()` form) or after the `function` keyword — the `()` is the real trigger. The
name arrives as a single unquoted `Literal`, which the lexer already produced by
splitting on metacharacters/whitespace/quotes, so any such literal is a
well-formed "word". Accepting any non-keyword single literal therefore matches
bash for every realistic name and avoids enumerating bash's fuzzy set
(`@ ? [ ] ^ , …`). Marginal over-leniency on an exotic name (e.g. `foo=bar()`) is
harmless — the sweep found **0** cases where huck was wrongly more lenient than
bash, and a too-lenient function name never mis-parses other code (the `()` gates
it). The two retained guards — single unquoted literal, and not-a-keyword — keep
huck matching bash on the cases that matter (`if() {…}` stays a syntax error).

### Behavior

- `foo-bar() { :; }`, `foo.bar`, `a+b`, `2foo`, `function fzf-file-widget()` →
  parse, define, and are invocable like bash.
- Unchanged: plain identifier functions; `if() {…}`/`while() {…}` rejected (both
  shells); `for a-b in …` rejected (both shells); quoted/multi-part names rejected.

## Verification

- **New bash-diff harness** `tests/scripts/function_name_diff_check.sh`: define
  AND call functions whose names use `-`, `.`, `:`, `+`, and a leading digit,
  across all three definition forms (`f() { …; }`, `function f { …; }`,
  `function f() { …; }`); assert byte-identical stdout+exit vs bash. Regression
  cases: `if() { :; }` (both error), `for a-b in 1; do :; done` (both error),
  plain `foo_bar` (both OK).
- **Parse-sweep payoff:** re-run `tools/parse_sweep.sh tools/scripts.tsv` and
  confirm the "invalid function name" `HUCK_GAP` cluster drops 11 → 0 and the
  total `HUCK_GAP` falls accordingly, with no new `HUCK_LENIENT`/`HUCK_CRASH`.
- Full `cargo test` (0 failures), all `tests/scripts/*_diff_check.sh` harnesses
  green, clippy clean.

## Scope boundary

In scope: the new `valid_function_name_text` and the two call-site swaps; the new
harness; the parse-sweep confirmation. **Not** in scope: for-loop / other
identifier validation (unchanged); the separate "unexpected token after command"
sweep cluster (e.g. the `function validate_addr()` script fails for a different
reason — its own future iteration); quoted or multi-part function names. No
`bash-divergences.md` change (never a tracked divergence). Record in
`project_huck_iterations.md` + `MEMORY.md`.
