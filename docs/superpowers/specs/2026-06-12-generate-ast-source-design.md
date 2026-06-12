# huck v146 — AST→source `generate` module (`declare -f`) Design

**Status:** approved design, ready for implementation plan.
**Implements:** a `generate` module that renders a parsed `Command` AST back to
normalized, re-parseable shell source. Wires `declare -f NAME` (currently a stub
printing `declare -f NAME`) to print the real function body. Foundation for the
deferred `export -f` (M-121).
**Branch (impl):** `v146-generate-source`.

## Background

huck stores functions as parsed AST (`shell.functions: Rc<HashMap<String,
Box<Command>>>`, the value being the function BODY `Command`), NOT source text.
`declare -f NAME` (src/builtins.rs ~908) only emits the literal line
`declare -f NAME` — it cannot show the body. bash shows the full body. This module
generates source from the AST.

**Naming:** the module is `generate` (NOT "deparse").

## Decision: NORMALIZED, re-parseable output (not byte-identical to bash)

bash's `declare -f` pretty-printer is intricate and internally inconsistent (e.g.
`for …;` puts `do` on a new line but `while …; do` keeps it inline; trailing-space
header `f () \n{ \n`; quirky `case` layout). Matching it byte-for-byte across the
full AST is a quirk-chase with no end. Instead `generate` emits ONE consistent,
valid, re-parseable bash style. Correctness is verified by **round-trip stability**,
not bash-diff (see Testing). This is exactly what `export -f` needs later (a child
shell re-parses the text).

## API (`src/generate.rs`)

```rust
/// Render a function definition as `NAME ()\n<body>` (for `declare -f`).
pub fn function_to_source(name: &str, body: &Command) -> String;

/// Render any command to normalized source. `indent` is the current nesting
/// depth (in 4-space units); the FIRST line is emitted at `indent`, and the
/// returned String has a trailing newline iff it spans multiple lines.
pub fn command_to_source(cmd: &Command, indent: usize) -> String;
```
Plus private helpers, each `EXHAUSTIVELY` matching its enum (a drift-guard: a new
AST variant forces a compile error here): `sequence_to_source`, `pipeline_to_source`,
`simple_to_source`, `exec_to_source`, `word_to_source`, `redirect_to_source`,
`assignment_to_source`, `testexpr_to_source`, and one per compound clause
(`if`/`while`/`for`/`case`/`select`/`arith_for`/`subshell`/`brace_group`).

`declare -f NAME` (src/builtins.rs) looks up `shell.functions.get(name)` and prints
`function_to_source(name, body)`; missing function → silent rc 1 (existing behavior).

## Normalized format rules

- **Indent:** 4 spaces per nesting level.
- **Function header:** `NAME ()` then newline, then the body command at indent 0.
  A `BraceGroup` body renders `{` / indented contents / `}`.
- **Sequence** (`first` + `rest: Vec<(Connector, Command)>` + `background`):
  - `Semi` → terminate the line, next command on a new line at the same indent.
  - `And` → ` && `, `Or` → ` || ` — inline on the same logical line.
  - `Amp` → ` &`, then a new line for what follows.
  - `background` (whole-sequence `&`) → trailing ` &`.
- **Pipeline:** stages joined by ` | `; a `negate` pipeline is prefixed `! `.
- **SimpleCommand::Exec:** `inline_assignments` (space-joined) + `program` + `args`
  (space-joined `word_to_source`) + redirects (space-joined, appended). 
  **SimpleCommand::Assign:** the assignments, space-joined.
- **Compounds** (each on its own lines, uniform):
  - `if`: `if <cond>; then` / body / (`elif <c>; then` / body)* / (`else` / body)? / `fi`.
  - `while`/`until`: `while <cond>; do` (or `until`) / body / `done`.
  - `for`: `for VAR in W1 W2 …; do` / body / `done` (no-`in` form omits ` in …`).
  - `arith_for`: `for ((init; cond; step)); do` / body / `done`.
  - `select`: `select VAR [in …]; do` / body / `done`.
  - `case`: `case SUBJECT in` / (`PAT1 | PAT2)` / body / `;;`|`;&`|`;;&`)* / `esac`.
  - `subshell`: `(` / body / `)`. `brace_group`: `{` / body / `}`.
  - `Arith`: `(( <body> ))`. `DoubleBracket`: `inline_assigns [[ <testexpr> ]]`.
  - `Redirected`: inner command + the trailing redirects.
- **Quoting:** `word_to_source` re-quotes only where needed (see below). The exact
  inter-token spacing is NOT specified to the byte — round-trip stability is the
  contract, so it need only be SELF-consistent and re-parseable.

## Word serialization (`word_to_source`)

A `Word` is `Vec<WordPart>`; render each part and concatenate:
- `Literal { text, quoted }` — if `quoted`, wrap so the text is preserved literally
  (double-quote + escape `"`,`\`,`$`,`` ` `` via the existing
  `escape_double_quote_value`, or single-quote when simpler); if `!quoted`,
  backslash-escape shell metacharacters that would otherwise be special (reuse the
  existing `escape_filename`/`xtrace_quote`-style helper). Empty quoted literal → `''`.
- `Var { name, .. }` → `$name`. `LastStatus` → `$?`. `AllArgs { joined }` → `$*`/`$@`.
- `ParamExpansion { name, modifier, subscript, indirect }` → `${ [!] name [subscript]
  [modifier] }` — reconstruct from `ParamModifier` + `SubscriptKind` (exhaustive match).
- `CommandSub { sequence }` → `$(<sequence_to_source>)`. `Arith { body }` → `$((<body>))`.
- `Tilde(TildeSpec)` → `~` / `~user`. `AssignPrefix { target, append }` → `name[sub]=`
  / `name+=` (the assignment LHS prefix). `ArrayLiteral(elems)` → `(elem …)`.
- A double-quoted RUN of parts (adjacent `quoted` parts incl. `$var`/`$(…)` inside
  `"…"`) must re-emit inside ONE pair of quotes so the quoting round-trips.

Word serialization is the trickiest part; any `ParamExpansion`/quoting form that
cannot round-trip exactly is documented as a known gap (and exercised by the corpus
so we KNOW which forms are affected).

## Testing — round-trip, not bash-diff

The correctness invariants for a source string `src`:
```
let a = parse(tokenize(src));            // Sequence
let s1 = sequence_to_source(&a, 0);
let b = parse(tokenize(&s1));
let s2 = sequence_to_source(&b, 0);
assert_eq!(s1, s2);                      // HARD: IDEMPOTENT — serialize is a stable fixpoint
```
- **`s1 == s2` is the HARD invariant** (idempotence): re-parsing the generated source
  and re-serializing yields the identical string. This guarantees the output is valid
  and self-consistent.
- **`a == b` (AST equality) is a STRONG check applied where serialization is
  representation-preserving.** It legitimately FAILS for quoting normalization (e.g.
  `'x'` → `x` → a different `quoted` flag, same meaning) — those cases are covered by
  the execution-equivalence test instead, NOT forced to AST-equal. The corpus marks
  which fragments assert `a == b` vs only `s1 == s2` + execution-equivalence.

1. **Round-trip corpus** (`src/generate.rs` mod tests): a table of source fragments
   covering EVERY construct — simple cmd, args+quoting (`"a  b"`, `'x'`, `a\ b`),
   pipelines, `&&`/`||`/`;`/`&`, `!`-negation, `if`/`elif`/`else`, `while`/`until`,
   `for`(+no-in), `for ((;;))`, `select`, `case` (all 3 terminators, `|` patterns),
   subshell, brace group, `[[ … ]]` (unary/binary/regex/!/&&/||), `((expr))`,
   redirects (`<`,`>`,`>>`,`>|`,`>&N`,heredoc,herestring), assignments (bare/`+=`/
   indexed/array-literal), nested compounds. Each asserts `s1 == s2` AND `a == b`.
2. **Execution-equivalence** spot tests: define a function, `declare -f` it, re-`eval`
   the printed text into a fresh shell, run it, compare output to the original.
3. **`declare -f` integration test**: `f(){ echo hi; }; declare -f f` prints a body
   containing `echo hi` (not the old `declare -f f` stub line) and is re-parseable.
4. **Full regression:** entire suite + all harnesses green; clippy clean. (No new
   bash-diff harness for the format — normalized ≠ bash; round-trip is the gate.)

## Scope & coverage

- **Full Command AST + Word**, exhaustive per-enum matches (drift-guard). Any function
  serializes. No "unsupported" fallback in scope.
- **Heredoc round-trip caveat:** a heredoc redirect's body must re-emit as a heredoc
  (`<<DELIM\n…\nDELIM`) which spans lines; if exact heredoc round-trip proves
  expensive, a here-string-style fallback that preserves semantics is acceptable and
  documented. (Flagged so the implementer scopes it deliberately.)
- **Out of scope:** `export -f` itself (the env encoding + child import) — a FOLLOW-ON
  (M-121) that consumes this module. byte-identical-to-bash `declare -f` output.

## Documented divergences
- `declare -f` output is NORMALIZED, not byte-identical to bash's pretty-printer
  (different whitespace/layout; semantically equivalent + re-parseable). Add/refresh
  a low-impact `[intentional]` note (the existing M-121/`declare -f`-stub mention is
  resolved for the body-printing part; `export -f` stays deferred under M-121).

## Files & responsibilities

| File | Change |
|------|--------|
| `src/generate.rs` (NEW) | The serializer: `function_to_source` + `command_to_source` + per-type helpers + the round-trip corpus tests. |
| `src/main.rs` (or lib root) | `mod generate;` registration. |
| `src/builtins.rs` | `declare -f NAME` (~908) calls `generate::function_to_source` with the looked-up function body; remove the stub line. |
| `docs/bash-divergences.md` | Note `declare -f` now prints a normalized (non-byte-identical) body; keep M-121 (`export -f`) deferred, narrowed to the env-encoding/propagation work. |

## Notes
- Reuse existing quoting helpers (`escape_double_quote_value`, the `@Q`/ansi-c and
  filename-escape routines) rather than writing new ones.
- The recursive walker threads an `indent: usize`; helpers return `String` (simple)
  or push into a `&mut String` line buffer (implementer's choice — keep it readable).
- **Git safety:** implementer subagents must NOT `git checkout <sha>`; the controller
  verifies the branch tip before merging. Commit trailer:
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
