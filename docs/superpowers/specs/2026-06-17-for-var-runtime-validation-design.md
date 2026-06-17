# v180: for-loop variable name — parse-permissive, runtime-validated — Design

**Status:** approved 2026-06-17
**Iteration:** v180
**Origin:** The parse sweep's "invalid variable name in 'for' loop" cluster
(ethtool_rmon.sh, fcnal-test.sh, interop_test.sh). Root cause traced to
`for if in $iface $neigh; do …` — the loop variable is the reserved keyword
`if`, which bash accepts (it's a valid identifier; keyword status only matters in
command position) but huck rejects at parse time. A sibling case `for a-b in …`
(non-identifier name) is the true "parse-vs-runtime" divergence: bash parses it
and errors at runtime, huck errors at parse.

## Problem

`for_variable_name` (`src/command.rs:1382`) delegates to `valid_identifier_text`,
which at PARSE time requires the loop variable to be a single unquoted `Literal`
that is a valid identifier (`[A-Za-z_][A-Za-z0-9_]*`) AND not a reserved keyword.
bash does neither at parse time: it accepts any word as the loop variable and
validates the identifier at RUNTIME.

Confirmed vs bash:
- `for if in 1 2; do echo $if; done` — bash rc 0, runs (`if`=1,2); huck rc 2,
  parse error. (`if=5; echo $if` already works in huck — only the for-loop var
  path wrongly excludes keywords.)
- `for in in a; do echo $in; done` — bash runs; huck parse error.
- `for a-b in 1; do echo x; done; echo after` — bash: prints
  `` `a-b': not a valid identifier``, **skips the body**, status 1, and `after`
  **runs** (non-fatal); huck: parse error rc 2.
- `for 1x in 1; …` — same as `a-b` (bash runtime error, status 1, non-fatal).

So bash: NAME is accepted at parse as any word; at runtime NAME must be a valid
identifier (keywords qualify — they match the identifier charset), else a
non-fatal "not a valid identifier" error (status 1, body not run, list continues).

## Goal

Match bash: accept any word as the loop variable at parse, validate the
identifier at runtime. This clears the corpus scripts (all use `for if in`) and
fixes the non-identifier parse-vs-runtime case.

## Design

### 1. Parser — accept any single word (`command.rs:1382`)

```rust
/// Returns the raw loop-variable name if `token` is a single, unquoted,
/// non-empty `Literal` `Word`. bash accepts ANY word as the `for` variable at
/// parse time (including reserved words like `if`, and non-identifiers like
/// `a-b`); the identifier rule is enforced at RUNTIME (`run_for`). So this does
/// NOT apply the keyword / charset checks of `valid_identifier_text`.
fn for_variable_name(token: &Token) -> Option<String> {
    let Token::Word(w) = token else { return None };
    if w.0.len() != 1 {
        return None;
    }
    let WordPart::Literal { text, quoted: false } = &w.0[0] else {
        return None;
    };
    if text.is_empty() {
        return None;
    }
    Some(text.clone())
}
```
(`valid_identifier_text` is unchanged — still used by coproc names. The
`ForClause.var` doc comment at `command.rs:647` changes from "a validated
identifier" to "the raw loop variable name; identifier-validated at runtime".)

### 2. Executor — validate at runtime (`run_for`, `executor.rs:1218`)

Make `is_valid_name` (`builtins.rs:591`, the `[A-Za-z_][A-Za-z0-9_]*` charset
check — reserved words like `if`/`in` pass it) `pub(crate)`. At the top of
`run_for` (before any iteration / list expansion that the body depends on — match
bash, which reports the bad name before running the body):

```rust
    if !crate::builtins::is_valid_name(&clause.var) {
        eprintln!("huck: `{}': not a valid identifier", clause.var);
        return ExecOutcome::Continue(1);
    }
```
This is non-fatal (`Continue`, not the fatal-PE/`Exit` path), so the surrounding
list continues — matching bash (`… ; echo after` still runs `after`). A valid
name (including a keyword) falls through to the existing loop logic unchanged.

### Behavior

- `for if in 1 2; do echo $if; done` → runs (`if`=1 then 2), rc 0 — like bash.
- `for in in a; do echo $in; done` → runs — like bash.
- `for a-b in 1; do echo x; done; echo after` → `` huck: `a-b': not a valid
  identifier`` on stderr, body NOT run, for-loop status 1, `after` runs — like
  bash (non-fatal, status 1).
- Valid loops (`for x in …`, `for x; do`, `for ((;;))`, empty list) unchanged.

## Verification

- **New bash-diff harness** `tests/scripts/for_var_name_diff_check.sh` (executing,
  stdout+exit; stderr discarded — the `huck:` vs `bash:` wording differs by the
  intentional prefix convention): keyword names run and match (`for if in a b`,
  `for in in x`); non-identifier names error non-fatally (`for a-b in 1; do echo
  body; done; echo after` → no `body`, `after` present, exit code matches);
  `for 1x in …`; and valid controls (`for x in 1 2`, `for x; do` over positional
  args, empty `for x in; do`).
- **Parse-sweep:** re-run `tools/parse_sweep.sh tools/scripts.tsv`; confirm
  ethtool_rmon.sh / fcnal-test.sh / interop_test.sh now parse (report any that
  still fail on a *different* construct — a derail beyond this fix). Report
  `HUCK_GAP` before/after; `HUCK_LENIENT`/`HUCK_CRASH` stay 0.
- **Full `cargo test`** (0 failures). UP-FRONT (v178 lesson) grep all of `tests/`
  + `src/` for tests asserting the old parse-rejection of a for-loop var —
  notably `tests/for_integration.rs` `for_invalid_variable_is_nonfatal_syntax_error`
  and any `for_variable_name`/parse unit test feeding `for a-b in`/`for 1x in`.
  Update those that encode the pre-fix parse-error to the new behavior (parses;
  runtime non-fatal "not a valid identifier"); do not weaken unrelated tests.
- All `tests/scripts/*_diff_check.sh` harnesses green; clippy clean.

## Scope boundary

In scope: `for_variable_name` (parse-permissive), `run_for` runtime
identifier-validation, `is_valid_name` visibility, the doc comments, the new
harness, and updating old-behavior tests. **Not** in scope: `valid_identifier_text`
or coproc-name validation (unchanged); arith-for `for ((;;))` (separate path,
already correct); the error-message *wording* (intentional `huck:`-prefix
family); the bash-vs-huck rc value for OTHER runtime errors. No
`bash-divergences.md` change (this cluster was sweep-found, not a tracked
divergence). Record in `project_huck_iterations.md` + `MEMORY.md`.
