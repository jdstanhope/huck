# v329 — DEBUG trap fires on function entry (with the definition line)

Issue: [#274](https://github.com/jdstanhope/huck/issues/274) — third step of the `dbg-support` bash-suite category sub-arc.

## Problem

Under function-tracing (`set -T` or `shopt -s extdebug`), bash fires the DEBUG
trap once on **function entry** — after the call-site fire, before the first
body command — with `$LINENO` set to the function's **definition line**. huck
misses this fire. It is the dominant residual `debug lineno` class in the
`dbg-support` diff after v328.

Verified against bash 5.2.21 (`f` defined at line 3, called at line 4):
```sh
set -T
trap 'echo "D=$LINENO"' DEBUG
f() {          # line 3
  echo a       # line 4
}
f              # line 7 in the multi-line case / line 4 here
# bash: D=<call-line> D=3 D=4 …   huck: D=<call-line> D=4 …   (missing the D=3 entry fire)
```
The entry fire is a distinct fire even for a one-liner (`f() { echo a; }` on
line 3 → bash `D=<call> D=3 D=3`, two fires on line 3). It applies to both the
`f() {…}` and `function f {…}` forms, under `-T` or `extdebug`.

## Design

The entry fire needs the function's definition line, which huck does not store.
Thread it like the v325 clause lines.

### 1. Store the definition line

- **`crates/huck-syntax/src/command.rs`** — add `line: u32` to
  `Command::FunctionDef { name, body, line }`.
- **`crates/huck-syntax/src/parser.rs`** — capture the definition line at the
  start of the two funcdef parsers (`finish_function_body` and the
  `function`-keyword form): `let line = iter.line();` at the point the
  name/`function` keyword is seen (the `f`/`function` line), and set it in the
  `FunctionDef`. Zero it in `zero_lines_in_command`'s `FunctionDef` arm
  (currently only recurses into the body).
- **`crates/huck-engine/src/shell_state.rs`** — add
  `pub function_def_line: std::collections::HashMap<String, u32>` (parallel to
  `function_source`). In `define_function`, store the **absolute** definition
  line: `self.function_def_line.insert(name.clone(), self.line_base() + line)`
  (0 stays 0). Remove it in the function-removal path (mirror
  `function_source.remove`). `define_function` gains a `line: u32` parameter;
  its call site in `executor.rs` (`Command::FunctionDef { name, body, line }`)
  passes it.

### 2. Fire DEBUG on function entry

- **`crates/huck-engine/src/executor.rs`, `call_function`** — after
  `shell.call_stack.push(frame)` + `sync_call_arrays()` (so `FUNCNAME`/frame
  are set for the action) and the locals push, but BEFORE
  `run_command(&body, …)`, stamp the definition line and fire:
  ```rust
  if let Some(&def_line) = shell.function_def_line.get(name)
      && def_line != 0
  {
      shell.current_lineno = def_line;
  }
  match crate::traps::fire_debug_trap(shell) {
      crate::traps::DebugDecision::Proceed => {}
      crate::traps::DebugDecision::SkipCommand => { /* skip the body */ …return }
      crate::traps::DebugDecision::ReturnFromSub(n) => { …return FunctionReturn(n) }
  }
  ```
  `fire_debug_trap` already applies the v327 functrace/extdebug gate (the entry
  fire is inside the just-pushed Function frame, so it fires only under
  tracing) and the v328 RETURN-suppression composes. The extdebug
  skip/return handling at the entry fire is verified against bash during
  implementation; if `SkipCommand`/`ReturnFromSub` behavior at entry is
  intricate, scope the entry fire to `Proceed` (fire only) and file a follow-up
  (as #262 did for compound headers) — the firing + `$LINENO` is the #274 fix.

  Note: the frame must be pushed BEFORE the fire so the DEBUG action sees the
  correct `${FUNCNAME[…]}` and so the functrace gate sees the Function frame.
  The v322 `$LINENO` reframe (eval_frame = current_lineno) inside
  `fire_debug_trap` will use the stamped def line.

## Testing

Gate = bash 5.2.21 fidelity + `dbg-support` diff shrinkage.

1. **Bash-diff harness** `tests/scripts/debug_function_entry_diff_check.sh`
   (model on `trap_zero_diff_check.sh`). Cases (byte-identical incl. exit),
   DEBUG action `echo "D=$LINENO"`:
   - multi-line function under `-T`: entry fire on the def line.
   - one-line function: entry fire on the def line (a distinct fire even when
     the body is the same line).
   - `function f {…}` keyword form.
   - under `extdebug` (not `-T`).
   - nested functions (`f` calls `g`): each gets its entry fire on its own def
     line.
   - NO tracing: no entry fire (regression guard from v327).
   - `${FUNCNAME[1]}` in the action at the entry fire matches bash.
2. **`dbg-support` diff shrinkage**: re-run `HUCK_BASH_TEST_CATEGORY=dbg-support`
   and record the new size (expect a large drop from ~635 — the ~312
   `debug lineno` entry-fire class collapses). Note the residual (`caller`,
   `$LINENO`) for the sub-arc's next iterations.
3. **Regression**: `dbg-support2` stays PASS (its DEBUG fires are top-level, no
   function entry under tracing — confirm); the DEBUG firing-count /
   extdebug-skip / lineno-fidelity / functrace / return-in-trap-action
   harnesses stay green (a function called top-level without tracing gets no
   entry fire — check each, add `set -T` where a fragment intends in-function
   firing); `trap_integration` / `trap_pseudo_signals_integration` /
   `functions_integration` / `funcname` green; full `run_diff_checks.sh` sweep
   green; huck-engine + huck-syntax lib green.

Per repo constraints: build with `cargo build -p huck`; per-crate tests
single-threaded; guard sweeps with `ulimit -v 1500000` + `timeout`; run the
`-p huck` trap/function integration binaries single-threaded before push; NO
GPL bash text.

## Scope

**In scope.** Storing the function definition line (parser + `FunctionDef` +
`function_def_line` map + `define_function`); the entry DEBUG fire in
`call_function` with the def line; the harness; the `dbg-support` measurement;
regressions.

**Out of scope (later sub-arc iterations).** The `caller` builtin;
`$LINENO`-in-trap fidelity residuals; `$BASH_COMMAND` at the entry fire
(unimplemented). If the entry-fire extdebug skip/return is deferred, it is a
follow-up.

## Documentation

- Removes a divergence (no new intentional one). #274 auto-closes via the PR
  body (`Closes #274`).
