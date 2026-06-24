# v216 — Bash error-prologue foundation + arith error-text slice

## Status

Design approved 2026-06-24.

## Background

Running bash 5.2.21's own `arith` test-suite category through huck (the
v214 harness) leaves the category FAILing. v215 resolved the `set -o
posix` cascade so the suite runs end-to-end; what remains splits into
two kinds of divergence:

1. **Error-text format** (pervasive): bash prints arithmetic errors as

   ```
   ./arith.tests: line 168: 7 = 43 : attempted assignment to non-variable (error token is "= 43 ")
   ```

   huck prints

   ```
   huck: arithmetic: assignment requires variable on LHS
   ```

2. **Behavioral**: cases where bash produces a *value* and huck errors
   (or vice versa) — `++7`/`--7`, dead-branch lazy evaluation, array-
   element lvalues, integer-literal overflow wrapping, substring
   offset/length ternary colons.

This iteration addresses **(1) only**, and only for arithmetic. The
behavioral divergences (2) are explicitly deferred (see "Out of scope").

### Why the prefix is a foundation, not an arith detail

bash's error prefix is uniform across every error path. The prologue is

```
<name>: [line <N>: ][<cmdname>: ]
```

where

- **`<name>`** = `BASH_SOURCE[0]` (if set and non-empty) else `$0` when
  the shell is **non-interactive**; otherwise the shell basename
  (`bash`). Source: `get_name_for_error()` in bash `error.c`.
- **`line <N>: `** appears only when **non-interactive** (and, for
  `error_prolog`, only when the line number is `> 0`). `<N>` is the
  current executing line. Source: `error_prolog()` / the builtin
  variant `builtin_error_prolog()`.
- **`<cmdname>: `** is the command context (`cd`, `let`, `((`, …). For
  `internal_error` (the arith path) the command name is supplied by the
  caller's message body (`evalerror`), not the generic prologue; for
  builtins it is appended by `builtin_error_prolog()`.

huck instead emits a flat `huck: ` prologue at ~419 sites, with the
command-name tail already matching bash (`huck: cd: msg` → tail
`cd: msg` is byte-identical to bash's tail). **Only the prologue
differs.** Converting all 419 sites with faithful `$0`/`BASH_SOURCE`/
lineno/interactive plumbing is too large for one spec, so the
shell-wide adoption is decomposed into a multi-iteration program:

- **v216 (this spec):** build the prologue mechanism and convert the
  **arith** error sites end-to-end, including the arith-specific
  expression-echo and `(error token is "…")` suffix.
- **v217+:** convert remaining categories slice by slice (builtins,
  parser/syntax, command-not-found/execution, redirections), each its
  own iteration with its own `*_diff_check.sh`.

## Goals

- A reusable `Shell::error_prefix` helper mirroring bash's
  `get_name_for_error` + `error_prolog` semantics.
- Arith error output that byte-matches bash for the cases where **both**
  shells error and differ only in text — prologue, leading-trimmed
  expression echo, message wording, and `(error token is "…")` suffix.
- A `tests/scripts/arith_error_diff_check.sh` gold-standard harness over
  a curated set of arith error fragments.
- No churn to interactive-mode behavior or interactive tests.

## Non-goals / Out of scope

The following are **behavioral** divergences. Error-text changes cannot
close their diff lines because bash emits a value, not an error. They
remain divergent and get (or keep) a `[deferred]` entry:

- `++7` / `--7` → bash yields `7` (treats leading `++`/`--` before a
  non-lvalue as repeated unary `+`/`-`); huck errors.
- Dead-branch lazy evaluation (`((0?x:0))` must not evaluate `x`) —
  `arith8.sub`.
- Array-element lvalues inside arithmetic (`(a[n]=n++)`, `0?arr[a=1]:0`).
- Integer-literal overflow wrapping (literals ≥ 2⁶³ wrap two's-
  complement in bash; huck rejects as out-of-range) — `arith5.sub`.
- Substring offset/length with arith ternary colons
  (`${PARAM:1?4:2:1}`) — `arith7.sub`.

The full `arith` test-suite category will therefore still FAIL after
v216; the prefix/wording diff lines close, the behavioral lines remain.

The other ~400 non-arith `huck:` error sites are unchanged in v216.

## Design

### 1. The error-prologue helper (`shell_state.rs`)

```rust
/// Bash-compatible error prologue: `<name>: [line N: ][cmd: ]`.
/// `cmd` is the command context (`let`, `((`, …) or None for `$(( ))`.
pub fn error_prefix(&self, cmd: Option<&str>) -> String
```

Logic (mirrors bash):

- `name`:
  - if `!self.is_interactive`: `BASH_SOURCE[0]` if present and
    non-empty, else `self.shell_argv0` (`$0`).
  - else: `"huck"` (bash's interactive analog is the shell basename
    `bash`; huck's own name is the faithful equivalent).
- `line`: included only when `!self.is_interactive` **and**
  `self.current_lineno > 0`, rendered as `line {N}: `.
- `cmd`: when `Some(c)`, append `{c}: `.

Examples:

| context | interactive | output |
|---|---|---|
| `$(( ))` in `./s.sh` line 5 | no | `./s.sh: line 5: ` |
| `let` in `./s.sh` line 9 | no | `./s.sh: line 9: let: ` |
| `(( ))` in `./s.sh` line 3 | no | `./s.sh: line 3: ((: ` |
| `$(( ))` at REPL | yes | `huck: ` |
| `let` at REPL | yes | `huck: let: ` |

Note: `BASH_SOURCE[0]` and `$0` for the top-level script both resolve
to the invoked path (`./arith.tests`); bash uses the path verbatim, not
the basename.

### 2. Arith token-offset & error tracking (`arith.rs`)

bash's `evalerror` prints, after the prologue:

```
<expr>: <msg> (error token is "<tok>")
```

- **`<expr>`** = the arith source string with **leading** whitespace
  trimmed (bash: `for (t = expression; whitespace(*t); t++)`). Trailing
  whitespace is preserved (`7 = 43 `).
- **`<tok>`** = `source[start_byte_of_most_recent_token ..]` — the tail
  of the source from where the last token read began (bash's `lasttp`
  pointer into `expression`). Empty string if no token was consumed.

Implementation steps:

1. `tokenize` returns a byte offset alongside each `ArithToken` (the
   start offset of that token in the input string). Tokenize-time errors
   (invalid base, value-too-great-for-base, invalid integer constant,
   invalid number) carry the offending token's start offset.
2. `ArithError` gains an optional `token_offset: Option<usize>` (the
   `lasttp` analog). The parser sets it to the start offset of the
   current/most-recently-consumed token when it raises an error.
3. A formatting entry point assembles, given the original source string
   and the command context:
   `format!("{prefix}{expr}: {msg} (error token is \"{tok}\")")`
   where `prefix = shell.error_prefix(cmd)`, `expr` = leading-trimmed
   source, `tok = &source[off..]` (or `""`).
4. The arith emission call sites (`expand.rs` `$(( ))`, `param_expansion.rs`
   substring offsets/length, `executor.rs` `run_arith` for `(( ))`, the
   `let` builtin) pass the original source string + the right command
   context (`None` / `"let"` / `"(("`).

### 3. Message-wording map

Convert huck's arith message text to bash's wording for the in-scope
(both-error) cases:

| huck (current) | bash 5.2.21 |
|---|---|
| `assignment requires variable on LHS` | `attempted assignment to non-variable` |
| `division by zero` | `division by 0` |
| `base must be 2-64, got N` | `invalid arithmetic base` |
| `base-B literal requires at least one digit` (`2#`) | `invalid integer constant` |
| `invalid digit for base B: 'X'` (`2#44`) | `value too great for base` |
| `expected ')', got None` | `` missing `)' `` |
| missing-operand / `expected expression, got <EOF>` | `syntax error: operand expected` |
| `expected expression, got Colon` (`4 ? : 3+5`) | `expression expected` |
| `expected ':' in ternary, got …` | `` `:' expected for conditional expression `` |
| trailing junk / `unexpected token after expression` | `syntax error in expression` |
| `0#4`, `2#110#11` (bad based-number) | `invalid number` |

Notes:

- bash's `division by 0` text applies to both `/` and `%` by zero.
- `invalid number` is bash's catch-all for a malformed `base#digits`
  token that is neither "invalid arithmetic base" nor "value too great
  for base" (e.g. a `#` appearing where the leading number is not a
  valid base specifier, or a second `#`).
- Where huck currently has a message with no clean bash analog and the
  case is behavioral (out of scope), the message is left unchanged.
- The exact bash messages are confirmed against the captured
  `arith.diff` from the v214 harness run, not copied from GPL'd
  `.right` files into this repo.

### 4. Testing

- **`tests/scripts/arith_error_diff_check.sh`** — a new gold-standard
  harness: a curated list of arith error fragments, each run through
  both `bash` and `huck` (invoked as a script file so non-interactive
  mode and a known `$0` apply), asserting byte-identical **stderr**.
  Fragments cover: `$(( 7 = 43 ))`, `$(( 44 / 0 ))`, `$(( 2# ))`,
  `$(( 2#44 ))`, `$(( 3425#56 ))`, `let 'rv = 7 + (43 * 6'`,
  `$(( 4 ? : 3 + 5 ))`, `(( a b ))`, and the `let`/`(( ))` command
  contexts — i.e. the both-error cases only.
- **Unit tests** in `arith.rs` that assert the old `huck: arithmetic:`
  text are updated to the new format (offset tracking, token tail,
  bash wording).
- **Interactive** arith errors keep the `huck:`-style prologue; no
  interactive-test changes expected.
- The v214 `arith` category in `docs/bash-test-suite-baseline.md` is
  re-triaged: its Note is updated to record that prefix/wording lines
  now match and the remaining failures are the deferred behavioral
  cases.

## Risks

- **Line-number accuracy.** `current_lineno` must equal bash's
  `executing_line_number()` for each fragment. For one-statement-per-
  line scripts this holds; multi-line `let`/`(( ))` constructs may need
  spot-checking. The curated harness uses simple one-per-line fragments
  to keep this deterministic.
- **Token-tail exactness.** The `(error token is "…")` byte content
  (including preserved trailing whitespace) depends on offset tracking
  matching bash's `lasttp`. The harness is the guard; any residual
  mismatch is narrowed there.
- **Scope creep into behavioral fixes.** The wording map must not
  silently "fix" a behavioral case. Each fragment in the harness is a
  both-error case; behavioral cases stay out.

## Divergence-doc bookkeeping

- Add a `[deferred]` entry capturing the remaining arith **behavioral**
  divergences (overflow wrapping, `++`/`--` on non-lvalues, lazy dead-
  branch eval, array-element lvalues, substring ternary colons) so the
  open work is tracked.
- L-55 (arith errors in `-c` mode continue) is unaffected and stays.
- Note in `docs/architecture.md` (error-reporting / "where to add"
  area) that `Shell::error_prefix` is the bash-compatible prologue and
  that shell-wide adoption is staged.
