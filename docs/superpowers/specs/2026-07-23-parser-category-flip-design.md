# v331 — Flip the `parser` bash-suite category to PASS

Issues: [#27](https://github.com/jdstanhope/huck/issues/27) — parser syntax
errors don't match bash's `near unexpected token` format (covers fixes 1–3);
[#283](https://github.com/jdstanhope/huck/issues/283) — non-interactive syntax
error recovers across a newline where bash aborts (fix 4).

## Problem

The `parser` bash-suite category is a near-miss: a full deep-dig found exactly
**four** independent divergences (parser.diff = 13 lines). All four are verified
byte-identical to bash 5.2.21 after the fixes below; together they take the
category to **0 diff → PASS** (Summary PASS 19→20, FAIL 63→62).

Each divergence, with the bash-verified before/after:

### 1. `for <bad-name>` runtime error missing the `line N:` prefix

`for` accepts any word as the loop variable at parse time; a non-identifier is a
runtime error (status 1, body not run, surrounding list continues). bash stamps
the for-header line into the diagnostic; huck did not (compound commands don't
stamp `current_lineno`), so the `line N:` prefix was absent.

```console
$ bash -c $'for 1x in a; do :; done'
bash: line 1: `1x': not a valid identifier     # rc 1

$ huck (before): huck: `1x': not a valid identifier    # missing `line 1:`
```

### 2. Concrete wrong token where a keyword was required → wrong error shape

When a compound command requires a keyword (`then`/`do`/`done`/`fi`/`esac`/`in`/
`}`) but a concrete *wrong* token is present, bash reports a near-token error
(`syntax error near unexpected token \`X'`). huck reported the
unterminated/unexpected-**EOF** shape instead (`expect_or_recover` only had an
EOF-recovery branch and a hard "unterminated" error).

```console
$ bash -c 'case x in in do do) :; esac'
bash: -c: line 1: syntax error near unexpected token `do'
bash: -c: line 1: `case x in in do do) :; esac'   # rc 2
```

### 3. `for(` (a single `(`, not `((`) → wrong error shape

After ruling out the `((`-arithmetic-for form, a lone `(` following `for` is a
syntax error *at the `(`*. huck fell through to the loop-variable reader and
emitted `invalid variable name in 'for' loop` instead of bash's near-token
error.

```console
$ bash -c 'for()'
bash: -c: line 1: syntax error near unexpected token `('
bash: -c: line 1: `for()'    # rc 2
```

### 4. Non-interactive syntax error: recover-across-newline vs abort

On a **regular parse error** in a non-interactive context (`-c` string, script
file, sourced file), bash aborts the entire parse-context: commands before ran,
commands after never run, rc 2. huck's driver loop skipped the offending line
and resumed, running commands that follow a syntax error. This is the last
residual — a script fragment (`posix2syntax.sub`) whose syntax error is followed
by a group command was the visible diff line.

```console
$ bash -c $'echo a\nfor()\necho b'
a
bash: -c: line 2: syntax error near unexpected token `('
bash: -c: line 2: `for()'      # rc 2, no "b"

$ huck (before): … prints "b" too, rc 0
```

Subtleties (all verified against bash): a **sourced** file aborts its own
remainder but the **parent** continues (`source bad; echo x` runs `echo x`);
the **same-line** case (`echo a; for(); echo b`) already matched bash only
incidentally (no newline to skip to → recovery ran to EOF); **lex** errors
(unterminated quote/`$(`/heredoc) already abort correctly (the construct
consumes to EOF, so nothing follows to recover into).

## Design

Four surgical, independent edits. Each is prototype-verified byte-identical to
bash; the exact code appears in the plan.

### 1. `run_for_inner` (`crates/huck-engine/src/executor.rs`)

Before emitting the not-a-valid-identifier error, stamp the header line so the
diagnostic carries bash's `line N:` prefix:

```rust
if clause.line != 0 {
    shell.current_lineno = shell.line_base() + clause.line;
}
```

`Command::For` already carries `clause.line` (v325 added compound-header lines);
`line_base()` folds in the eval/stdin base exactly as the DEBUG fires do.

### 2 + 3. `expect_or_recover` and `parse_for` (`crates/huck-syntax/src/parser.rs`)

**`expect_or_recover`**: add a branch *before* the EOF-recovery branch — when a
concrete token is present and it is **not** a recovery-close delimiter, emit a
near-token error rather than the unterminated shape:

```rust
} else if iter.peek_kind()?.is_some() && !iter.peek_is_recovery_close()? {
    Err(ParseError::Unexpected(iter.unexpected_here(None)?))
}
```

The `peek_kind().is_some()` guard preserves the truncated-inner-mode EOF
recovery (`echo $(if true; then echo hi` — EOF → `peek_kind` is `None` → falls
through to the existing `recover_at_eof && peek_is_recovery_close` branch). This
function is shared across `if`/`while`/`case`/brace; the guard is why the change
is safe there (verified: the 475 huck-syntax lib tests stay green).

**`parse_for`**: after the `((`-arith-for check, reject a lone `(`:

```rust
if matches!(iter.peek_kind()?, Some(TokenKind::Op(Operator::LParen))) {
    return Err(ParseError::Unexpected(iter.unexpected_here(None)?));
}
```

### 4. `run_sourced_contents_in_sinks_inner` (`crates/huck-engine/src/builtins.rs`)

In the regular parse-error arm (after the lex-error `if is_lex { … }` early
restart, before the skip-to-newline recovery), abort the parse-context when
non-interactive:

```rust
if !shell.is_interactive {
    return ExecOutcome::Continue(2);
}
```

Because each `-c` string / script / `source` runs its own
`run_sourced_contents_in_sinks` invocation, the early return aborts **this**
context while a parent driver loop (for a sourced file) continues — reproducing
bash's source-aborts-remainder / parent-continues behavior with no extra
machinery. Interactive `source`/`.`/rc keep the skip-and-continue recovery (the
same `!is_interactive` gate the existing fatal-expansion drain uses a few lines
above). `Continue(2)` maps to exit status 2 at `run_program_in_sinks`.

## Testing

Gate = bash 5.2.21 fidelity + the `parser` category at 0 diff.

1. **Bash-diff harness** `tests/scripts/parser_syntax_errors_diff_check.sh`
   (model on an existing `-c` harness), comparing `bash --norc --noprofile`
   vs `"$HUCK_BIN"` byte-identical incl. stderr and `EXIT:$?`. Cases:
   - `for 1x in a; do :; done` → `line N:` prefix, rc 1 (fix 1).
   - `case x in in do do) :; esac` → near-token `do`, rc 2 (fix 2).
   - `for()` / `for()\ntrue` → near-token `(`, rc 2 (fix 3).
   - multi-line `-c` with a mid-string syntax error → aborts, no trailing
     command runs, rc 2 (fix 4).
   - a **script file** with a mid-file syntax error → aborts, rc 2 (fix 4).
   - a **sourced** file whose syntax error is followed by more lines, then a
     parent command → sourced remainder aborted, parent runs, rc 0 (fix 4).
   - **regression guards**: a valid multi-command `-c` still runs every command;
     an interactive-style keyword-recovery case (`echo $(if true; then echo hi`)
     still recovers (fix 2's EOF guard); a same-line `for(); echo b` still
     aborts (unchanged).
2. **`parser` category** flips: `HUCK_BASH_TEST_CATEGORY=parser` → PASS, 0 diff
   (was 13 lines).
3. **Regression**: huck-syntax + huck-engine lib green; the `-p huck` parser /
   error / syntax integration binaries green; `syntax_error_diag`, `posix_mode`,
   and the full `run_diff_checks.sh` sweep green (coproc flake pre-existing);
   `dbg-support2`/`rhs-exp`/`procsub`/`posix2` stay PASS (no category regressed).

Per repo constraints: build with `cargo build -p huck`; per-crate tests
single-threaded; NEVER `cargo test --workspace`; guard sweeps with
`ulimit -v 1500000` + `timeout`; run the `-p huck` integration binaries
single-threaded before push; NO GPL bash text.

## Scope

**In scope.** The four fixes; the harness; the category flip; regressions.

**Out of scope.** Other parser near-token shapes not in the `parser` category;
the opposite-direction backtick recover-vs-abort (#25/#215); non-interactive
piped-stdin line numbers (#79); any broader syntax-error-recovery redesign.

## Documentation

- Both #27 and #283 auto-close via the PR body (`Closes #27`, `Closes #283`).
  No intentional divergence added (`docs/bash-divergences.md` unchanged).
- Record the iteration in `project_huck_iterations.md` + `MEMORY.md`.
- Update `docs/bash-test-suite-baseline.md` (a "Updated by v331" note: `parser`
  PASS, Summary PASS 19→20, FAIL 63→62).
