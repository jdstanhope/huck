# huck v93 — `$`-expansion in arithmetic + `declare -f/-F` silent Design

**Status:** approved design, ready for implementation plan.
**Implements (primary):** parameter expansion, special parameters, command
substitution, and array references inside arithmetic contexts — `(( ))`,
`$(( ))`, and C-style `for ((;;))` headers — so `(($# == 2))`,
`((i < ${#COMP_WORDS[@]} - 1))`, `$((${1#-a} + 2))`, `(( x = $(cmd) ))`, etc.
evaluate instead of raising `arithmetic '((...))': expected identifier after '$'`.
**Implements (bundled):** `declare -f` / `declare -F` on a missing function name
returns rc 1 with **no output** (bash-faithful), instead of printing
`declare: NAME: not found`.
**Closes:** **M-88** (currently `[deferred]` — re-prioritized from low; it is the
dominant blocker for sourcing `/usr/share/bash-completion/bash_completion`).
**Branch (impl):** `v93-arith-dollar-expansion` (created from `main` at plan time).

## Why this matters

Sourcing a real interactive `~/.bashrc` (bash-completion + mise + oh-my-posh)
showed M-88 as the top remaining cause of errors. The very first
bash-completion error is `(($# == 2))` (line 166) — huck's arithmetic tokenizer
only accepts `$` followed by `[A-Za-z0-9_]+`, so `$#`, `${…}`, `$(…)`, `$@` all
hit `expected identifier after '$'`, and each failed `(( ))` cascades into
spurious `unexpected else/fi/}`, `command not found: =`, and
`invalid function name`. oh-my-posh hits the same in `$(( ))`. Fixing M-88
clears that whole class. The bundled `declare -f/-F` fix clears mise's hook
probes (`declare -F _mise_hook >/dev/null` etc.).

## Root cause

Array subscripts already work correctly: `eval_subscript` (`src/expand.rs:120`)
runs `expand_word_to_string(subscript, shell)` **then** `arith::parse` —
expand-then-parse at eval time. But `(( ))`, `$(( ))`, and arith-`for` headers
pre-parse to an `ArithExpr` **at lex/parse time** (`$(( ))` at `src/lexer.rs:1116`,
`(( ))` at `src/command.rs:733`), where no `Shell` exists, so `$`-forms are never
expanded and the arith lexer's `$[A-Za-z0-9_]+` hack (`src/arith.rs:144`) is all
there is. The fix: defer arith parsing for these three sites to eval time,
mirroring `eval_subscript`.

## Verified bash 5.2 semantics (the contract)

- `set -- a b; (($# == 2)) && echo Y` → `Y`.
- `a=(x y z); ((${#a[@]} == 3)) && echo Y` → `Y`.
- `i=1; ((i < ${#a[@]} - 1)) && echo Y` (with `a=(x y z)`) → `Y`.
- `$((${1#-a} + 2))` with `set -- -a5` → `7` (param expansion inside `$(( ))`).
- `x=$(( $(echo 3) * 4 )); echo $x` → `12` (command substitution inside `$(( ))`).
- `for ((i = 0; i < ${#a[@]}; i++)); do echo $i; done` → `0 1 2`.
- `n=5; echo $((n))` → `5`; `echo $((n + 1))` → `6` (bare identifier still works).
- `(( x == "5" ))` with `x=5` → true (quote removal: the operand becomes `5`).
- Empty after expansion: `e=; echo $(( e ))` → `0`; `$(( ))` → `0` (empty arith = 0).
- `declare -f no_such_fn; echo $?` → rc `1`, **no output**.
- `declare -F no_such_fn; echo $?` → rc `1`, **no output**.
- `declare -p UNSET; echo $?` → rc `1`, prints `bash: declare: UNSET: not found`
  (UNCHANGED — huck already matches; `-p` is not silenced).

## Section 1 — Architecture: expand-then-parse

Bash treats an arithmetic expression as if within double quotes: all tokens
undergo parameter expansion, command substitution, and quote removal, and the
result is evaluated as arithmetic where a bare `name` is a variable reference
(resolved recursively by the evaluator). huck's `arith::eval` already resolves
bare identifiers via the shell. The only missing step is the expansion pass in
front — which `eval_subscript` already performs. v93 routes the three remaining
arith sites through the same expand-then-parse path.

## Section 2 — Carriers (lexer + AST)

Three carriers stop storing a pre-parsed `ArithExpr` and instead store an
expandable `Word` (`Vec<WordPart>`):

| Site | Before | After |
|------|--------|-------|
| `$(( ))` expansion | `WordPart::Arith { expr: ArithExpr, quoted }` (`src/lexer.rs:138`) | `WordPart::Arith { body: Word, quoted }` |
| standalone `(( ))` | `Token::ArithBlock(String)` (`src/lexer.rs:184`) → `Command::Arith(ArithExpr)` (`src/command.rs:735`) | `Token::ArithBlock(Word)` → `Command::Arith(Word)` |
| C-style `for ((;;))` | `ArithForClause { init/cond/step: Option<ArithExpr> }` (`src/command.rs:492`) | `ArithForClause { init/cond/step: Option<Word> }` |

**Lexer.** `scan_arith_body` (the `$(( ))` body, `src/lexer.rs:1115`) and
`scan_arith_block` (the standalone `(( ))` body, `src/lexer.rs:1311`) currently
return a raw `String` that is immediately `arith::parse`d. They will instead
build a `Word`: keep the existing paren-depth counting to find the matching
`))`, but recognize `$name` / `${…}` / `$(…)` / `` `…` `` / `$((…))` as proper
`WordPart`s (reusing the existing double-quote-context scanning helpers the
lexer already uses for `"…"` content), with all other characters
(`<`, `>`, `*`, spaces, `==`, digits, operators) accumulated as
`WordPart::Literal { quoted: false }`. The eager `arith::parse` calls at
`src/lexer.rs:1116` and `src/command.rs:733` are removed.

**Arith-`for` header.** The header text (between `for ((` and `))`) is split on
top-level `;` into three segments; each non-empty segment is scanned into a
`Word` the same way. (The lexer/parser path that currently produces
`ArithForHeader`/`ArithForClause` is adjusted to carry Words; `src/command.rs`
around lines 492-495, 1061-1063.)

**Continuation.** `UnterminatedArithBlock` (EOF before `))`, `src/lexer.rs:22`)
and the arith-for-header continuation remain lex-level signals and are
unaffected. `ParseError::ArithBlock` (`src/command.rs:560`) and
`ParseError::ArithForHeader` (`:563`) were raised when `arith::parse` failed at
parse time; with parsing deferred, a malformed arith expression now surfaces as
a **runtime** error from `eval_arith_word` (matching bash, which also reports
arithmetic syntax errors at evaluation time). These two `ParseError` variants
and their `continuation::classify` handling are removed if they become unused,
or retained only if still produced elsewhere — the implementer verifies via the
compiler and the continuation tests.

## Section 3 — Eval + arith-lexer cleanup

New helper (in `src/expand.rs` or `src/executor.rs`, beside `eval_subscript`):

```rust
/// Bash-faithful arithmetic evaluation of an arith body Word: expand all
/// `$`-forms + quotes first (as `eval_subscript` does for subscripts), then
/// parse and evaluate. An empty/all-whitespace expansion is `0` (bash:
/// `$(())` == 0). Bare identifiers in the expanded string are resolved as
/// variables by `arith::eval`.
fn eval_arith_word(body: &Word, shell: &mut Shell) -> Result<i64, crate::arith::ArithError>;
```

Implementation: `let s = expand_word_to_string(body, shell); let t = s.trim();
if t.is_empty() { return Ok(0); } let e = crate::arith::parse(t)?;
crate::arith::eval(&e, shell)`.

Wire it into the four eval sites that previously used a pre-parsed `ArithExpr`:
- `run_arith` (`src/executor.rs:476`) — `((expr))` command (0 ⇒ `Continue(1)`,
  nonzero ⇒ `Continue(0)`; parse/eval error ⇒ `eprintln!("huck: ((: {e}")` +
  `Continue(1)`, unchanged).
- `run_arith_for_inner` (`src/executor.rs`) — init/cond/step; a `None` segment
  keeps its current meaning (init/step skipped, cond ⇒ always true).
- The two `$(( ))` expansion sites (`src/expand.rs:666`, `:796`) — same
  error-to-empty-field + status behavior they have today.

**Arith lexer cleanup.** The `$`-branch in `arith::tokenize` (`src/arith.rs:144`)
becomes unreachable from these four paths (their strings are `$`-free after
expansion) and from `eval_subscript` (already `$`-free). It is retained as
defensive code with a doc comment marking it effectively dead, OR removed if no
caller can produce a `$` — the implementer decides based on the compiler and a
search for `arith::parse` callers. No change to `arith::eval`'s identifier
resolution.

## Section 4 — `declare -f` / `declare -F` silent-on-missing

In `builtin_declare` (`src/builtins.rs`), the `-f` (print function body) and
`-F` (print function name) paths: when a requested NAME is not a defined
function, do **not** print `declare: NAME: not found`; just contribute rc 1
(overall declare exit status is 1 if any name was missing). Existing behavior
for defined functions (printing the body / name) and for `-p` on unset
variables (which keeps printing `declare: NAME: not found` to match bash) is
unchanged.

## Files & responsibilities

| File | Change |
|------|--------|
| `src/lexer.rs` | `WordPart::Arith` carries `body: Word`; `Token::ArithBlock(Word)`; `scan_arith_body`/`scan_arith_block` build Words (no eager `arith::parse`) |
| `src/command.rs` | `Command::Arith(Word)`; `ArithForClause` init/cond/step `Option<Word>`; arith-for header split into Words; remove dead `ParseError::ArithBlock`/`ArithForHeader` if unused |
| `src/executor.rs` | new `eval_arith_word`; `run_arith` + `run_arith_for_inner` use it |
| `src/expand.rs` | the two `$(( ))` expansion sites call `eval_arith_word` on the body Word |
| `src/arith.rs` | document/remove the now-dead `$`-branch in `tokenize` |
| `src/builtins.rs` | `declare -f`/`-F` silent on missing function |
| `src/continuation.rs` | adjust only if a removed `ParseError` variant was referenced |
| `tests/arith_dollar_integration.rs` | NEW — `$`-forms in `(( ))`/`$(( ))`/arith-for |
| `tests/scripts/arith_dollar_diff_check.sh` | NEW — 19th bash-diff harness (bash_completion idioms) |
| `tests/declare_silent_*` (integration + harness fragments) | `-f`/`-F` silent |
| `docs/bash-divergences.md`, `README.md` | M-88 `[fixed v93]` (re-prioritized); declare note; **NEW M-90 deferred: builtin error output ignores `2>` redirection** (every builtin uses `eprintln!`); changelog; README row |

## Testing

1. **Unit** (`src/arith.rs` and/or parser): a `Word`-bearing arith body expands
   then evaluates; `(( ))` empty body ⇒ 0.
2. **Integration** (`tests/arith_dollar_integration.rs`): `(($# == 2))`,
   `((${#a[@]} == 3))`, `for ((i=0; i<${#a[@]}; i++))`, `$((${1#-a}+2))`,
   `x=$(( $(echo 3) * 4 ))`, `(( x == "5" ))`, empty-expansion ⇒ 0, bare
   identifier still works.
3. **bash-diff harness** `tests/scripts/arith_dollar_diff_check.sh` (huck's
   19th): the real bash_completion idioms above, each byte-identical to bash 5.2.
4. **declare**: integration + harness fragments — `declare -f MISSING`/`-F
   MISSING` (rc 1, no output) vs `declare -F EXISTING` (prints) vs `declare -p
   UNSET` (still prints, matches bash).
5. **Regression**: confirm the `(( ))`-failure cascade
   (`unexpected else/fi/}`, `command not found: =`) disappears for the
   bash_completion fragment that previously triggered it.

## Newly-discovered deferral to record (docs-only, NOT v93 code)

- **M-90: builtin error output ignores `2>` redirection** — `[deferred]`
  (high). Every huck builtin writes diagnostics via `eprintln!` to the process's
  real stderr rather than through a redirectable error sink, so
  `cmd … 2>/dev/null` does not suppress builtin error messages
  (e.g. `declare -p UNSET 2>/dev/null` still prints). bash routes builtin stderr
  through the command's fd 2. Fixing this requires threading an error sink
  through all builtins — a broad refactor, out of scope for v93. (This is why
  mise's `declare -p PROMPT_COMMAND 2>/dev/null` leaked even though `-p` itself
  matches bash.)

## Edge cases & notes

- **Quote removal**: handled by `expand_word_to_string` — `(( x == "5" ))`
  becomes `x == 5`. A literal `"` with no arithmetic meaning is removed, matching
  bash's "treated as if within double quotes" rule.
- **`$(( ))` error semantics** are preserved at the existing expansion sites
  (division by zero, etc. → empty field + status), now surfaced through
  `eval_arith_word`'s `Result`.
- **`(( ))` runtime syntax errors**: a malformed expression after expansion
  (e.g. `(( + ))`) now errors at eval time via `eprintln!("huck: ((: {e}")` +
  status 1, matching bash's evaluation-time arithmetic syntax errors.
- **`**` / bitwise / ternary** and all existing arith operators are unaffected —
  only the front-end expansion changes; `arith::parse`/`eval` are reused as-is.
- **No regression for `$`-free arithmetic**: `$((1+2))`, `((i++))`, `for
  ((i=0;i<10;i++))` expand to themselves (no `$`-forms) and parse/eval exactly
  as before.
