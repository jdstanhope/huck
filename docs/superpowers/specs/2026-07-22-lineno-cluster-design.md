# v325 — `$LINENO` fidelity cluster: piped stdin, multi-line eval, compound-header DEBUG fires

Issues: [#266](https://github.com/jdstanhope/huck/issues/266) (stdin),
[#258](https://github.com/jdstanhope/huck/issues/258) (eval),
[#261](https://github.com/jdstanhope/huck/issues/261) (compound headers)

## Problem

Three independent `$LINENO` divergences from bash 5.2.21, verified via **file**
execution (piped stdin has its own bug — part A — that contaminates
stdin-based probes):

- **A (#266) — piped stdin resets `$LINENO` per line.** A script from a pipe /
  redirect reports `$LINENO` as 1 on every line; the same script run as a FILE
  or via `-c` is correct.
  ```
  huck s.sh    -> 1 2 3   (file: correct)
  huck < s.sh  -> 1 1 1   (stdin: WRONG; bash -> 1 2 3)
  ```
- **B (#258) — multi-line eval body `$LINENO` off by an offset.** For an eval
  on line E with a body of `total` lines, huck reports `E - 1 + K` for body
  line K; bash reports `E + total - 1 + K - 1` (i.e. huck is low by
  `total - 1`; single-line evals match).
- **C (#261) — compound-header DEBUG fire reports the wrong line.** The v324
  DEBUG fires at `for`/`select`/`case`/arith-for headers stamp whatever
  `current_lineno` happens to hold (the last body command's line, or 1), not
  the header's line. bash reports the header line each time.
  ```
  trap 'echo "L$LINENO"' DEBUG; for x in 1 2\n do\n echo hi\n done
  bash -> L2,L4,...   huck -> L1,L4,... then L4,L4  (header should be L2)
  ```

## Design

The three fixes touch three different subsystems (REPL reader, eval, parser +
executor) and share nothing but the `eval_frame`/`line_base()` mechanism.

### A — cumulative `$LINENO` for piped stdin (`crates/huck-cli/src/repl.rs`)

The non-interactive stdin REPL reads one logical command at a time
(`read_logical_command`) and calls `process_line(&buffer, …)` on each — every
call re-parses from line 1, so `current_lineno = line_base() + cmd.line` starts
over. The FILE path reads the whole script and calls `process_line` once, so
its `cmd.line` values are absolute (correct).

Maintain a cumulative physical-line counter in the REPL loop, **only when
`!shell.is_interactive`** (interactive bash resets `$LINENO` per command, which
huck already does). Before each `process_line`, set the line base so the
command's line-K maps to `lines_before + K`; after, advance the counter by the
buffer's physical line count.

Reuse the existing `eval_frame`/`line_base()` carrier
(`line_base() = eval_frame.saturating_sub(1)`): set
`shell.eval_frame = Some(lines_before + 1)` before `process_line` (so
`line_base() = lines_before`), then
`lines_before += buffer_physical_line_count` after. `eval_frame` is `None` at
top level otherwise, and an `eval`/`source` inside the script saves and
restores it around its own frame, so this composes. Count buffer physical lines
as `buffer.bytes().filter(|&b| b == b'\n').count()` plus 1 if the buffer does
not end in a newline (a logical command may span several physical lines).

**Scope note:** if a `read` builtin consumes a subsequent script line from the
same stdin, the REPL counter does not see that consumed line, so a later
command's `$LINENO` can drift. That `read`-consumes-a-line edge is out of scope
(a follow-up); the common multi-line-script case is the fix.

### B — multi-line eval body offset (`crates/huck-engine/src/builtins.rs`, `eval_in_sink` ~6841)

Derived bash model (9-case truth table): `LINENO(body line K) = E + N + K - 1`,
where `E` = the eval command's line and `N` = the number of newlines in the
eval body. huck sets `eval_frame = Some(current_lineno.max(1))` (so
`line_base() = E - 1`, giving `LINENO(K) = E - 1 + K` — low by `N`). Fix: add
the body's newline count.

```rust
// was: shell.eval_frame = Some(shell.current_lineno.max(1));
let body_newlines = joined.bytes().filter(|&b| b == b'\n').count() as u32;
shell.eval_frame = Some(shell.current_lineno.max(1) + body_newlines);
```

Verification against the model: `E - 1 + body_newlines + K = E + N + K - 1`. ✓
(single-line body: `N = 0`, unchanged — matches the already-correct case).

### C — compound-header line for DEBUG fires (huck-syntax + huck-engine)

The v324 DEBUG fires at compound headers need the header's own line. Add a
`line: u32` field to the four compound clauses and stamp it before the header
fire.

1. **Clause structs** (`crates/huck-syntax/src/command.rs`): add `pub line:
   u32` to `ForClause`, `ArithForClause`, `CaseClause`, `SelectClause` (mirror
   `ExecCommand.line`).
2. **Parser** (`crates/huck-syntax/src/parser.rs`): in `parse_for` (~4647),
   `parse_arith_for_clause` (~4605), `parse_select` (~4754), `parse_case`
   (~4832), capture `let line = iter.line();` at entry, BEFORE consuming the
   `for`/`case`/`select` keyword (as `parse_simple_with_leading_word` captures
   the command's start line), and set `line` in the constructed clause.
3. **`zero_lines_in_command`** (`parser.rs` ~1553): zero the four new clause
   `line` fields too, so the line-stripping path (`-c` / comsub bodies, which
   carry `line: 0`) stays consistent.
4. **Executor** (`crates/huck-engine/src/executor.rs`): immediately before each
   compound-header DEBUG fire added in v324 — the per-iteration fire in
   `run_for_inner`/`run_select_inner`, the entry fire in `run_case_inner`, and
   the init/cond/step fires in `run_arith_for_inner` — stamp the header line
   when `clause.line != 0`:
   ```rust
   if clause.line != 0 {
       shell.current_lineno = shell.line_base() + clause.line;
   }
   let _ = crate::traps::fire_debug_trap(shell);
   ```
   The arith-for init/cond/step fires all use the clause line (bash reports the
   `for ((…))` line for each). Body simple commands re-stamp their own line
   through the normal path, so the next header fire re-stamps correctly.

`current_lineno` reframe interaction: under part A / eval, `line_base()` is
non-zero, and `line_base() + clause.line` yields the correct absolute line, so
C composes with A and B.

## Testing

Gate = bash 5.2.21 fidelity, tested via **files** and **stdin** (not just `-c`).

1. **Bash-diff harness** `tests/scripts/lineno_fidelity_diff_check.sh`. Model
   on `trap_zero_diff_check.sh`, but add a helper that writes each fragment to a
   temp file and runs BOTH shells (a) as a file arg and (b) via `< file`
   (stdin), comparing byte-identical output. Cases:
   - A: a 3-line `echo $LINENO` script — file AND stdin both `1 2 3`.
   - A: multi-line commands (a for-loop body `echo $LINENO`) via stdin —
     absolute lines match.
   - B: `eval` at lines 1/2/5 with 1/2/3-line bodies (the truth-table cases) —
     `$LINENO` matches bash.
   - C: `trap 'echo "L$LINENO"' DEBUG` + a multi-line `for`/`case`/`select`/
     arith-for — the header-fire lines match bash (L2/L4-style).
   - regressions: `-c` multi-line still correct; function-body `$LINENO`
     unchanged; `dbg-support2`-shape still works; a lone command's line.
2. **Unit tests**: B's newline-offset formula (a small `eval_in_sink`-level or
   pure-helper test); the parser captures a non-zero `line` for each clause;
   `zero_lines_in_command` zeros them.
3. **Regression guards**: `dbg-support2` category stays PASS; the DEBUG
   firing-count harness (`debug_firing_points_diff_check.sh`) stays green (C
   changes the line, not the count); `bash_source_lineno` / `lineno` /
   `script_line_numbers_integration` / `eval` integration binaries green; full
   `run_diff_checks.sh` sweep green; huck-syntax + huck-engine lib suites green.

Per repo constraints: build with `cargo build -p huck`; per-crate tests
single-threaded; guard sweeps with `ulimit -v 1500000` + `timeout`; run the
`-p huck` lineno/eval/trap integration binaries single-threaded before push;
NO GPL bash text in the repo.

## Scope

**In scope.** A (stdin cumulative line base), B (eval body newline offset), C
(compound clause `line` fields + parser capture + executor stamp before the
header fires). The harness + unit tests + regressions.

**Out of scope.** The `read`-consumes-a-stdin-line `$LINENO` edge (A follow-up
if it diverges); `$LINENO` for the compound-header fires under extdebug
skip/return (#262 territory); `BASH_LINENO`/`BASH_SOURCE` arrays; the general
`caller` builtin (part of the larger `dbg-support` category, not this cluster).

## Documentation

- `docs/architecture.md`: if `$LINENO`/`eval_frame` are described, note the
  piped-stdin cumulative base, the eval body-newline offset, and that compound
  clauses carry a source line consumed by the DEBUG header fires.
- Removes divergences (no new intentional one); the PR closes #266, #258, #261
  (`Closes #266`, `Closes #258`, `Closes #261`).
