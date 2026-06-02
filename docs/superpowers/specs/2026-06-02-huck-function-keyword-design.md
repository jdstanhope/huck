# v77 — Function Keyword Form (M-09) Design Spec

**Date**: 2026-06-02
**Iteration**: v77
**Divergence closed**: M-09 (`function name { ... }` keyword form for function
definition). Medium-priority Tier-2 deferral.

## Goal

Add bash's `function NAME { ... }` keyword form for defining functions,
alongside huck's existing POSIX `name() { ... }` form. The optional
`()` after the name (`function NAME () { ... }`) is also supported.

Both forms produce the same `Command::FunctionDef` AST node; the
function-call path is unchanged.

## Non-goals

These are explicitly deferred (M-09 follow-up entries in
`docs/bash-divergences.md` after merge):

- **Relaxed function-name characters**. Bash 5 accepts `.`, `-`, `+`,
  `:` and other non-POSIX-identifier characters when the function is
  defined via the keyword form (`function foo.bar { :; }`). Huck stays
  POSIX-strict for both forms.
- **Definition-attached redirections** (`function NAME { body } > file`
  / `name() { body } > file`). Bash allows attaching redirections to
  the function definition itself, taking effect at every call. Huck's
  existing POSIX form already does NOT support this — confirmed by
  `f() { :; } > /tmp/x` producing `huck: syntax error: unexpected
  token after command`. The keyword form matches the POSIX form's
  behavior. Both forms will gain this in a future iteration.

## Architecture

Single file changed: `src/command.rs`. Three localized edits:

1. **`Keyword` enum gains `Function`**:
   - Add `Function => "function"` to `keyword_name()` (around line 27).
   - Add `"function" => Some(Keyword::Function)` to the parse map
     (around line 60).

2. **Parser dispatch in `parse_command`** (around line 651): add one
   match arm `Some(Keyword::Function) => parse_function_keyword_def(iter)`.

3. **New helper `parse_function_keyword_def`** (sibling of the existing
   `parse_function_def`):
   - Consume the `function` keyword token.
   - Read the next token. It must be a Word that's a valid identifier
     and not itself a reserved keyword — validated via the existing
     `valid_identifier_text`. Else → `ParseError::FunctionName`.
   - Optionally consume `()`: peek the next token; if it's `Op(LParen)`,
     consume it and require the following token to be `Op(RParen)`
     (else `ParseError::FunctionBody`).
   - `skip_newlines(iter)`.
   - If the iterator is now exhausted, return
     `ParseError::UnterminatedFunction`.
   - `parse_command(iter)` for the body.
   - Verify the body is one of the allowed compound shapes
     (`If`/`While`/`For`/`Case`/`BraceGroup`/`Subshell`/`DoubleBracket`
     — same constraint the POSIX form enforces, factored into a shared
     helper).
   - Return `Command::FunctionDef { name, body: Box::new(body) }`.

Zero changes to lexer, executor, expand, or any other module. The
function-call path doesn't care which surface form created the
`FunctionDef`.

## Data flow (unchanged)

```
   user input:  function foo { echo hi; }
                       |
              lex --> [Word("function"), Word("foo"), LBrace, Word("echo"),
                       Word("hi"), Semi, RBrace]
                       |
   parse_command (sees Word("function") at command position, classifies
   via keyword_of as Keyword::Function)
                       |
              parse_function_keyword_def(iter)
                       |
              Command::FunctionDef { name: "foo", body: BraceGroup(...) }
                       |
   stored in shell.functions["foo"] at execute time
```

## Disambiguation: `function` outside command position

- **`function=value`**: lexer's assignment-prefix detection (`src/lexer.rs:497`)
  emits an `AssignPrefix(Bare("function"), append=false)` BEFORE
  any keyword classification fires at parse time. The token reaches
  the parser as an assignment-shaped Word; `keyword_of` is not consulted.
  `function=value` continues to work as a normal variable assignment.

- **`echo function`**: keyword classification only fires at command
  position (word 0 of a simple command after the most recent
  separator). `function` in argument position is a plain word literal.

- **`if function name { :; }; then ...; fi`**: `parse_command`
  dispatches via `keyword_of` in any command-accepting context, so the
  inner `function` definition parses correctly.

- **`function` alone (no following word)**: parse error
  `FunctionName`.

- **`function name` (no body)**: `UnterminatedFunction`.

- **`function name X` where X isn't a valid compound or `(`**:
  `FunctionBody`.

## Refactor: shared name validation + body shape check

`parse_function_def` (the existing POSIX-form parser) and
`parse_function_keyword_def` (the new keyword-form parser) share two
concerns:

1. Validating that the name token is a POSIX identifier and not a
   reserved keyword (`valid_identifier_text`).
2. Validating that the body is one of the allowed compound shapes.

The validation of (1) already lives in `valid_identifier_text` and is
reused. For (2), factor a small `is_function_body_shape(&Command) -> bool`
helper out of the existing inline match in `parse_function_def`:

```rust
fn is_function_body_shape(cmd: &Command) -> bool {
    matches!(
        cmd,
        Command::If(_) | Command::While(_) | Command::For(_)
            | Command::Case(_) | Command::BraceGroup(_)
            | Command::Subshell { .. } | Command::DoubleBracket(_)
    )
}
```

Both parsers call it. The shape list above includes `DoubleBracket`
— bash accepts `[[ ... ]]` as a function body for both forms, but
huck's existing `parse_function_def` (POSIX form) does NOT. The shared
helper closes this gap for both forms simultaneously. No existing test
exercises `f() [[ ... ]]`, so the change is purely additive — POSIX-form
tests continue to pass and the keyword form gets `[[ ]]` body support
for free.

Add one regression test for the POSIX-form `[[ ]]` body acceptance
(`function_posix_form_double_bracket_body` in `src/command.rs::tests`)
to lock in the bash-compat extension.

## Errors (no new variants)

All errors use existing `ParseError` variants:

| Condition | Error |
|---|---|
| `function` with no following token | `FunctionName` |
| `function name` where name is not a POSIX identifier | `FunctionName` |
| `function if { :; }` (name is a reserved keyword) | `FunctionName` |
| `function name` with iterator exhausted | `UnterminatedFunction` |
| `function name X` where X is not a compound or `(` | `FunctionBody` |
| `function name (` with no `)` | `FunctionBody` |
| `function name () X` where X is not a compound | `FunctionBody` |

## Testing

### Unit tests (`src/command.rs::tests`)

~12 tests covering parser behavior:

- `function_keyword_form_brace_body` — `function f { echo hi; }`.
- `function_keyword_form_with_parens` — `function f() { :; }`.
- `function_keyword_form_subshell_body` — `function f() ( :; )`.
- `function_keyword_form_compound_body` — `function f if true; then :; fi`.
- `function_keyword_form_newline_before_body` — `function f\n{\n:;\n}`.
- `function_keyword_no_name_errors` — `function { :; }` → `FunctionName`.
- `function_keyword_keyword_name_errors` — `function if { :; }` → `FunctionName`.
- `function_keyword_missing_body_errors` — `function f` → `UnterminatedFunction`.
- `function_keyword_bad_body_errors` — `function f echo hi` → `FunctionBody`.
- `function_keyword_unbalanced_parens_errors` — `function f (` → `FunctionBody`.
- `function_as_assignment_var_still_works` — regression: `function=v; echo $function` → `v`.
- `function_in_arg_position_still_works` — regression: `echo function` echoes literal.

### Integration tests (`tests/function_keyword_integration.rs`, new)

~6 binary-driven tests verifying end-to-end semantics:

- Define + call via keyword form with brace body.
- Define + call via keyword form with optional parens.
- Verify positional args propagate (`function f { echo "$1-$2"; }; f a b` → `a-b`).
- Verify keyword form and POSIX form produce identical observable
  behavior for the same body.
- Verify `set -x` shows the function call correctly.
- Define via keyword form, redefine via POSIX form, verify latest wins.

### Bash-diff harness (`tests/scripts/function_keyword_diff_check.sh`, new)

~6 fragments byte-identical against bash 5.2:

- `function f { echo hi; }; f`
- `function f() { echo "$1"; }; f arg`
- `function f () ( echo nested; ); f`
- `function f if true; then echo cond; fi; f`
- Define + call within a subshell.
- A function defined via keyword form being callable from a script
  context.

### Test budget

~12 unit + ~6 integration + ~6 diff-check fragments. ~150 LOC of
tests + ~50 LOC of parser. Small iteration.

## Scope estimate

Implementation: ~50 LOC across `src/command.rs` (keyword entry, dispatch
arm, new helper, shared body-shape predicate).

Tests + docs: ~200 LOC.

Single file changed for production code. Two new files for tests +
harness. M-09 entry flipped + change-log entry in
`docs/bash-divergences.md`. v77 row in `README.md`.

Likely 1-2 tasks:
- Task 1: parser changes + unit tests.
- Task 2: integration tests + bash-diff harness + docs.

## Deferrals (become entries in `bash-divergences.md`)

After v77 ships, M-09's status becomes `[fixed v77]`. The follow-on
entries:

- **Relaxed function-name characters** (`.`, `-`, `+`, `:`, etc.) for
  both forms. Bash 5 allows; huck stays POSIX-strict.
- **Definition-attached redirections** (`function name { body } > file`
  and `name() { body } > file`). Neither form supports them in huck.

## Change-log entry (for `docs/bash-divergences.md`)

To add after merge:

> **2026-06-XX** (implementer updates to merge date): v77 ships M-09 —
> bash's `function NAME { ... }` keyword form. New `Keyword::Function`
> variant + parse-map entry in `src/command.rs`. New parser arm in
> `parse_command` dispatches to a new `parse_function_keyword_def`
> helper that consumes the keyword, validates the name via the existing
> `valid_identifier_text`, optionally consumes `()`, then parses the
> compound body. Body shape check factored into a shared
> `is_function_body_shape` predicate; pre-existing POSIX-form omission
> of `[[ ]]` body closed as part of the consolidation. Lexer and
> executor unchanged. `function=value` continues to work as a normal
> assignment (lexer's assignment-prefix detection fires before keyword
> classification). 12 unit tests + 6 integration tests + 6 bash-diff
> fragments (byte-identical to bash 5.2). Deferred follow-on: relaxed
> name characters and definition-attached redirections (both forms).

## Open questions

None. All architectural decisions resolved during brainstorm.
