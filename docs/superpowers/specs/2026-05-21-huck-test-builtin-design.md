# huck v16: The `test` / `[` Builtin

**Date:** 2026-05-21
**Status:** Design

## Goal

Add the POSIX `test` builtin and its alias `[` — a conditional
expression evaluator that exits 0 (true), 1 (false), or 2 (usage
error). This is the first step toward control flow: `if` (v17) needs
a real condition command to branch on.

## Scope

**In scope:**
- File tests: `-e`, `-f`, `-d`, `-r`, `-w`, `-x`, `-s`, `-L`
- String tests: `-z`, `-n`, single-argument truthiness, `=`, `==`, `!=`
- Integer comparisons: `-eq`, `-ne`, `-lt`, `-le`, `-gt`, `-ge`
- Negation: `!`
- The `[` alias, requiring a closing `]` argument
- The POSIX argument-count evaluation algorithm
- Exit status `0` true / `1` false / `2` usage error

**Out of scope (deferred):**
- The `-a` / `-o` / `( )` combinators — POSIX-deprecated and
  ambiguous; modern scripts use `[ ... ] && [ ... ]`
- The double-bracket `[[ ... ]]` form — a distinct shell-keyword
  compound command parsed by the shell grammar, *not* a builtin and
  unrelated to the single-bracket `[` alias (which IS in scope
  above). A candidate for a later iteration.
- Less-common file tests: `-p`, `-S`, `-b`, `-c`, `-O`, `-G`, `-N`,
  `-k`, `-u`, `-g`, `-t`
- String `<` / `>` ordering (a `[[ ]]` feature; POSIX `test` does
  not define them)
- Any lexer / parser / AST / executor change — `test` and `[` are
  pure builtins

## Architecture

A new module `src/test_builtin.rs` owns the evaluation logic. The
`builtins.rs` dispatch gains a thin `builtin_test` wrapper that
handles the `[` framing and maps the result to an exit status. No
lexer, parser, AST, or executor change is required — that is the
reason `test` ships before `if`.

### Evaluation entry point

```rust
/// Evaluates a `test` expression. `Ok(true)` / `Ok(false)` are the
/// result; `Err(message)` is a usage error.
pub fn evaluate(args: &[String]) -> Result<bool, String>;
```

### Exit status

| Result | Exit status |
|--------|-------------|
| expression true | 0 |
| expression false | 1 |
| usage error | 2 |

Exit `1` (false) and exit `2` (error) are distinct so a caller — in
particular `if` in v17 — can tell a false condition from a broken
one.

### Operators

**File tests** (operand is a path):

| Operator | True when |
|----------|-----------|
| `-e f` | `f` exists |
| `-f f` | `f` is a regular file |
| `-d f` | `f` is a directory |
| `-r f` | `f` is readable |
| `-w f` | `f` is writable |
| `-x f` | `f` is executable |
| `-s f` | `f` exists and has size > 0 |
| `-L f` | `f` is a symbolic link |

`-e`, `-f`, `-d`, `-s` follow symlinks (resolved via
`std::fs::metadata`). `-L` uses `std::fs::symlink_metadata` so it
inspects the link itself. `-r`, `-w`, `-x` use `libc::access` with
`R_OK` / `W_OK` / `X_OK` — the POSIX-correct check against the
process's real UID/GID. A nonexistent path makes any file test
**false** (not an error).

**String tests:**

| Form | True when |
|------|-----------|
| `-z s` | `s` has length 0 |
| `-n s` | `s` has length > 0 |
| `s` (single argument) | `s` is non-empty |
| `s1 = s2` | strings equal |
| `s1 == s2` | strings equal (`==` accepted as a bash-ism) |
| `s1 != s2` | strings not equal |

**Integer comparisons:** `n1 -eq n2`, `-ne`, `-lt`, `-le`, `-gt`,
`-ge`. Both operands are parsed as decimal `i64`. A non-integer
operand is a usage error: `Err("integer expression expected")`.

**Negation:** `!` negates the result of the sub-expression it
prefixes, per the argument-count rules below. Negation maps
true↔false but leaves a usage error as a usage error — `! <broken>`
is still broken.

### Argument-count algorithm

POSIX `test` is defined by the number of arguments, not a grammar.
With `-a` / `-o` / `( )` out of scope, evaluation is a small
recursion:

```
evaluate(args):
  match args.len():
    0 -> Ok(false)
    1 -> Ok(args[0] is non-empty)
    2 -> if args[0] == "!":
            negate(evaluate([args[1]]))
         else if args[0] is a unary operator:   # -e -f -d -r -w -x -s -L -z -n
            Ok(apply_unary(args[0], args[1]))
         else:
            Err("unary operator expected" / "unknown operator")
    3 -> if args[1] is a binary operator:        # = == != -eq -ne -lt -le -gt -ge
            apply_binary(args[1], args[0], args[2])
         else if args[0] == "!":
            negate(evaluate([args[1], args[2]]))
         else:
            Err("binary operator expected")
    4 -> if args[0] == "!":
            negate(evaluate(args[1..4]))
         else:
            Err("too many arguments")
    _ -> Err("too many arguments")               # 5 or more
```

`negate(Ok(b))` is `Ok(!b)`; `negate(Err(e))` is `Err(e)`.

Consequences (all standard `test` behavior):
- `[ ]` → 0 args → false.
- `[ -f ]` → 1 arg → true: with one argument `test` only checks
  non-emptiness, and `-f` is a non-empty string. Operators act as
  operators only in the 2- and 3-argument positions.
- `[ ! ]` → 1 arg → true (the string `!` is non-empty).
- `[ ! -f foo ]` → 3 args → `args[1]` (`-f`) is not a binary
  operator and `args[0]` is `!` → negate `evaluate(["-f", "foo"])`.
- `[ a = b ]` → 3 args → `args[1]` is `=` → string compare.
- `[ -n -n ]` → 2 args → `args[0]` is unary `-n` applied to the
  string `-n` → non-empty → true.

### `builtins.rs` integration

- `BUILTIN_NAMES` (the v14 constant) gains `"test"` and `"["`.
- `run_builtin`'s `match` gains one arm covering both, passing the
  invoked name through:

  ```rust
  "test" | "[" => builtin_test(name, args),
  ```

- `builtin_test(name: &str, args: &[String]) -> ExecOutcome`:
  - If `name == "["`: the last argument must be `]`. If `args` is
    empty or its last element is not `]` → print
    `huck: [: missing ']'` to stderr, return `Continue(2)`. Otherwise
    evaluate `args` with the trailing `]` removed.
  - If `name == "test"`: evaluate `args` as-is (a trailing `]` is an
    ordinary argument).
  - Map the result: `Ok(true)` → `Continue(0)`, `Ok(false)` →
    `Continue(1)`, `Err(msg)` → print `huck: test: {msg}` to stderr,
    `Continue(2)`.

`builtin_test` produces no stdout and does not touch `Shell` state,
so its signature takes only `name` and `args`.

### `[` as a bare command word

A bare word `[` reaches v10 pathname expansion: it contains an
unquoted `[`, so a glob match is attempted; `glob::Pattern::new("[")`
fails (unterminated bracket class), and v10's invalid-pattern
fallback returns the literal `[`. So `[` arrives at the executor as
the command name `[` with no new handling required. A bare `]` has
no `[`, so it is never globbed. An integration test confirms this
path.

## Data flow examples

`test -f /etc/hostname`:
1. Lexed/expanded as the command `test` with arg `-f /etc/hostname`.
2. `run_builtin("test", ["-f", "/etc/hostname"])` → `builtin_test`.
3. `evaluate(["-f", "/etc/hostname"])` → 2 args, `args[0]` is unary
   `-f` → `metadata("/etc/hostname")` is a regular file → `Ok(true)`.
4. `Continue(0)`; `$?` becomes 0.

`[ "$x" = foo ]` with `x` unset:
1. `$x` expands to empty; the command is `[` with args
   `["", "=", "foo", "]"]`.
2. `builtin_test("[", …)` strips the trailing `]` → evaluate
   `["", "=", "foo"]`.
3. 3 args, `args[1]` is `=` → string compare `"" == "foo"` →
   `Ok(false)`.
4. `Continue(1)`; `$?` becomes 1.

`[ 3 -lt 10 ]`:
1. evaluate `["3", "-lt", "10"]` → 3 args, `-lt` binary → parse `3`
   and `10` as `i64` → `3 < 10` → `Ok(true)` → `Continue(0)`.

`[ -f foo` (missing `]`):
1. `builtin_test("[", ["-f", "foo"])` — last arg is `foo`, not `]` →
   stderr `huck: [: missing ']'` → `Continue(2)`.

`test abc -eq 1`:
1. evaluate `["abc", "-eq", "1"]` → 3 args, `-eq` binary → `"abc"`
   fails `i64` parse → `Err("integer expression expected")`.
2. stderr `huck: test: integer expression expected` → `Continue(2)`.

## Error handling summary

| Condition | Result |
|-----------|--------|
| no arguments | false (exit 1) |
| nonexistent path in a file test | false (exit 1) |
| unknown operator in operator position | usage error (exit 2) |
| 5 or more arguments | usage error (exit 2) |
| non-integer operand to an integer comparison | usage error (exit 2) |
| `[` invoked without a closing `]` | usage error (exit 2) |
| usage error under `!` | still a usage error (exit 2) |

## Testing

**`src/test_builtin.rs` unit tests** — `evaluate` directly:
- Every argument-count case: 0, 1, 2, 3, 4, and 5+ arguments
- Each file operator against `tempfile` fixtures: a regular file, a
  directory, a symlink, an empty file, a nonexistent path
- String operators: `-z`, `-n`, single argument, `=`, `==`, `!=`
- Integer operators: all six, plus a non-integer operand → `Err`
- Negation: `! expr`, `! ! expr`, `!` over a usage error
- Usage errors: unknown operator, 5+ arguments
- `-r` / `-w` / `-x`: *true* cases are tested robustly; *false*
  cases are environment-sensitive (the root user bypasses `access`
  checks) and are tested best-effort

**`src/builtins.rs` tests:**
- `run_builtin("test", …)` returns `Continue(0)` / `Continue(1)` /
  `Continue(2)` for true / false / error expressions
- `run_builtin("[", …)` strips the trailing `]` and behaves like
  `test`
- `[` without a closing `]` → `Continue(2)`

**Integration tests (`tests/test_builtin_integration.rs`)** —
end-to-end via the shell binary:
- `test -f <file>` / `[ -d <dir> ]`, with `$?` checked afterward
- a string comparison and an integer comparison
- `[` invoked without `]` writes an error and sets `$?` to 2
- a bare `[` command word reaches the builtin (the v10
  globbing-fallback path)

## File layout impact

- **New:** `src/test_builtin.rs` — `evaluate` plus operator helpers
  and unit tests
- **New:** `tests/test_builtin_integration.rs`
- **Modify:** `src/builtins.rs` — `BUILTIN_NAMES` gains `"test"` and
  `"["`; a dispatch arm; the `builtin_test` wrapper
- **Modify:** `src/main.rs` — register `mod test_builtin`
- **Modify:** `README.md` — v16 row, builtins list, features note,
  test count
- **No lexer / parser / AST / executor changes.**

## Open questions

None at design time.

## References

- POSIX 2008 — `test` utility, argument-count evaluation rules
- bash(1) — `test` / `[` builtin
- `access(2)` — file accessibility check against the real UID/GID
