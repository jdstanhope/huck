# v348 — flip the `exportfunc` bash-suite category

**Issue:** [#339 — exportfunc: hyphen-function import, HEREDOC_MAX (CVE-2014-7186), eval line-number, export-name validation](https://github.com/jdstanhope/huck/issues/339).

**Goal:** flip `exportfunc` to PASS (byte-identical) by fixing four real
behavioral roots. Target: full runner PASS 35 → 36.

## Background & feasibility spike

`exportfunc` tests exporting/importing shell functions via the
`BASH_FUNC_<name>%%` environment encoding, plus two shellshock-era CVE
regression checks. Local bash 5.2.21 matches `exportfunc.right` exactly, so all
roots are reproducible. The clean full-category diff is **11 lines** → exactly
these four roots (all real behavior; no `$0`/prog-name artifacts):

## Root 1 — import a hyphen-named exported function

`foo-a() {...}; export -f foo-a; ${THIS_SH} -c 'foo-a'` → bash runs it
(`exportfunc ok 2`); huck `foo-a: command not found`. bash allows `-` (and other
non-identifier chars) in a function name and imports the `BASH_FUNC_foo-a%%`
env var; huck's import is too strict.

**Location:** `crates/huck-engine/src/shell_state.rs` — the BASH_FUNC import loop
(~1177) that strips `BASH_FUNC_` prefix / `%%` suffix (~1071) to get `<name>`
and calls `parse_imported_function(name, value)` (~1004). The over-strict check
rejects `foo-a`. **Fix:** accept a `<name>` that is a valid *function* name
(bash: any nonempty string with no `=` / `/` / NUL and not otherwise reserved —
`-` is fine), not only a strict `[A-Za-z_][A-Za-z0-9_]*` identifier. Keep the
shellshock protection (`parse_imported_function` already rejects a trailing
command / non-lone-FunctionDef body — do not relax that).

## Root 2 — `eval` syntax-error line-number off-by-one

`eval 'X() { (a)>\'` (malformed body) → bash `eval: line 44: syntax error:
unexpected end of file`, huck `line 43`. The line number reported for a syntax
error inside `eval` is one low.

**Location:** the `eval` builtin's parse-error line reporting / `$LINENO` base
for the eval'd string. **Fix:** align the eval error line base with bash (the
error line is relative to the eval string; verify the exact +1 and whether it is
eval-specific or a shared `-c`/string-parse base — fix at the eval site to avoid
disturbing script line-numbering).

## Root 3 — enforce HEREDOC_MAX (CVE-2014-7186)

`cat <<EOF <<EOF ... <<EOF` (18 heredocs on one command, exportfunc1.sub:14) →
bash errors `maximum here-document count exceeded`; huck processes all of them.
bash caps pending heredocs per command at `HEREDOC_MAX` (10).

**Location:** `crates/huck-syntax/src/lexer.rs` — where a heredoc opener enqueues
into `pending_heredocs` (~1295) / `atom_pending_heredocs`. **Fix:** when the
count of pending heredocs for the current command would exceed the bash limit
(10), emit the error `maximum here-document count exceeded` (matching bash's
message and error line: `./exportfunc1.sub: line 14: ...`) and fail the parse.
Confirm bash's exact threshold empirically (`HEREDOC_MAX` = 10 in bash source —
the 11th opener triggers it) and match the message + line + resulting output.

## Root 4 — export-name validation for functions

`export -f foo=bar` and `export -f /bin/echo` → bash rejects (`export:
foo=bar: cannot export`, exit 1) because the name can't be encoded as a
`BASH_FUNC_<name>%%` env var; huck accepts (exit 0, emits the env pair).

**Location:** the `export -f` builtin (function-export path). **Fix:** reject
`export -f <name>` when `<name>` cannot be env-encoded (contains `=`; bash's
message is `export: <name>: cannot export`, exit status 1). Match bash's exact
message + status. (Note the asymmetry with Root 1: bash imports `foo-a` because
`BASH_FUNC_foo-a%%` IS a valid env var name, but rejects exporting `foo=bar`
because `BASH_FUNC_foo=bar%%` is NOT — the `=` breaks the encoding.)

## Verification

- **Official `exportfunc` runner** produces zero diff (the flip signal).
- **Diff-check harness** `exportfunc_diff_check.sh` where feasible without the
  test's `${THIS_SH}` re-exec dance: Root 1 via `export -f foo-a; bash-vs-huck`
  round-trip using `$HUCK_BIN -c`; Root 3 the 18-heredoc line; Root 4 the two
  `export -f` rejects; Root 2 the eval line-number. (Some roots need a child
  `$HUCK_BIN -c` — use it directly.)
- **Unit tests** for `parse_imported_function`/import-name acceptance (Root 1,
  hyphen accepted; shellshock bodies still rejected), the heredoc-count limit
  (Root 3), and export-name rejection (Root 4).
- **No-regression:** full bash-suite runner PASS **35 → 36**, branch PASS-set
  diffed against the v347 baseline (exactly the 34... i.e. 35 + `exportfunc`;
  Root 1 touches env-function import — verify the exportfunc-adjacent categories
  and any function/env categories explicitly); `run_diff_checks.sh` green;
  per-crate lib tests + the relevant `-p huck` integration bins
  (`export_f_integration`, `declare_func_export_integration`, heredoc bins).

## Scope / non-goals

- Only the four roots. The shellshock CVE checks already pass and must stay.
- Root 1 must NOT relax the shellshock body validation in
  `parse_imported_function` — only the NAME-acceptance check.

## Summary of touched files

- `crates/huck-engine/src/shell_state.rs` — Root 1 import name-acceptance.
- `crates/huck-engine/src/builtins.rs` (or the export builtin module) — Root 4
  export-name validation.
- `crates/huck-syntax/src/lexer.rs` — Root 3 HEREDOC_MAX enforcement.
- The `eval` builtin site — Root 2 line-number base.
- `tests/scripts/exportfunc_diff_check.sh` (new).
