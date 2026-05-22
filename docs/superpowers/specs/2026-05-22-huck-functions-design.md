# huck v22: Functions — Design Spec

## Overview

huck has all POSIX compound commands through v21. v22 adds **functions** —
user-defined named code units that take positional arguments and can be
invoked like any other command:

```
greet() {
    echo "hello, $1"
}
greet world      # → hello, world
```

To make functions actually useful, v22 also adds the supporting pieces
they need: the `{ … }` brace-group compound command (the standard
function-body form), the positional parameters `$1`-`$N`, `$@`/`"$@"`,
`$*`/`"$*"`, `$#` (in function scope), the `return` builtin, and the
`ExecOutcome::FunctionReturn` machinery that lets `return` propagate
through nested control flow back to the function call.

This is a large iteration — bigger than any single v17-v21 — because all
these pieces are mutually load-bearing. Without brace groups, function
definitions have no body. Without positional parameters, functions can't
take arguments. Without `return`, functions can't exit early.

## Scope

**In scope:**

- `name() compound-command` function-definition syntax (POSIX form;
  `function name { … }` bash extension is out).
- Brace groups `{ list ; }` as a new compound command — usable standalone
  and as the canonical function body.
- Function call mechanics: command lookup finds user-defined functions,
  positional parameters are pushed for the call and popped on return.
- `return [N]` builtin (exits the function with status `N`, defaulting to
  `$?`).
- Positional parameters `$1`-`$9`, `${10}` and higher (braced),
  `$@`/`"$@"`, `$*`/`"$*"`, `$#`. Populated by function calls only in
  v22.
- Function-vs-builtin precedence: control builtins (`return`, `exit`,
  `break`, `continue`) always win; user-defined functions then shadow
  any other builtin.
- `LoopBreak`/`LoopContinue`/`Exit` propagate out of a function call
  unchanged (so `break` inside a function affects the caller's
  enclosing loop, matching bash). Only `FunctionReturn` is caught at the
  function boundary.

**Out of scope:**

- `set --` / `shift` builtins (populating positional parameters outside
  a function call).
- Script-file arguments (`huck script.sh arg1 arg2`) — huck has no
  script-file mode.
- `$0`.
- `local`/`typeset` variable scoping (bash extension, not POSIX).
- Subshells `( list )` as a compound command, and `name() ( … )` (a
  function body that's a subshell). Requires fork-and-isolate semantics
  huck does not have.
- Functions used as a stage inside a `|` pipeline — matches the v17
  "if inside a `|` pipeline not implemented" limitation.
- Backgrounding a whole function call — matches v17/v18/v20/v21
  limitations on backgrounding compound commands.

## Architecture

| Unit | File | Responsibility |
| --- | --- | --- |
| Brace-group AST + parser | `src/command.rs` | `Command::BraceGroup`, `Keyword::{LBrace,RBrace}`, `parse_brace_group`, `UnterminatedBrace` |
| Function-definition AST + parser | `src/command.rs` | `Command::FunctionDef`, `parse_pipeline_with_first` refactor, function-def detection in `parse_command`, `FunctionName`/`FunctionBody` errors |
| Continuation | `src/continuation.rs` | `UnterminatedBrace` → `Incomplete(Compound)`; joiner knows `{` |
| Lexer for `$1`/`$@`/`$*`/`$#`/`${N}` | `src/lexer.rs` | Recognise digit-only var names and the special names `@`/`*`/`#`; emit `WordPart::AllArgs` |
| Positional parameters in expansion | `src/expand.rs` | Special-case lookup for `$1`-`$N`/`$#`; new `WordPart::AllArgs` arms; `expand_assignment` and `expand_pattern` arms too |
| Shell state | `src/shell_state.rs` | `functions: HashMap<…>`, `positional_args: Vec<String>`, `lookup_var` accessor |
| `ExecOutcome::FunctionReturn` | `src/builtins.rs` (the enum lives here) | New variant; every match site ripples (compiler-enforced) |
| Executor function-call path | `src/executor.rs` | `run_command` arms for `BraceGroup` and `FunctionDef`; function-call dispatch in `run_exec_single`; `call_function` scope helper |
| `return` builtin | `src/builtins.rs` | New entry in `BUILTIN_NAMES` and `run_builtin` |
| Error messages | `src/shell.rs` | `parse_error_message` arms for the new `ParseError` variants |

## Section 1 — Brace groups `{ … }`

The smallest, most foundational piece. Functions need a body form, and
`{ … }` is the canonical one.

### Lexer

No changes. `{` and `}` stay ordinary word characters. POSIX requires
them as separate words (whitespace-separated), so `{ cmd; }` lexes to
`Word("{")`, `Word("cmd")`, `Op(Semi)`, `Word("}")`; `{cmd}` (no spaces)
stays a single literal `Word("{cmd}")` and is *not* a brace group —
matching bash.

### Keywords

`Keyword` gains `LBrace` and `RBrace`. `keyword_of` recognises the
standalone single-Literal unquoted words `"{"` and `"}"`. `Keyword::name`
gets the two arms.

### AST

`Command` gains `BraceGroup(Box<Sequence>)`. A brace group is just a
sequence run in the current shell environment — no isolation, no
subshell semantics (huck doesn't have subshells).

### Parser

A new `ParseError::UnterminatedBrace`. A new `parse_brace_group` that
mirrors `parse_while`'s structure:

```rust
expect_keyword(iter, Keyword::LBrace, ParseError::UnterminatedBrace)?;
let body = parse_compound_section(iter, &[Keyword::RBrace],
                                   ParseError::UnterminatedBrace)?;
expect_keyword(iter, Keyword::RBrace, ParseError::UnterminatedBrace)?;
Ok(body)
```

`parse_command` gains an arm:
`Some(Keyword::LBrace) => Ok(Command::BraceGroup(Box::new(parse_brace_group(iter)?)))`.

### Executor

`run_command` gains an arm:
`Command::BraceGroup(seq) => execute_sequence_body(seq, shell, sink)`.
Just delegates — `break`/`continue`/`return`/`exit` propagate through
exactly as they do for any sequence.

### Continuation

`classify` adds `UnterminatedBrace` to the `Incomplete(Compound)` arm so
multi-line `{ … }` continuation works for free. `ends_with_control_keyword`
gains `"{"`.

### Backward compatibility

Adding `{`/`}` to `keyword_of` makes them reserved when written as
standalone words. An existing test that uses an unquoted `{` or `}` as
a literal would need to quote it — a small ripple parallel to v21's
`(`/`)` ripple but bounded (no lexer change, only `keyword_of`'s
recognition is new).

## Section 2 — Function definition: AST & parser

### AST

`Command` gains `FunctionDef { name: String, body: Box<Command> }`. The
body is any compound `Command` (most often `BraceGroup` from Section 1,
but POSIX allows any compound — `if`/`while`/`for`/`case`/`{ … }`).

### Parser refactor

`parse_pipeline` is split into:

```rust
fn parse_pipeline_with_first<I>(
    first: Option<Word>,
    iter: &mut std::iter::Peekable<I>,
) -> Result<Pipeline, ParseError> { … }

fn parse_pipeline<I>(iter: &mut std::iter::Peekable<I>) -> Result<Pipeline, ParseError> {
    parse_pipeline_with_first(None, iter)
}
```

When `first: Some(w)`, the pipeline starts with `program = Some(w)`.
This is the minimum change that enables two-token lookahead in
`parse_command` without a generic peek wrapper.

### Function-def detection in `parse_command`

After the compound-keyword dispatch (now including `LBrace`) and before
the bare-pipeline call, the non-keyword arm:

```
if peek is Word(w):
    consume w
    if peek is LParen:
        // Function definition.
        name = valid_identifier_text(&w).ok_or(ParseError::FunctionName)?
        consume (
        expect )                     // missing → ParseError::FunctionBody
        skip_newlines
        body = parse_command(iter)   // recurse; must be a compound
        if body is Pipeline → ParseError::FunctionBody
        return FunctionDef { name, body: Box::new(body) }
    else:
        return Pipeline(parse_pipeline_with_first(Some(w), iter)?)
else:
    return Pipeline(parse_pipeline(iter)?)
```

The identifier validation reuses v20's `for`-loop variable rule (single
unquoted `Literal`, `[A-Za-z_][A-Za-z0-9_]*`, not a reserved keyword).
The existing `for_variable_name` is refactored — its body extracted into
a shared `valid_identifier_text(&Word) -> Option<String>` that both v20
and v22 use.

### New errors

- `ParseError::FunctionName` — message arm: `"invalid function name"`.
- `ParseError::FunctionBody` — message arm: `"'(' must be followed by ')' and a compound-command body"`.

### Executor placeholder

`run_command` gains `Command::FunctionDef(_) => unreachable!("function
execution lands in Section 3")` for the AST-and-parser commit; Section 3
replaces it.

### Examples

| Input | Result |
| --- | --- |
| `foo() { echo hi; }` | `FunctionDef { name: "foo", body: BraceGroup(…) }` |
| `foo() if true; then echo; fi` | `FunctionDef { name: "foo", body: If(…) }` |
| `foo() echo hi` | `Err(FunctionBody)` (pipeline body) |
| `1foo() { …; }` | `Err(FunctionName)` |
| `foo "name"() { …; }` | parses `foo` as a normal command (quoted second word is not `(`) |
| `foo(` then EOF | `Err(FunctionBody)` |

## Section 3 — Function calls + `return`

### Shell state additions

```rust
pub functions: HashMap<String, Box<Command>>,    // function table
pub positional_args: Vec<String>,                // current frame
```

`positional_args` is initialised empty. Function calls swap it for the
call's args via `std::mem::take` save/restore (Section 4 reads from
this).

### `ExecOutcome::FunctionReturn(i32)`

A new variant alongside `Exit`/`Continue`/`LoopBreak`/`LoopContinue`. It
propagates through every sequence / `if` / loop / `case` short-circuit
using exactly the same machinery v18 established for `LoopBreak` — every
existing `match` on `ExecOutcome` gains a `FunctionReturn` arm
(compiler-enforced, the same ripple v18 had). Caught only at the
function-call boundary.

### `return` builtin

New entry in `BUILTIN_NAMES` and `run_builtin`:

```rust
"return" => {
    let code = match args.first() {
        Some(s) => s.parse::<i32>().unwrap_or(shell.last_status()),
        None => shell.last_status(),
    };
    ExecOutcome::FunctionReturn(code)
}
```

Pattern mirrors v18's `break`/`continue`. A stray `return` at the top
level (no enclosing function) is neutralised by the REPL's existing
"stray loop control" arm, extended to also handle `FunctionReturn` →
`set_last_status(0)`.

### Function-definition execution

`Command::FunctionDef { name, body }` →
`shell.functions.insert(name.clone(), body.clone()); ExecOutcome::Continue(0)`.
`Command` already derives `Clone`; cloning the body once per definition
(and once per call — see below) is fine for a learning shell.

### Function call mechanism

In `run_exec_single`, after `resolve` expands the program/args:

```
1. If program is a control builtin (return/exit/break/continue) → run_builtin.
2. Else if shell.functions has program → call_function(body.clone(), args, shell, sink).
3. Else if program is any other builtin → run_builtin.
4. Else → fork-exec from PATH.
```

`call_function` saves `shell.positional_args` via `std::mem::take`,
replaces it with the call's args (the args slice *excluding* the
program name — POSIX `$1` is the first user arg, not the function
name), runs `run_command(&body, shell, sink)`, restores the saved
positionals, and:

- `ExecOutcome::FunctionReturn(n)` → return `Continue(n)`.
- `Exit`, `LoopBreak`, `LoopContinue`, `Continue` all propagate
  unchanged — `break` inside a function targets the caller's enclosing
  loop (matching bash), and `exit` propagates out of the shell as
  always.

### Functions inside `|` pipelines

Not supported in v22 — matching the v17 `if`-inside-`|` limitation. A
function used as a single-stage command works everywhere else
(sequences, `&&`/`||`, redirects, `&`-backgrounding-of-the-pipeline-as-a-whole-not-yet-anyway).

### Why the control-builtin set is hardcoded

`return`/`exit`/`break`/`continue` are flow control. Letting a function
shadow `return` (`return() { … }`) would infinitely recurse or hang the
shell on the next `return`. Letting it shadow `exit` makes the shell
unkillable. Hardcoding the un-shadowable set keeps the fundamentals
working without making *all* builtins un-overridable — `cd() { … }` and
`echo() { … }` (common useful wrappers) still work.

## Section 4 — Positional parameters

### Shell state

`positional_args: Vec<String>` (added in Section 3); empty at the top
level, replaced per function call.

### `WordPart` additions

One new variant:

```rust
WordPart::AllArgs { quoted: bool, joined: bool }
// joined = false → $@ / "$@"
// joined = true  → $* / "$*"
```

`$1`-`$N`, `${N}`, and `$#` reuse the existing
`WordPart::Var { name, quoted }` with `name` being a digit string or
`"#"`.

### Lexer changes

The `$` handler in `read_dollar_expansion` is extended:

- `$N` where N is a single ASCII digit → `Var { name: "<digit>" }`.
- `$@` → `AllArgs { joined: false, quoted: in_quotes }`.
- `$*` → `AllArgs { joined: true, quoted: in_quotes }`.
- `$#` → `Var { name: "#", quoted: in_quotes }`.

The `${…}` brace-form parser is extended to accept digit-only names
(`${10}`, `${42}`) and the special names `@`/`*`/`#`. `${10}` →
`Var { name: "10" }`. `${@}`/`${*}` → `AllArgs { joined, quoted }`.
`${#}` → `Var { name: "#" }`.

### Variable-lookup integration

The expander's `WordPart::Var { name }` lookup is extended to recognise
positional names before falling back to the regular variable HashMap.
Implementation: introduce `Shell::lookup_var(name: &str) -> Option<String>`
(returns owned `String` because computed values can't be `&'static`):

```rust
pub fn lookup_var(&self, name: &str) -> Option<String> {
    if name == "#" {
        return Some(self.positional_args.len().to_string());
    }
    if name.chars().all(|c| c.is_ascii_digit()) && !name.is_empty() {
        let n: usize = name.parse().ok()?;
        if n == 0 { return None; }   // $0 deferred to a later iteration
        return self.positional_args.get(n - 1).cloned();
    }
    self.vars.get(name).map(|v| v.value.clone())
}
```

The expander's `WordPart::Var` arms call `lookup_var` instead of `get`.
Non-expander callers (`cd`, completion, …) keep using the existing
`shell.get` for plain variable names.

### `WordPart::AllArgs` expansion semantics

A new arm in `expand()`:

- `$@` / `$*` **unquoted**: each arg becomes its own field, then each
  field is IFS-split (matches huck's existing variable-splitting). The
  two forms behave identically when unquoted.
- `"$@"` **quoted, joined=false**: each arg becomes its own field, **no**
  IFS-splitting — preserves each arg verbatim, including any internal
  whitespace. This is the only construct in shell that produces
  *multiple* fields while quoted. Empty `positional_args` → zero fields.
- `"$*"` **quoted, joined=true**: a single field, all args joined by the
  first IFS character (default space). Empty → empty single field (or
  zero fields if at end-of-word — follows huck's existing emit/no-emit
  rules).
- The standard POSIX prefix/suffix behaviour when `"$@"` is embedded in
  a word (`foo"$@"bar` → `fooarg1`, `arg2`, …, `argNbar`) is
  implemented — the `expand()` field-building machinery already
  supports merging into the current field and starting new ones.

`expand_assignment` (no-split) and `expand_pattern` (no-split,
quote-aware) get matching `AllArgs` arms — there's no field splitting
in either context, so they concatenate args with a space (or per IFS).

### Boundaries

`shift`, `set --`, and script-file argument population are deferred —
the function-call path is the only way to populate `positional_args` in
v22.

## Section 5 — Testing

**Lexer unit tests** — `$1` / `${10}` lex to `Var { name: "1" / "10" }`;
`$@` → `AllArgs { joined: false }`; `$*` → `AllArgs { joined: true }`;
`"$@"` → quoted variant; `$#` → `Var { name: "#" }`; `${@}` / `${*}` /
`${#}` brace forms produce the same; quoted `{`/`}` stay literal word
content.

**Parser unit tests** — `{ echo; }` parses to `BraceGroup`; multi-line
`{ … }` matches single-line; truncated `{` → `UnterminatedBrace`;
`foo() { echo; }` parses to `FunctionDef { name, body: BraceGroup }`; a
non-brace compound body (`foo() if true; then echo; fi`); an invalid
name (`1foo()`, leading digit) → `FunctionName`; missing `)` (`foo(`)
→ `FunctionBody`; a pipeline body (`foo() echo hi`) → `FunctionBody`;
the `for`-loop variable validator still works (`for_variable_name`
extraction did not regress).

**Executor unit tests** — a brace group runs its body in the current
shell; `FunctionDef` registers and returns 0; a defined function called
with no args runs its body; `$1`/`$@`/`$#` empty at the top level;
`return 7` makes a function exit with status 7; `return` with no arg
uses `$?`; `LoopBreak`/`LoopContinue` propagate *out* of a function
call (no isolation); a user `cd() { echo my-cd; }` shadows the builtin
while `return` does not.

**Integration tests** (new `tests/functions_integration.rs`) — basic
definition + call; args (`add() { echo $(($1 + $2)); }; add 3 4` →
`7`); `$#` counts (`f a b c` → `3`); `$@` unquoted with a splittable
arg; `"$@"` quoted preserves individual args; `"$*"` joined with IFS;
recursion (a `countdown N` function); early `return` with a status; a
function in `&&`/`||`/`;` sequences; a function shadowing `cd`;
`return` outside a function at the REPL is harmless (`$?` = 0); `break`
inside a function inside a `while` exits the caller's `while`;
multi-line `f() { … }` definition; standalone `{ cmd1; cmd2; }`;
truncated/missing-brace → syntax error; a function whose body is an
`if`/`for`/`case` compound.

**No new PTY tests** — multi-line function definitions ride on v19's
continuation mechanism (already PTY-covered for unterminated compound
commands); consistent with v18/v20/v21 adding none.

**Regression** — all existing tests stay green (currently 814 after
v21 + the warning cleanup; the v17-v21 suites prove that adding
`{`/`}` to `keyword_of` and a new `ExecOutcome::FunctionReturn` variant
is backward-compatible — the `ExecOutcome` ripple is exactly the
exhaustive-match exercise v18 already did for `LoopBreak`).

## Error handling

| Situation | Behaviour |
| --- | --- |
| Truncated `{ … }` (missing `}` or EOF mid-group) | `ParseError::UnterminatedBrace`; at a REPL incomplete, continuation line is read |
| Invalid function name (`1foo()`, quoted, multi-part Word) | `ParseError::FunctionName`, syntax error, `$?` = 2 |
| Function definition with missing `)` or non-compound body | `ParseError::FunctionBody`, syntax error, `$?` = 2 |
| `return` outside a function | Neutralised at the REPL; `$?` set to 0 |
| `break`/`continue` inside a function but outside any loop | Propagate out; if the caller has no loop either, the REPL neutralises them — unchanged from today |
| Function call with too few args (`f` references `$2` that's unset) | `$2` expands to empty (unset variable behaviour) |
| `return N` where `N` is not an integer | Fall back to `$?` (mirrors v18 `break`/`continue` argument handling) |
| User defines a function with the same name as a control builtin (`return() { … }`) | Definition succeeds; the builtin always wins, so the function is unreachable |
