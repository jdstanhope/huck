# v227 — getopts category flip (error-prologue, targeted) — Design

**Status:** approved (brainstorm 2026-06-26)
**Iteration goal:** flip bash's `getopts` test-suite category FAIL→PASS
(PASS 9→10) by landing the error-message prologue *plus* the getopts-specific
fixes that are its co-blockers. This is the measure-first re-scope of
"error-message prologue shell-wide": measurement showed the pure prologue
flips zero categories, so v227 targets the smallest-residual category whose
last blockers include the prologue.

## Background & measurement

huck historically prefixes all error messages with `huck:`; bash uses a
`<name>: [line N: ][cmd: ]` prologue in script mode (name = `BASH_SOURCE[0]`
/ `$0`). v216 added `Shell::error_prefix(cmd: Option<&str>) -> String`
(`shell_state.rs`) as the foundation and converted only the arith slice.
Shell-wide conversion is a multi-iteration program; the standing rule
(`bash-test-suite-value-map`) is "do the prologue where it's a category's
LAST blocker."

Before designing, all 11 prologue-touched categories were measured by
normalizing the prologue away on both sides and inspecting the residual.
**No category is prologue-only** — every one retains non-prologue blockers,
confirming the prologue flips nothing alone. The two smallest residuals were
`getopts` (4/5 residual lines) and `parser` (6/7). `getopts` was chosen: it
is the most self-contained, and its blocker set exercises four distinct
prologue forms, making it a representative slice of the prologue program
while still delivering a category flip.

Measurement command (operator tree, nothing vendored):
`THIS_SH=<release huck> sh ./run-getopts` from a copy of
`$BASH_SOURCE_DIR/tests`, diffing against bash's `getopts.right`.

## The five blockers

Running `getopts.tests` (bash file-mode) surfaces exactly five divergences.
Bash statuses/messages below are verified against bash 5.2.21 and its
`builtins/getopts.def`.

| # | Trigger (test site) | huck now | bash 5.2.21 | Kind |
|---|---|---|---|---|
| B1 | `getopts` / `getopts opts` — too few operands | `huck: getopts: usage: … name [arg]` | `getopts: usage: getopts optstring name [arg ...]` | message body + drop `huck:` prefix |
| B2 | getopts-internal option errors (OPTERR≠0): `illegal option -- c`, `option requires an argument -- b` (getopts1/5.sub) | `huck: illegal option -- c` | `./getopts5.sub: illegal option -- c` (prefix = `$0`, no line, no `getopts:`) | prefix = `$0` |
| B3 | `getopts -a opts name` — invalid option *to getopts itself* (getopts.tests line 23) | *(silently accepted; no output)* | `./getopts.tests: line 23: getopts: -a: invalid option` then `getopts: usage: …` | missing behavior |
| B4 | `getopts :ab: opt-var "$@"` — invalid name var (getopts7.sub) | `huck: getopts: \`opt-var': not a valid identifier`, **OPTIND left unset** → `[: integer expression expected`, `remaining args: -a` | `./getopts7.sub: line 17: getopts: \`opt-var': not a valid identifier`, OPTIND bound (→2) → `remaining args:` empty | prologue + OPTIND-ordering bug |
| B5 | `readonly OPTARG; getopts :x x` (getopts10.sub) | `huck: OPTARG: readonly variable` | `./getopts10.sub: line 16: OPTARG: readonly variable` | prologue (generic `assign()` site) |

### Why these specific bash formats (from `getopts.def`)

- **Usage** (`builtin_usage()`): prints `%s: usage: …` with
  `this_command_name` = `getopts` only — no shell prologue, no line number,
  in BOTH interactive and script mode. Hence B1's `getopts: usage: …`.
- **Invalid option to the builtin** (`internal_getopt(list, "")`): getopts
  declares no options, so any leading `-X` is invalid and reported via the
  builtin-error path (full prologue + `getopts:`), followed by
  `builtin_usage()`. Hence B3's two lines.
- **getopts-internal option diagnostics** (`sh_getopt`): `dogetopts` sets
  `argv[0] = dollar_vars[0]` (= `$0`) before calling `sh_getopt`, so these
  use `$0` as the prefix — no line, no `getopts:`. Gated by `OPTERR`. Hence
  B2.
- **OPTIND binding order**: `dogetopts` binds `OPTIND` from the post-parse
  value **unconditionally**, *before* `getopts_bind_variable(name, …)` runs
  the identifier/readonly checks. So an invalid name (or readonly OPTARG)
  still advances OPTIND. huck currently checks the name first and returns
  before binding OPTIND, leaving it unset. Hence B4's OPTIND bug.
- **Invalid identifier** (`getopts_bind_variable` → `sh_invalidid`): full
  prologue + `getopts:` + `` `name': not a valid identifier ``; returns
  `EXECUTION_FAILURE` (1). Hence B4's message.
- **Readonly assignment** (`bind_variable` of OPTARG): the generic
  readonly-variable assignment error, full prologue + `name: readonly
  variable`, no `getopts:`. Hence B5.

The four prologue forms exercised: **none** (`getopts: usage:`),
**`$0`-only** (`$0:`), **full builtin** (`<src>: line N: getopts:`), and
**full bare** (`<src>: line N:`).

## Fix design

All changes are in `crates/huck-engine/src/builtins.rs::builtin_getopts`
except B5. `builtin_getopts` is restructured so OPTIND is bound before the
name/OPTARG checks (B4), and the error prefixes are routed through
`shell.error_prefix(...)` / `shell.shell_argv0`.

### B1 — usage message

Replace the literal at `builtins.rs:4898`:
```
"huck: getopts: usage: getopts optstring name [arg]"
```
with
```
"getopts: usage: getopts optstring name [arg ...]"
```
(no `huck:` prefix — bash's `builtin_usage()` emits only the builtin name;
`[arg]` → `[arg ...]`). Status stays `Continue(2)` (EX_USAGE).

### B2 — getopts-internal option errors use `$0`

Change the verbose-error emission (currently `e!(err, "huck: {body}")`,
`builtins.rs:4939`) to:
```
e!(err, "{}: {body}", shell.shell_argv0);
```
Still gated on `OPTERR != "0"`. In stdin/interactive mode `shell_argv0` is
`huck`, so the existing stdin harness and integration tests are unaffected;
in file mode it is the script path (`./getopts5.sub`), matching bash's
`dollar_vars[0]`. `shell_argv0` is preserved inside functions (bash keeps
`$0`), so the getopts5.sub function-call cases are covered.

### B3 — reject invalid option to getopts itself

Before treating `args[0]` as the optstring (and after the no-operand
usage case), scan the leading operand: if `args[0]` starts with `-` and is
neither `-` nor `--`, it is an invalid option to getopts. Emit, using the
first character `c` after the dash:
```
{error_prefix(Some("getopts"))}-{c}: invalid option
getopts: usage: getopts optstring name [arg ...]
```
and return `Continue(2)`. (`error_prefix(Some("getopts"))` yields
`<BASH_SOURCE>: line N: getopts: `.) A bare `--` leading operand is
consumed (option terminator) and the following operands become
optstring/name; a bare `-` is treated as a normal (non-dash) operand. Only
the single-invalid-`-X` case is exercised by the suite; the `--`/`-`
handling mirrors `internal_getopt` for correctness and is documented.

Ordering of the early checks in `builtin_getopts`:
1. `args.is_empty()` → usage (B1), return 2.
2. `args[0]` starts with `-` and ∉ {`-`,`--`} → invalid option (B3), return 2.
3. (after consuming a leading `--`) `args.len() < 2` → usage (B1), return 2.

### B4 — bind OPTIND before name validation; prologue on invalid name

Restructure the body so that after `getopts_step`:
1. Write back OPTIND + cursor cache **unconditionally**
   (`shell.set("OPTIND", step.optind…)`, `getopts_optind_cache`,
   `getopts_sp`).
2. **Then** validate `name`: if `!is_valid_name(name)`, emit
   `{error_prefix(Some("getopts"))}` + `` `{name}': not a valid identifier ``
   and return `Continue(1)` (bash `EXECUTION_FAILURE`).
3. Assign the matched letter to `name`, set/unset OPTARG (B5 site).
4. Emit the getopts-internal option error (B2) if any.

This both adds the prologue and fixes the OPTIND-left-unset bug: in
getopts7.sub, parsing `-a` advances OPTIND to 2, so `[ "$OPTIND" -gt 1 ]`
is true, `shift 1` runs, and `remaining args:` is empty — matching bash and
removing huck's spurious `[: integer expression expected`.

The current early name-validity check (`builtins.rs:4903-4906`, before
OPTIND is read) is removed; the check moves to step 2 above.

### B5 — convert the generic readonly assignment error

In `crates/huck-engine/src/shell_state.rs::assign` (line 1648), change:
```rust
with_err(|err| e!(err, "huck: {name}: readonly variable"));
```
to compute the prologue first (so the `&self` borrow ends before
`with_err`), then:
```rust
let prefix = self.error_prefix(None);
with_err(|err| e!(err, "{prefix}{name}: readonly variable"));
```
`error_prefix(None)` yields `<BASH_SOURCE>: line N: ` non-interactively and
`huck: ` interactively, so getopts10.sub gets
`./getopts10.sub: line 16: OPTARG: readonly variable` and interactive
assignments are unchanged. This is a shell-wide micro-conversion (every
assignment readonly error), bash-correct, and advances the prologue program
(the `errors` category also has bare `readonly variable` lines).

This converts ONLY the generic `assign()` site. The builtin-specific
readonly errors (`unset:`/`export:`/`local:`/`readonly:` at
`builtins.rs:702,773,1245,1287,1386,1490,1659`) keep their `huck:` prefix —
they are separate prologue work for the `errors`/`builtins` categories in a
later iteration (out of scope for v227, YAGNI).

## Testing & verification

**Primary deliverable:** the bash-test-suite `getopts` category flips
FAIL→PASS via `tests/bash-test-suite/runner.sh`
(`HUCK_BASH_TEST_CATEGORY=getopts`), raising the PASS count 9→10. This is
the gold-standard check.

**Harness** (`tests/scripts/getopts_diff_check.sh`): add a **file-mode**
section — write each fragment to a temp script, run through `bash` and
`huck`, assert byte-identical stdout+stderr+rc — covering all five
blockers:
- B1: `getopts` with too few operands (`getopts: usage: … [arg ...]`, rc 2).
- B2: a non-silent optstring hitting `illegal option`/`option requires an
  argument`, asserting the `$0`/script-path prefix.
- B3: `getopts -a opts name` → invalid-option line + usage, rc 2.
- B4: invalid name var → prologue message + OPTIND bound (observe via a
  following `[ "$OPTIND" -gt 1 ]` / `shift` so `remaining args:` matches).
- B5: `readonly OPTARG; getopts :x x` → `<src>: line N: OPTARG: readonly
  variable`.

The existing stdin-mode checks stay and remain green (B2/B5 keep `huck:`
when `$0`=`huck` interactively; B1's `getopts: usage:` carries no shell
prefix in either mode).

**Integration tests** (`tests/getopts_integration.rs`): add cases for B3
(leading invalid option rejected, rc 2, no parse side effects) and B4
(OPTIND bound even when the name is invalid).

**Regression guard:** B5 touches the shared `assign()` path, so run **all
142 `tests/scripts/*_diff_check.sh` harnesses** and **`cargo test
--workspace` (3733 tests)**; re-measure the `errors` category (must not
regress — should shrink). `funcnest_diff_check.sh` is release-only (v224
debug-stack artifact, not a regression).

**Risks (assessed low):**
- B5 generic conversion: the only tests pinning the generic readonly
  message (`arrays_integration.rs:122`,
  `associative_arrays_integration.rs:121`) use `err.contains("readonly
  variable")` (substring), which survives the prefix change; the full sweep
  is the backstop.
- B3 leading-`-` detection cannot misfire on a valid optstring (POSIX
  getopts optstrings never start with `-`; a leading `:` is the silent flag,
  not a dash).
- B4 invalid-name return code aligned to bash's `1`; not directly
  constrained by the suite, but the byte-identical harness confirms it.
- Interactive readonly output unchanged (`error_prefix` returns `huck:`
  when interactive).

## Non-goals

- The other prologue-touched categories (alias, type, dirstack, trap,
  printf, shopt, execscript, errors, builtins, parser) — each has its own
  residual blockers (command-not-found word order, Rust `io::Error` text
  leakage, unimplemented builtins, `set -o` gaps, etc.). They are future
  iterations; v227 only converts the generic `assign()` readonly site beyond
  getopts.
- Converting the builtin-specific readonly errors or any other `huck:` site
  not required by the `getopts` category.
- The command-not-found word order and Rust-io-text cleanup (the two broad
  shared blockers measurement surfaced) — separate candidate iterations.
