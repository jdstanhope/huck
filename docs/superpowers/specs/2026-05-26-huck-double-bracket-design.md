# v30: `[[ ]]` Extended Test — Design Spec

## Goal

Implement bash's `[[ ]]` extended test construct: pattern-aware string
comparison (`==`/`!=` with glob RHS), regex matching (`=~`),
lexicographic string comparison (`<`/`>`), integer comparison (`-eq`
et al.), file tests (`-f`, `-d`, …), logical combinators (`&&`/`||`/
`!`/`( )`), and no word-splitting / no pathname expansion on operands.
Closes M-14 from `docs/bash-divergences.md`.

Pre-v30: `[[` is lexed as an ordinary word; the parser doesn't
recognize it as a keyword; any `[[ ]]` invocation is at best a
"command not found" or a parse error. Heavy gap — `[[ ]]` is in
virtually every modern bash script.

After v30:
```sh
[[ $name == prefix_* ]]    # glob pattern match
[[ $line =~ ^[0-9]+$ ]]    # regex
[[ $count -gt 0 ]]         # integer compare
[[ -f $path && -r $path ]] # combinators
[[ $a < $b ]]              # lex compare
```

## Scope

User-confirmed:
- **Full bash operator set** — pattern `==`/`!=`, regex `=~`,
  lexicographic `<`/`>`, integer `-eq`/`-ne`/`-lt`/`-gt`/`-le`/`-ge`,
  file tests (all huck currently supports in `test`), string tests
  `-n`/`-z`, logical `!`/`&&`/`||`, grouping `( )`.
- **Pattern-vs-literal via `expand_pattern`** — RHS of `==`/`!=` is a
  glob pattern when unquoted, literal when any part is quoted. Reuses
  v21 case-pattern semantics.
- **`regex` crate** for `=~` — Rust's RE2-style engine. Documented
  divergence: no lookbehind/lookahead (rarely used in shell scripts).

## Semantics

**No word-splitting**: `FOO="a b"; [[ $FOO == "a b" ]]` is true. The
parser collects each operand as a single Word (delimited by `[[`/`]]`
boundaries, whitespace, and known operators). At eval time the executor
expands via `expand_assignment` (no-split, no-glob).

**No pathname expansion** on operands: `[[ -f *.txt ]]` does NOT
expand `*.txt` to a list of files — `*.txt` is the literal operand
to `-f`, which checks if a file named `*.txt` exists. Matches bash.

**Pattern matching for `==`/`!=`**: RHS Word is run through
`expand_pattern` (v21), which honors `quoted` flags on Literal parts.
The result is a glob pattern (escaped-where-quoted) compiled via the
existing `glob::Pattern`. The LHS is expanded via `expand_assignment`
and matched.

- `[[ hello.txt == *.txt ]]` — RHS unquoted `*` is a wildcard → true.
- `[[ hello.txt == "*.txt" ]]` — RHS quoted `*` is literal → false
  (hello.txt is not literally `*.txt`).

**Regex matching for `=~`**: RHS Word is expanded via
`expand_assignment` (no glob escaping, since regex isn't glob). The
expansion is compiled as a `regex::Regex`. The LHS is matched.
Invalid regex → exit code 2 with `huck: [[: regex error: ...` on
stderr (bash also exit-2's on malformed regex; the message format
differs).

**Lexicographic `<`/`>`**: byte-string comparison. `[[ "abc" < "abd" ]]`
is true. No locale-awareness — matches bash's default (which actually
respects `LC_COLLATE`, but huck doesn't honor locale; document as
minor divergence).

**Integer ops `-eq`/etc.**: both sides expanded to strings, parsed as
i64, compared numerically. Non-numeric → exit 2 with `huck: [[: bad
integer: <text>`.

**File tests `-f`/`-d`/`-r`/`-w`/`-x`/`-e`/`-s`/`-L`**: reuse the
existing `test_builtin` logic. Operand expanded via
`expand_assignment` (no split).

**String tests `-n`/`-z`**: standard. `[[ -n $foo ]]` is true if
`$foo` is non-empty after expansion; `-z` is the inverse.

**Combinators**:
- `!` unary negation (higher precedence than `&&`).
- `&&` short-circuit AND.
- `||` short-circuit OR (lower precedence than `&&`).
- `( )` grouping — does NOT spawn a subshell (unlike top-level `(...)`).

**Precedence** (low → high): `||` < `&&` < `!` < primary.

**Exit status**: `[[ ]]` evaluates to true → 0; false → 1; syntax /
regex error → 2.

**Composition**:
- `if [[ ... ]]; then ...; fi` — works (the `[[ ]]` is the if's
  condition).
- `while [[ ... ]]; do ...; done` — works.
- `[[ ... ]] && cmd` — short-circuit, runs cmd if true.
- `FOO=hi [[ $FOO == hi ]]` — inline assignments (v23) apply
  per-stage; `[[ ]]` sees FOO=hi.
- `([[ ... ]])` — runs in subshell (v28). Same exit-status semantics.
- `[[ ... ]] | cat` — `[[ ]]` is a compound command (v25 classification
  routes it InProcess, forks the subshell, runs the test). Its
  exit-status reaches `cat` via the pipe close.

## Lexer changes

`src/lexer.rs`:

**`[[` and `]]` as keywords**: huck's existing `Keyword` enum already
has variants like `If`, `Then`, `While`, `LBrace`, etc. Add:
```rust
pub enum Keyword {
    // existing...
    DoubleBracketOpen,    // [[ at command-start
    DoubleBracketClose,   // ]] terminator
}
```

The lexer's keyword-recognition path (which produces `Token::Op` or
similar for reserved words like `if`, `while`) checks at WORD-start
position: if a token would be the literal `[[` or `]]`, emit the
keyword instead.

Like `if`/`while`/`{`, `[[` only triggers in command-start position.
Mid-word `[[` (e.g. `array[[i]]` if huck ever supported arrays — not
relevant today) stays a literal word.

The interior of `[[ ... ]]` is normal Word/Operator tokenization. The
parser will consume tokens until it sees `]]`.

## AST changes

`src/command.rs`:

```rust
pub enum Command {
    // existing...
    DoubleBracket(Box<TestExpr>),    // NEW
}
```

New module `src/test_expr.rs` (or inline in `command.rs`):
```rust
pub enum TestExpr {
    Unary { op: TestUnaryOp, operand: Word },
    Binary { op: TestBinaryOp, lhs: Word, rhs: Word },
    Regex { lhs: Word, pattern: Word },
    Not(Box<TestExpr>),
    And(Box<TestExpr>, Box<TestExpr>),
    Or(Box<TestExpr>, Box<TestExpr>),
}

pub enum TestUnaryOp {
    FileExists,       // -e
    IsRegFile,        // -f
    IsDir,            // -d
    IsReadable,       // -r
    IsWritable,       // -w
    IsExecutable,     // -x
    IsNonEmpty,       // -s
    IsSymlink,        // -L
    StringNonEmpty,   // -n
    StringEmpty,      // -z
}

pub enum TestBinaryOp {
    StringEq,    // == (pattern match if RHS has unquoted glob chars)
    StringNe,    // !=
    StringLt,    // < (lex)
    StringGt,    // > (lex)
    IntEq,       // -eq
    IntNe,       // -ne
    IntLt,       // -lt
    IntGt,       // -gt
    IntLe,       // -le
    IntGe,       // -ge
}
```

**Reuse vs new**: huck's `src/test_builtin.rs` already has equivalent
op enums for the single-bracket `test`/`[` builtin. Two options:
- A) Reuse `test_builtin`'s op enums.
- B) Define separate `TestUnaryOp`/`TestBinaryOp` for `[[ ]]`.

Recommend B for clarity — the `[[ ]]` semantics differ from `test` for
some ops (pattern matching, regex, no-split). Separate enums avoid
"which semantics does this op enum represent?" confusion. The shared
file-test logic can be factored into a helper function called from
both `test_builtin` and `run_double_bracket`.

## Parser changes

`src/command.rs`:

**New `parse_double_bracket(iter) -> Result<Command, ParseError>`**:

1. Consume `Token::Op(Keyword::DoubleBracketOpen)` (or however the lexer represents it).
2. Parse expression tree via Pratt-style precedence:
   - `parse_test_or` — handles `||`, recurses into `parse_test_and`.
   - `parse_test_and` — handles `&&`, recurses into `parse_test_not`.
   - `parse_test_not` — handles `!`, recurses into `parse_test_primary`.
   - `parse_test_primary` — handles `( expr )` grouping OR a single
     test (unary / binary / regex). For a test, read the first operand
     token:
     - If it's a unary-op word (`-f`, `-d`, `-n`, `-z`, etc.): consume,
       read operand Word, build `Unary { op, operand }`.
     - Else: read first Word as `lhs`, peek next token for binary op
       (`==`, `!=`, `=~`, `<`, `>`, `-eq`, etc.), read `rhs` Word,
       build `Binary { op, lhs, rhs }` or `Regex { lhs, pattern }`.
3. Consume `Token::Op(Keyword::DoubleBracketClose)`.
4. Return `Command::DoubleBracket(Box::new(expr))`.

**Operator recognition inside `[[ ]]`**:
- `==`, `!=`, `=~`, `<`, `>` arrive as Word tokens (because the lexer
  doesn't emit them as operators). The parser checks the Word's
  single-Literal text to recognize them.
- `-eq` etc. similarly: Words starting with `-` followed by `eq`/`ne`/etc.
- `!`, `&&`, `||`, `(`, `)` — these MAY already be Operator tokens
  from the existing lexer (`!` isn't currently, but `&&` and `||` are).
  Handle uniformly.

**Top-level dispatch**: `parse_command` gains an arm for
`Keyword::DoubleBracketOpen` (alongside `if`, `while`, etc.) that
dispatches to `parse_double_bracket`.

## Executor changes

`src/executor.rs`:

**New `run_double_bracket(expr: &TestExpr, shell: &mut Shell) -> ExecOutcome`**:

```rust
fn run_double_bracket(expr: &TestExpr, shell: &mut Shell) -> ExecOutcome {
    match eval_test_expr(expr, shell) {
        Ok(true) => ExecOutcome::Continue(0),
        Ok(false) => ExecOutcome::Continue(1),
        Err(msg) => {
            eprintln!("huck: [[: {msg}");
            ExecOutcome::Continue(2)
        }
    }
}

fn eval_test_expr(expr: &TestExpr, shell: &mut Shell) -> Result<bool, String> {
    match expr {
        TestExpr::Unary { op, operand } => {
            let s = expand_assignment(operand, shell);
            Ok(eval_unary(*op, &s))
        }
        TestExpr::Binary { op, lhs, rhs } => {
            let l = expand_assignment(lhs, shell);
            eval_binary(*op, &l, rhs, shell)
        }
        TestExpr::Regex { lhs, pattern } => {
            let l = expand_assignment(lhs, shell);
            let p = expand_assignment(pattern, shell);
            let re = regex::Regex::new(&p).map_err(|e| format!("regex error: {e}"))?;
            Ok(re.is_match(&l))
        }
        TestExpr::Not(inner) => eval_test_expr(inner, shell).map(|b| !b),
        TestExpr::And(a, b) => {
            if eval_test_expr(a, shell)? { eval_test_expr(b, shell) } else { Ok(false) }
        }
        TestExpr::Or(a, b) => {
            if eval_test_expr(a, shell)? { Ok(true) } else { eval_test_expr(b, shell) }
        }
    }
}

fn eval_binary(op: TestBinaryOp, lhs: &str, rhs_word: &Word, shell: &mut Shell)
    -> Result<bool, String>
{
    match op {
        StringEq | StringNe => {
            // Pattern-match: expand_pattern honors quoted flags.
            let pattern_str = expand_pattern(rhs_word, shell);
            let pat = glob::Pattern::new(&pattern_str).map_err(|e| format!("bad pattern: {e}"))?;
            let matched = pat.matches(lhs);
            Ok(if op == StringEq { matched } else { !matched })
        }
        StringLt | StringGt => {
            let rhs = expand_assignment(rhs_word, shell);
            Ok(match op {
                StringLt => lhs < rhs.as_str(),
                StringGt => lhs > rhs.as_str(),
                _ => unreachable!(),
            })
        }
        IntEq | IntNe | IntLt | IntGt | IntLe | IntGe => {
            let rhs = expand_assignment(rhs_word, shell);
            let l: i64 = lhs.parse().map_err(|_| format!("bad integer: {lhs}"))?;
            let r: i64 = rhs.parse().map_err(|_| format!("bad integer: {rhs}"))?;
            Ok(match op {
                IntEq => l == r, IntNe => l != r,
                IntLt => l < r,  IntGt => l > r,
                IntLe => l <= r, IntGe => l >= r,
                _ => unreachable!(),
            })
        }
    }
}
```

`eval_unary` dispatches to file-test helpers (factored from
`test_builtin`) or string-test (`-n`/`-z`).

**Composition with `if`/`while`**: `run_command` dispatches
`Command::DoubleBracket` like any other compound command. `if [[ ... ]];
then …; fi` works because the existing `run_if` evaluates its condition
via `run_command`, which returns the exit status.

**Composition with pipelines**: per v25, `Command::DoubleBracket` in a
pipeline stage is classified `InProcess` (not external, not Simple) →
forks via `fork_and_run_in_subshell` → runs in subshell → exit status
propagates.

## New dependency

Add `regex = "1.10"` (or current major) to `Cargo.toml`. Used only for
`[[ $s =~ pattern ]]`. The crate is mature, fast (RE2-based), and has
been stable for years.

## Edge cases

- **Empty `[[ ]]`**: parser error (no expression).
- **`[[ -f ]]`** (missing operand): parser error.
- **`[[ x == ]]`** (missing RHS): parser error.
- **`[[ ]` and `]] ]`**: parsed as `[[` keyword followed by remaining text
  as operands; if `]]` doesn't appear, parser errors with
  unterminated.
- **`[[ "$x" == "" ]]`** vs `[[ -z $x ]]`: both work; the latter is
  more idiomatic. Both should give the same result for unset/empty x.
- **`[[ "$ARRAY[0]" == foo ]]`** (bash array indexing): out of scope —
  arrays aren't implemented in huck.
- **`[[ -v var ]]`** (variable-is-set): out of scope for v30 (not in
  the file-test list); document.
- **`[[ $a -ot $b ]]`** (file ages, `-ot`/`-nt`/`-ef`): out of scope
  for v30; document.
- **Regex with shell metachars** (`[[ $s =~ \. ]]`): `\.` literal in the
  expanded pattern; works. Quoting in regex is identical to bash —
  `[[ $s =~ "literal" ]]` matches the literal string `literal`.
- **`[[ $x = bar ]]`** (single `=` instead of `==`): bash accepts both
  as string-equal. Include `=` as an alias for `==` for compat.
- **Nested `[[`**: `[[ [[ x ]] ]]` — bash disallows; we should too.
  Parser-level rejection.
- **Operator at LHS** (`[[ == foo ]]`): parser error — no LHS Word.

## Out of scope

- `-v var` (variable-set test) — bash 4.2+ feature; defer.
- `-nt` / `-ot` / `-ef` (file age/identity tests) — defer.
- Bash arrays (`[[ ${arr[i]} == ... ]]`) — arrays not implemented.
- POSIX character classes in regex (`[[:alpha:]]`) — Rust `regex` crate
  supports these natively, so they actually work. Bonus.
- Lookbehind/lookahead in regex (`(?=...)`, `(?<=...)`) — Rust `regex`
  doesn't support these; bash POSIX ERE also doesn't. Match.

## Tests

### Lexer (`src/lexer.rs::tests`)

| Test | Covers |
| --- | --- |
| `tokenize_double_bracket_open_at_word_start` | `[[` → keyword |
| `tokenize_double_bracket_close` | `]]` → keyword |
| `tokenize_double_bracket_not_at_word_start_is_literal` | `cmd[[foo]]` → Word |
| `tokenize_double_bracket_with_space_terminator` | `[[ ` recognised after whitespace |

### Parser (`src/command.rs::tests`)

| Test | Covers |
| --- | --- |
| `parse_dbracket_string_eq_literal` | `[[ a == b ]]` |
| `parse_dbracket_string_eq_pattern` | `[[ $f == *.txt ]]` — rhs unquoted |
| `parse_dbracket_string_ne` | `[[ x != y ]]` |
| `parse_dbracket_string_eq_single_equals` | `[[ a = b ]]` (bash alias) |
| `parse_dbracket_regex` | `[[ s =~ ^foo$ ]]` |
| `parse_dbracket_integer_compare` | `[[ 5 -eq 5 ]]`, `[[ 5 -gt 3 ]]` |
| `parse_dbracket_string_lex` | `[[ a < b ]]` |
| `parse_dbracket_unary_file` | `[[ -f /tmp ]]` |
| `parse_dbracket_unary_string_empty` | `[[ -z $x ]]` |
| `parse_dbracket_not` | `[[ ! -f /tmp/x ]]` |
| `parse_dbracket_and` | `[[ -f a && -r a ]]` |
| `parse_dbracket_or` | `[[ x == a || x == b ]]` |
| `parse_dbracket_grouped` | `[[ ( a || b ) && c ]]` |
| `parse_dbracket_empty_errors` | `[[ ]]` → ParseError |
| `parse_dbracket_unterminated_errors` | `[[ x == y` → ParseError |

### Integration (new `tests/double_bracket_integration.rs`)

| Test | Script | Expected |
| --- | --- | --- |
| `dbracket_string_eq_true` | `[[ hello == hello ]] && echo ok\nexit\n` | `ok` |
| `dbracket_string_eq_false_sets_status` | `[[ hello == world ]]; echo $?\nexit\n` | `1` |
| `dbracket_pattern_match_glob` | `[[ hello.txt == *.txt ]] && echo ok\nexit\n` | `ok` |
| `dbracket_quoted_rhs_is_literal` | `[[ hello.txt == "*.txt" ]] \|\| echo no\nexit\n` | `no` |
| `dbracket_regex_match` | `[[ hello42 =~ ^[a-z]+[0-9]+$ ]] && echo ok\nexit\n` | `ok` |
| `dbracket_regex_invalid_errors` | `[[ x =~ "[" ]]; echo $?\nexit\n` | `2` |
| `dbracket_int_eq` | `[[ 5 -eq 5 ]] && echo ok\nexit\n` | `ok` |
| `dbracket_int_gt` | `[[ 10 -gt 3 ]] && echo ok\nexit\n` | `ok` |
| `dbracket_int_bad` | `[[ abc -eq 5 ]]; echo $?\nexit\n` | `2` |
| `dbracket_file_test_existing` | `[[ -f /etc/hostname ]] && echo ok\nexit\n` | `ok` |
| `dbracket_file_test_missing` | `[[ ! -f /definitely/not/here ]] && echo ok\nexit\n` | `ok` |
| `dbracket_string_nonempty_z` | `[[ -z "" ]] && echo ok\nexit\n` | `ok` |
| `dbracket_string_nonempty_n` | `[[ -n hello ]] && echo ok\nexit\n` | `ok` |
| `dbracket_and_short_circuit` | `[[ -f /no/such && -r /no/such ]]; echo $?\nexit\n` | `1` (no error from second test) |
| `dbracket_or_short_circuit` | `[[ hello == hello \|\| -f /no/such ]] && echo ok\nexit\n` | `ok` |
| `dbracket_grouped_precedence` | `[[ ( a == a \|\| b == c ) && d == d ]] && echo ok\nexit\n` | `ok` |
| `dbracket_no_word_splitting` | `FOO="a b"\n[[ $FOO == "a b" ]] && echo ok\nexit\n` | `ok` |
| `dbracket_in_if` | `if [[ -f /etc/hostname ]]; then echo ok; fi\nexit\n` | `ok` |
| `dbracket_in_while` | (test that exits the loop after one iter) | works |
| `dbracket_chained_with_and` | `[[ a == a ]] && echo ok\nexit\n` | `ok` |
| `dbracket_with_inline_assignment` | `FOO=hi [[ $FOO == hi ]] && echo ok\nexit\n` | `ok` |
| `dbracket_in_subshell` | `([[ a == a ]]) && echo ok\nexit\n` | `ok` |

## Docs

- `docs/bash-divergences.md`: M-14 → `[fixed (2026-05-26)]` with notes:
  - Regex engine is Rust `regex` (RE2), not POSIX ERE. No lookbehind/lookahead.
  - Lex compare `<`/`>` is byte-order, not locale-aware (`LC_COLLATE` ignored).
  - `-v`/`-nt`/`-ot`/`-ef` and arrays out of scope.
  - Tier 2 count drops by 1.
- `README.md`: v30 row.

## Change log

- **2026-05-26**: Spec drafted; scope = full bash `[[ ]]` operator set
  via `regex` crate for `=~`. Pattern-vs-literal for `==`/`!=` via v21
  `expand_pattern`. No word-splitting / no pathname expansion on
  operands. Adds `regex` dependency.
