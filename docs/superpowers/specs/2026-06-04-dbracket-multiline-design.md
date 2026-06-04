# huck v87 — multi-line `[[ ]]` + missing test operators Design

**Status:** approved design, ready for implementation plan.
**Implements:** (1) multi-line `[[ … ]]` continuation — a `[[` whose `]]` is on a
later physical line (or whose expression is line-broken after `[[`, an operand,
or `&&`/`||`) now gathers continuation lines instead of erroring; (2) the four
test operators M-14 left out — `-v` (variable-is-set), `-nt`/`-ot`/`-ef` (file
age / identity) — in **both** `[[ … ]]` and the `test`/`[` builtin.
**Discovered:** loading a stock Debian `~/.bashrc`, which sources
`/usr/share/bash-completion/bash_completion` (uses line-broken `[[ … ]]`
conditions and `-v`).
**Divergence tracker:** extend **M-14** (operators no longer out-of-scope) + new
sub-entry **M-14a** `[fixed v87]`.
**Branch (impl):** `v87-dbracket-multiline` (created from `main` at plan time).

## Scope

Two decisions were made during brainstorming:

1. **Continuation + missing operators** (not continuation-only): v87 fixes
   multi-line `[[ ]]` AND adds `-v`/`-nt`/`-ot`/`-ef`.
2. **Both `[[ ]]` and `test`/`[`**: the four operators are added to both
   constructs for a consistent operator set, matching bash.

**Out of scope** (remain deferred): other bash-completion load errors not about
`[[` (the `complete -A <action>` gaps, `set -v` verbose mode); `[[ ]]` with a
backslash-continued line *inside* an operand (rare; the generic `\`-continuation
already handles trailing-backslash); `-v arr[i]` array-element form (see
"`-v` operand scope" below). The existing M-14 regex/locale divergences (L-09,
no `LC_COLLATE`) are unchanged.

## Background: how continuation works today

huck's REPL reader (`read_logical_command`, `src/shell.rs`) gathers continuation
lines by calling `continuation::classify(buffer)`. `classify`
(`src/continuation.rs`) is **parse-error-driven**:

1. trailing-backslash → `Incomplete(Backslash)`;
2. tokenize → on an "unterminated" `LexError` → `Incomplete(OpenQuote)`/`Heredoc`;
3. trailing `|`/`&&`/`||` operator token → `Incomplete(Operator)`;
4. `command::parse(tokens)` → maps `ParseError::Unterminated{If,Loop,Case,Brace,
   Function,Subshell}` → `Incomplete(Compound|Subshell)`; **any other parse
   error → `Error`**.

`ParseError::UnterminatedDoubleBracket` exists but is **not** in the step-4
mapping, so an unclosed `[[` becomes `Error` and the partial line is parsed,
producing the observed failures:

- `[[ -f x` (then `]]` next line) → `UnterminatedDoubleBracket` → currently
  `Error` → "unterminated '[[ ]]'" then "unexpected ']]'".
- `[[ -f x &&` (then `-f y ]]` next line) → the `[[` parser hits end-of-tokens
  while expecting the `&&` RHS and returns `TestExprMissingOperand` → "missing
  operand in '[[ ]]'".

A **manually single-lined** `[[ -f x && -f y ]]` already parses and runs (M-14);
trailing-`&&` continuation already works in non-`[[` contexts. So the multi-line
defect is entirely in (a) the parser's EOF-inside-`[[` error classification and
(b) the classifier not mapping `UnterminatedDoubleBracket`.

## Part 1 — Multi-line `[[ ]]` continuation

### Parser (`src/command.rs`)

In the `[[` expression parser (the Pratt-style parser starting near the
`is_test_terminator` helper / `parse_double_bracket`), change every point that
consumes a token while parsing the conditional so that **encountering
end-of-input (`iter.peek() == None`) while an operand, operator, or closing `]]`
is still expected returns `ParseError::UnterminatedDoubleBracket`**, instead of
`TestExprMissingOperand` or `TestExprBadOperator`.

Concretely, the distinction is:

- **Input exhausted (`None`) before `]]` is consumed → `UnterminatedDoubleBracket`.**
  Covers: `[[` alone, `[[ -f x`, `[[ -f x &&`, `[[ a ==`, `[[ ( a == b`.
- **A real terminator (`]]` or `)`) is present but an operand is missing →
  keep `TestExprMissingOperand`.** Covers: `[[ x == ]]`, `[[ -f ]]`, `[[ ]]`
  (the existing `EmptyDoubleBracket`/`TestExprMissingOperand` paths are
  unchanged — these are genuine, non-recoverable errors that bash also rejects).

This single rule covers every line-break position bash allows: after `[[`, after
an operand, after `&&`/`||`, after `(`, and immediately before `]]`.

`echo [[` is unaffected: `[[` there is an ordinary argument Word (not at
`[[`-command position), so `parse` returns a valid simple command → `Complete`.
We deliberately do **not** count `[[`/`]]` token depth in the classifier (which
would misclassify `echo [[`).

### Classifier (`src/continuation.rs`)

- Add `ContinuationReason::DoubleBracket`.
- In `classify`'s step-4 match, add `ParseError::UnterminatedDoubleBracket =>
  Completeness::Incomplete(ContinuationReason::DoubleBracket)`.
- `joiner_for(ContinuationReason::DoubleBracket, _) => " "` (single space).
  Space-joining is correct for every break position (operands and operators are
  whitespace-separated inside `[[ ]]`); it never introduces a `;` (which `Compound`'s
  joiner would, breaking the expression).

### End-of-input behavior

When a script (or `-c`/piped input) ends mid-`[[`, the REPL reader gets EOF while
`classify` says `Incomplete(DoubleBracket)`. The reader's existing
EOF-while-incomplete path (already used for unterminated `if`/quotes) surfaces the
final parse error (`unterminated '[[ ]]'`), matching bash's "unexpected EOF". No
new reader logic is required beyond the classifier mapping.

## Part 2 — The four missing operators

All evaluation logic is centralized in `src/test_builtin.rs`, which `[[`'s unary
file-tests already delegate to (`eval_unary`, `src/executor.rs`). The `[[` parser
keeps its own operator tables (`TestUnaryOp`/`TestBinaryOp` in `src/command.rs`)
and its `eval_binary` (`src/executor.rs`), delegating the new file-comparison
binaries to `test_builtin` so the semantics live in one place.

### `-nt` / `-ot` / `-ef` (binary; pure filesystem)

bash 5.2 semantics (verified), modeling a **missing file as the oldest possible
mtime**:

- `f1 -nt f2` → true if `f1` is newer than `f2` **or `f1` exists and `f2` does
  not**. (Both missing → false; `f1` missing → false.)
- `f1 -ot f2` → true if `f1` is older than `f2` **or `f2` exists and `f1` does
  not**. (Both missing → false; `f2` missing → false.)
- `f1 -ef f2` → true if `f1` and `f2` are the same file (same device + inode);
  **both must exist** (any missing → false).

Implementation:

- `src/test_builtin.rs`: add `"-nt" | "-ot" | "-ef"` to `is_binary_op`; in
  `apply_binary`, compute via `std::fs::metadata` (`.modified()` for age;
  `std::os::unix::fs::MetadataExt` `.dev()`/`.ino()` for `-ef`). A `metadata`
  error (missing file) feeds the existence rules above (treat as "no mtime").
- `src/command.rs`: add `TestBinaryOp::{NewerThan, OlderThan, SameFile}`; map
  `"-nt"`/`"-ot"`/`"-ef"` in the binary-operator parse path.
- `src/executor.rs` `eval_binary`: new arms for the three ops that expand both
  operand words (no glob/field-split, per `[[` rules — same expansion the other
  binary ops use) and delegate to `test_builtin::evaluate(&[lhs, op, rhs], …)`
  (or a small shared `compare_files(op, lhs, rhs)` helper in `test_builtin`).

These ops need no shell.

### `-v name` (unary; variable-is-set)

bash 5.2 semantics (verified): true iff the named variable is **set** (a
set-but-empty variable is true; unset is false). Positional parameters work
(`-v 1` true iff `$1` is set). The operand undergoes the normal `[[`/`test`
operand expansion (so `-v $x` checks the variable named by `$x`).

New shell helper:

- `src/shell_state.rs`: `pub fn is_set(&self, name: &str) -> bool` — single source
  of truth for set-ness:
  - a positional index (all-ASCII-digits, `n >= 1`) → `n <= positional_args.len()`;
  - the special params huck always defines (`0`, `$`, `#`, `-`) → true;
    `!` → true only if a background job has run (mirrors bash's "set after first
    `&`"); keep this minimal — these are edge cases bash-completion never probes;
  - otherwise → `self.vars.contains_key(name)` (set-but-empty ⇒ true).

#### `-v` operand scope

v87 supports **scalar variable names and positional parameters**. The
array-element form `-v arr[i]` is **deferred** (documented as a new low-priority
**M-14b**): an unparsed-subscript name simply falls through to
`vars.contains_key("arr[i]")` → false, which is acceptable (bash-completion's
`-v` uses are plain names). If the implementer finds the existing
indexed/associative element-exists machinery trivially reachable from `is_set`,
including it is welcome but not required.

Wiring:

- **`[[` (`src/command.rs` + `src/executor.rs`)**: add `TestUnaryOp::VarSet`
  (`"-v"`); in `eval_test_expr`'s `Unary` arm (which has `&Shell`), evaluate
  `VarSet` via `shell.is_set(&expanded_operand)` directly, *before* delegating the
  other unary ops to `eval_unary`. `eval_unary` itself does **not** implement
  `VarSet` — but it does gain a `&Shell` parameter purely to forward to
  `test_builtin::evaluate` (see the `test`/`[` wiring below; `evaluate` has one
  signature that takes `&Shell` for its own `-v` arm).
- **`test`/`[` (`src/builtins.rs` + `src/test_builtin.rs`)**: `-v` needs shell,
  which `builtin_test`/`test_builtin::evaluate` currently lack. Thread an
  immutable shell reference through:
  - dispatch `"test" | "[" => builtin_test(name, args, shell)` (`src/builtins.rs`);
  - `builtin_test(name, args, shell)` → `test_builtin::evaluate(args, shell)`;
  - `test_builtin::evaluate`, `evaluate_short_form`, and the recursive-descent
    `Parser` carry the `&Shell`; add `"-v"` to `is_unary_op`; the `-v` arm of
    `apply_unary` calls `shell.is_set(operand)`. The fs/string ops ignore the
    shell parameter.
  - One `evaluate` signature: `test_builtin::evaluate(args, shell: &Shell)`.
    `eval_unary` (`src/executor.rs`) gains a `&Shell` parameter and forwards it on
    its delegated `test_builtin::evaluate(&[op, operand], shell)` calls;
    `eval_test_expr` already holds the shell and passes it to `eval_unary`. The
    `[[` `VarSet` op never reaches `test_builtin` (intercepted in `eval_test_expr`);
    only the `test`/`[` path exercises `test_builtin`'s `-v` arm.

## Files & responsibilities

| File | Change |
|------|--------|
| `src/command.rs` | `[[` parser: EOF-inside-`[[` → `UnterminatedDoubleBracket`; `TestUnaryOp::VarSet`; `TestBinaryOp::{NewerThan,OlderThan,SameFile}`; parse mappings for `-v`/`-nt`/`-ot`/`-ef` |
| `src/continuation.rs` | `ContinuationReason::DoubleBracket`; map `UnterminatedDoubleBracket`→`Incomplete`; `joiner_for`→`" "` |
| `src/shell_state.rs` | `Shell::is_set(name) -> bool` |
| `src/executor.rs` | `eval_test_expr` `VarSet` via `shell.is_set`; `eval_binary` `-nt`/`-ot`/`-ef` arms (delegate to `test_builtin`); thread `&Shell` into `eval_unary` |
| `src/test_builtin.rs` | `evaluate`/`Parser` carry `&Shell`; `-v` in `is_unary_op`+`apply_unary` (via `shell.is_set`); `-nt`/`-ot`/`-ef` in `is_binary_op`+`apply_binary`; a `compare_files` helper |
| `src/builtins.rs` | `builtin_test(name, args, shell)` + dispatch passes `shell` |
| `tests/dbracket_multiline_integration.rs` | NEW — binary-driven integration tests |
| `tests/scripts/dbracket_multiline_diff_check.sh` | NEW — huck's 14th bash-diff harness |
| `docs/bash-divergences.md`, `README.md` | M-14 update + M-14a `[fixed v87]` (+ M-14b deferred for `-v arr[i]`); changelog; README v87 row |

## Testing

1. **Unit tests**:
   - `src/continuation.rs`: `[[ -f x` → `Incomplete(DoubleBracket)`; `[[ a &&` →
     `Incomplete(DoubleBracket)`; `[[ a == b ]]` → `Complete`; `[[ x == ]]` →
     `Error` (real missing operand, not Incomplete); `echo [[` → `Complete`;
     `joiner_for(DoubleBracket, _)` == `" "`.
   - `src/command.rs`: parse `[[ -v X ]]` → `Unary{VarSet}`; `[[ a -nt b ]]` →
     `Binary{NewerThan}`; EOF cases (`[[ -f x`, `[[ a &&`) → `UnterminatedDoubleBracket`.
   - `src/shell_state.rs`: `is_set` for set, set-empty, unset, positional set/unset.
   - `src/test_builtin.rs`: `-nt`/`-ot`/`-ef` existence-edge truth table; `-v`.
2. **Integration** (`tests/dbracket_multiline_integration.rs`): multi-line `[[`
   broken after `[[`, after an operand, after `&&`, before `]]`; a `[[ … ]] && {`
   block split across lines; `-v` set/unset and `-nt`/`-ot`/`-ef` in both
   `[[ … ]]` and `[ … ]` (fixture files with `touch -d` controlled mtimes + a
   hard link for `-ef`); EOF-mid-`[[` in `-c` mode → unterminated error + nonzero.
3. **bash-diff harness** `tests/scripts/dbracket_multiline_diff_check.sh`
   (huck's 14th), byte-identical to bash 5.2: the multi-line fragments and the
   four operators (the harness `touch -d`s two files with known, different
   mtimes + makes a hard link, in a `mktemp -d` fixture). Multi-line fragments
   are fed as real multi-line program text on stdin so both shells exercise their
   continuation readers identically.

## Edge cases & notes

- `[[ x == ]]` / `[[ -f ]]` / `[[ ]]` stay errors (terminator present, operand
  missing) — never trigger continuation.
- `echo [[`, `x=[[`, `grep '[[' f` stay `Complete` (the `[[` is not at
  command position / is quoted).
- `[[ a -nt b ]]` operand expansion follows the existing `[[` rule (parameter/
  command/arith expansion, **no** word-splitting or globbing).
- `-v`'s set-but-empty ⇒ true distinction relies on `vars.contains_key`, not
  `lookup_var` (which returns `Some("")` for empty but also resolves specials).
- Multi-line continuation composes with existing reasons: a `[[` inside an `if`
  body still works because the unclosed `[[` is detected first (its parse error
  surfaces before the `if` completes).
