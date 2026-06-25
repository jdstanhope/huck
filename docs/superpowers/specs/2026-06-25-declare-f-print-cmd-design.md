# v218 — bash-faithful function reconstruction (`declare -f` / `type`)

## Status

Design approved 2026-06-25 (scope confirmed with the user: faithful
`print_cmd.c` port for divergence classes A–D + the `declare -F` fix F + the
required `type`-prints-body companion fix G; herestr quote-provenance E and
`time`-pipeline support deferred).

## Background

The bash test-suite runner (`tests/bash-test-suite/runner.sh`, v214/v217)
runs upstream bash 5.2.21's own `.tests` through huck and diffs against the
committed `.right` files. Several still-FAIL categories diverge because
huck's function/command **reconstructor** (`crates/huck-syntax/src/generate.rs`)
emits a deliberately *normalized* source form (documented as M-121), not the
byte-exact format bash's `print_cmd.c` produces for `declare -f` and `type`.

The three reachable target categories — **cprint**, **func**, **arith-for** —
exercise the reconstructor through `type NAME` and `declare -f NAME`. (A
fourth, **herestr**, is dominated by here-string quote provenance, which is
AST-limited; see Out of Scope.)

### What `generate.rs` is and who calls it

`generate.rs` renders a parsed `Command` AST back to re-parseable shell
source. Its current contract is *round-trip idempotence* (render→parse→render
is stable, and the AST is preserved), NOT byte-identity with bash. Callers
(full blast radius):

- `builtins.rs:1065` `emit_function` — `declare -f NAME` body.
- `builtins.rs:955` — exported-function listing (`declare -fx`).
- `shell_state.rs:2180` — `exported_function_value` (the `BASH_FUNC_xxx%%`
  env-export VALUE form a child shell re-parses).
- `lib.rs:79` re-exports `command_to_source` / `function_to_source`.

The format change is confined to these paths. The env-export and round-trip
re-import paths keep working because the new format still re-parses to the
same AST (idempotence is preserved — see Testing).

### The three reachable categories invoke the printer like this

- **cprint.tests** — defines functions, then `type fu\%nc`, `type tf`,
  `type tf2`, and `declare -f f1`. Bodies contain: simple commands with
  redirects, pipelines, `&&`/`||` lists, `;`-lists, background (`&`),
  subshell `( exit 1 )`, brace group `{ echo a; }`, `while (( i < 3 ))`,
  arithmetic `i=$(( i + 1 ))`, `case`, `[[ ]]`, `if … elif … fi`, `until`,
  and a `time` pipeline.
- **func.tests** — `type`, `declare -f f1`, and a child-shell `type zf`.
- **arith-for.tests** — `type fx` over an arithmetic-`for` function.

All three need (a) faithful `generate.rs` AND (b) `type` to print the body
(huck currently prints only `NAME is a function`). Neither alone flips a
category.

## Goals

1. `generate.rs` reproduces bash 5.2.21 `print_cmd.c` output **byte-for-byte**
   for the constructs huck's AST supports, in the `declare -f` / `type`
   (`inside_function_def`) context.
2. `type NAME` (and `command -V NAME`) print the reconstructed function body
   after `NAME is a function`, matching bash.
3. `declare -F NAME` (explicit name) prints the bare name; `declare -F` (no
   args) keeps the `declare -f[x] NAME` listing.
4. A gold-standard `tests/scripts/declare_f_diff_check.sh` harness asserts
   byte-identical output between system bash and huck across every construct.
5. `generate.rs` round-trip idempotence is preserved; `cargo test --workspace`
   stays green.

## Non-goals / Out of scope (deferred, documented)

- **herestr quote provenance (E).** huck's `Word` AST normalizes quoting, so a
  here-string body `<<< "$a"" ""$b"` re-renders as `"$a $b"`, and a
  double-quoted literal can re-render single-quoted. Byte-matching requires
  tracking original quote spans through the lexer/`Word` — a larger AST change,
  the same limitation as the known xtrace quote-provenance residual. herestr
  stays FAIL; record as a `[deferred]` divergence.
- **`time` pipelines.** huck has no `time`/`CMD_TIME_PIPELINE` AST node at all,
  so a `time` pipeline inside a function cannot round-trip. cprint contains
  one. Out of scope; record as a `[deferred]` gap. (cprint may therefore not
  reach full PASS this iteration even after A–G; arith-for and func are the
  high-confidence flips.)
- **`declare -F` attribute nuances** beyond current behavior in the no-arg
  listing (e.g. `declare -fx` vs `declare -f`): unchanged. F is only the
  explicit-name bare-name fix.
- Any non-`inside_function_def` printing mode (`print_comsub`, xtrace
  `reconstruct_word_source` in expand.rs) — separate code path, untouched.

## Design

### Authoritative reference

bash 5.2.21 `print_cmd.c` (read from the operator-supplied
`$BASH_SOURCE_DIR`, GPL posture: understood and re-implemented in Rust, never
vendored or copied verbatim) and the byte output of system `bash 5.2.21`
captured live by the diff harness. The relevant algorithm, distilled:

#### Function definition (`declare -f`, `type`) — `print_function_def`

```
NAME () ␣\n          (note the trailing space after `)`, then newline)
{ ␣\n                (brace, trailing space, newline)
<body at indent 4>\n
}
```

The body is the function's group-command *inner* command (bash unwraps the
outer `{ }` and supplies its own). Printing runs with `inside_function_def`
set and `indentation` starting at 4.

#### Connection (`;` / newline connector) under `inside_function_def`

`;` is a **separator emitted after the left operand**, followed by a newline:

```
a;
b;
c              (final command: no trailing `;`)
```

This replaces huck's current `sequence_to_source`, which terminates *every*
element. (A `&` connector emits ` &\n`; `&&`/`||` stay inline as ` && ` /
` || `.)

#### `semicolon()` — the trailing-`;` quirk

bash's `semicolon()` appends `;` **unless the last emitted char is already `&`
or `\n`**. It is called by the for/while/until/if/select printers after their
body (and after the if-test). Group, function, subshell, and case bodies do
**not** call it. Consequence:

- `if`/`while`/`until`/`for`/`select` body's last command gets a trailing `;`
  (e.g. `b;` then `done`), UNLESS that command ended in `&` (background) or a
  newline (a nested compound that ends with `done`/`fi`/`esac`/`}`).
- group/function/subshell/case body's last command gets **no** trailing `;`.

#### Compound layouts (`inside_function_def`)

| Construct | Format |
|---|---|
| `if` | `if <test>; then\n` … body … `;\n` `fi`. The test's `;` and the body's trailing `;` both come from `semicolon()`. |
| `elif` | **No elif node in bash** — render as nested `else\n` + indent+4 + `if … fi`, recursively, deepening per branch. The inner `fi` gets a `;` (outer `semicolon()`); the outermost `fi` does not. |
| `else` | `semicolon()` + `else\n` + body. |
| `while`/`until` | `<kw> <test>; do\n` (test + `do` on the **same** line via `; do`) … body … `;\n` `done`. |
| `for … in` | `for NAME in W…;\n` then `do\n` (**`do` on its own line**) … body … `;\n` `done`. |
| arith-`for` | `for ((INIT; TEST; STEP))\n` (no `;` before `do`) then `do\n` (**own line**) … body … `;\n` `done`. |
| `select` | like `for … in` but `select NAME in W…;\n` `do\n`. |
| `case` | `case W in ␣\n` (trailing space after `in`), each clause: `<pat0> \| <pat1>)\n` body at +8, then `;;` / `;&` / `;;&` on its **own line** at +4; final `esac`. Body has no trailing `;`. |
| subshell | inline `( ␣<body>␣ )` (single space inside parens, no newlines). |
| group `{ }` (inside fn) | multiline `{ ␣\n<body at +4>\n}`. |
| `(( … ))` arith command | `((` + space-joined word body + `))`, **no surrounding quotes** (fixes the `((" i < 3 "))` bug). |
| `[[ … ]]` | unchanged (already `[[ <expr> ]]`). |

Indentation is 4 spaces per level (`indentation_amount`), via a `newline()`
that emits `\n` + current indent.

### Component 1 — rewrite `generate.rs` compound/function/connection printers

Touch only the compound/function/connection/`Arith` rendering. Leaf rendering
(`word_to_source`, `part_to_source`, redirections, `[[ ]]`, param expansions)
is unchanged. Concretely:

- **New `inside_function_def` mode.** The reconstructor must thread a flag (or
  a small `Printer` struct holding `indent` + `inside_function_def`) so the
  connection printer knows to emit the bash separator form and the compound
  printers know to call the `semicolon()` rule. `function_to_source` enters
  this mode; `command_to_source`/`exported_function_value` enter it for the
  function body (the env-export VALUE form `() {\n … \n}` must also adopt the
  faithful body so a child shell re-parses identically — verified by the
  existing `exported_function_value_form` test plus a round-trip).
- Implement the `semicolon()` last-char rule (`& ` / `\n` suppression) as a
  helper that inspects the accumulated string tail.
- Implement `if_to_source` elif→nested-`else { if }` conversion.
- `Command::Arith` → `(({}))` with the word rendered **unquoted** (the `Arith`
  word's parts must render without the wrapping double-quotes huck currently
  adds; verify the AST stores the body such that this is a pure rendering
  change, not a parse change).
- Subshell → inline `( … )`; group inside fn → multiline.

### Component 2 — `type` / `command -V` print the body (G)

- `emit_type_entry` (`builtins.rs:6594`, the `type` helper) `CommandResolution::Function`
  branch: after `NAME is a function`, when NOT concise (`type -t`), emit
  `function_to_source(name, body)` (look the body up in `shell.functions`).
- `builtin_command` `command -V` path (`builtins.rs:6940`): same addition, for
  parity with bash (`command -V` also prints the body). `command -v` (concise)
  stays `NAME`.
- `type -t` stays `function`. Verify the existing
  `myfn is a function` unit test (builtins.rs:11089) is updated to the new
  body-printing behavior.

### Component 3 — `declare -F NAME` bare name (F)

`emit_function`/`declare_list_functions` (`builtins.rs:1028–1067`): when
`names_only` AND the name was **explicitly requested** (the `for name in names`
branch of `declare_list_functions`), print the bare `NAME` (bash `name_cell`).
The no-args branch is **unchanged** — it keeps emitting `declare -f NAME` for
every function as today (the existing behavior; bash's `-fx`-for-exported
nuance in that path is a separate, pre-existing divergence and stays out of
scope). Thread an `explicit: bool` (or split the two emit calls) so the two
cases differ.

## Testing / Verification

- **Gold-standard harness** `tests/scripts/declare_f_diff_check.sh` (mirrors
  `arith_error_diff_check.sh`): for each fragment, define a function, run
  `declare -f` / `type` / `declare -F` through both system `bash` and the huck
  binary, assert byte-identical stdout. Fragments cover every row of the
  layout table: simple `;`-list (last-`;` suppression), `&&`/`||`, pipeline,
  background `&`, redirects, subshell, group, `if`, `if/elif/else`, `while`,
  `until`, `for … in`, arith-`for`, `select`, `case` (all three terminators),
  `[[ ]]`, `(( ))`, and nesting. Skip gracefully if `bash` is absent (like the
  helper-provisioning preflight).
- **Round-trip preservation.** `generate.rs`'s `assert_rt` / `assert_rt_ast_eq`
  tests must still pass — the new format re-parses to the same AST. Update any
  test that asserts a *literal* normalized string to the new bash-faithful
  string; idempotence/AST-equality assertions need no change in expectation,
  only re-verification.
- **Unit tests** for Components 2 and 3 in `builtins.rs` (type-prints-body;
  `declare -F NAME` bare vs `declare -F` listing).
- **Full suite:** `cargo test --workspace` (~3648 tests; a plain `cargo test`
  silently skips the engine/syntax/cli crates).
- **Baseline re-triage:** re-run cprint/func/arith-for/herestr through the
  runner and update `docs/bash-test-suite-baseline.md` with measured status
  (expect arith-for and func to flip or collapse to small residuals; cprint
  gated by the deferred `time` pipeline; herestr gated by E).

## Risks

- **Round-trip regressions from the format change.** Mitigated by the existing
  `assert_rt*` corpus plus new harness; any AST-changing render is a bug caught
  by `assert_rt_ast_eq`.
- **`semicolon()` last-char rule subtlety** (`&`/`\n` suppression) — covered by
  background-`&` and nested-compound harness fragments.
- **elif nesting indent drift** — covered by an `if/elif/elif/else` fragment.
- **Arith word still wrapping quotes** — if the `Arith` AST stores the body in
  a way that forces quotes, this becomes a small AST/parse touch; flagged for
  the implementer to confirm it's render-only.
- **`exported_function_value` child re-parse** — the env-export VALUE form must
  stay re-parseable after the format change; covered by the existing
  `exported_function_value_form` test + a new round-trip fragment.
- **cprint may not fully PASS** (deferred `time` pipeline). This is expected and
  recorded; func + arith-for are the committed flips.

## Divergence-doc bookkeeping

- DELETE the resolved `declare -f` trailing-space entry from
  `docs/bash-divergences.md` (current-divergences-only doc).
- ADD `[deferred]` entries for: (E) here-string / word quote provenance in
  reconstruction (AST-limited), and the `time`-pipeline reconstruction gap (no
  `time` AST node). Cross-reference the xtrace quote-provenance residual.
- Update `docs/bash-test-suite-baseline.md` and the iteration memory
  (`project_huck_iterations.md` + `MEMORY.md`) on merge.
