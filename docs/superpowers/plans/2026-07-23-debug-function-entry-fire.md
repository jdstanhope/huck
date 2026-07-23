# v329 — DEBUG on function entry Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fire the DEBUG trap on function entry (under `set -T`/`extdebug`) with `$LINENO` = the function's definition line — the third step of the `dbg-support` sub-arc (should collapse the ~312 `debug lineno` residual).

**Architecture:** Store the function definition line (add `line` to `FunctionDef`, capture in the parser, store an absolute line in a `function_def_line` map at `define_function`), then fire the entry DEBUG in `call_function` after the frame push, before the body.

**Tech Stack:** Rust; huck-syntax + huck-engine; bash-diff harness.

Spec: `docs/superpowers/specs/2026-07-23-debug-function-entry-fire-design.md`
Issue: [#274](https://github.com/jdstanhope/huck/issues/274)

## Global Constraints

- bash 5.2.21: under tracing, exactly one DEBUG entry fire per function call, after the call-site fire and before the first body command, with `$LINENO` = the definition line (the `f`/`function` line). No entry fire without tracing. `fire_debug_trap` already applies the v327 functrace/extdebug gate.
- Do NOT change: the v327 gate, v328 RETURN-suppression, the extdebug decision path, the body-command fires.
- Commit trailer; `cargo fmt --all` before committing. Per repo memory: build with `cargo build -p huck`; per-crate tests single-threaded; NEVER `cargo test --workspace` (OOMs); guard sweeps with `ulimit -v 1500000` + `timeout`; run `-p huck` trap/function integration bins single-threaded before push; NO GPL bash text; no `Closes #N` in the commit (bare `#N`).

---

### Task 1: Store the function definition line + fire DEBUG on entry

**Files:**
- Modify: `crates/huck-syntax/src/command.rs` (`FunctionDef` line field)
- Modify: `crates/huck-syntax/src/parser.rs` (capture the def line in both funcdef paths; zero it)
- Modify: `crates/huck-engine/src/shell_state.rs` (`function_def_line` map; `define_function` param + store/remove)
- Modify: `crates/huck-engine/src/executor.rs` (`Command::FunctionDef` passes the line; entry fire in `call_function`)
- Create: `tests/scripts/debug_function_entry_diff_check.sh`

- [ ] **Step 1: Write the harness (red)**

Create `tests/scripts/debug_function_entry_diff_check.sh` (model on `trap_zero_diff_check.sh`), `check "label" 'frag'` comparing `bash --norc --noprofile -c "$frag"` vs `"$HUCK_BIN" -c "$frag"` byte-identical incl. `EXIT:$?`. DEBUG action `echo "D=$LINENO"`. Cases (a `-c` string is one line, so `$LINENO` is 1 there — use `check` with fragments that put the function on a KNOWN line via embedded newlines; a `-c` multi-line string counts newlines, so `$LINENO` increments — verify the expected values against bash first):
```sh
check "entry multiline -T"  'set -T
trap "echo D=\$LINENO" DEBUG
f() {
  echo a
}
f'
check "entry oneline -T"    'set -T
trap "echo D=\$LINENO" DEBUG
f() { echo a; }
f'
check "entry function-kw"   'shopt -s extdebug
trap "echo D=\$LINENO" DEBUG
function f {
  echo a
}
f'
check "entry nested -T"     'set -T
trap "echo D=\$LINENO" DEBUG
g() { echo g; }
f() { g; }
f'
check "no-tracing no-entry" 'trap "echo D=\$LINENO" DEBUG
f() { echo a; }
f'
check "funcname at entry"   'set -T
trap "echo \${FUNCNAME[0]:-main}" DEBUG
f() { echo a; }
f'
```
Build (`cargo build -p huck`) and run — the entry cases FAIL (huck misses the entry fire). Confirm each fragment's expected output against `bash --norc` first.

- [ ] **Step 2: Add `line` to `FunctionDef`**

In `command.rs`, change `FunctionDef { name, body }` → `FunctionDef { name, body, line: u32 }`. The compiler lists every construction/match to fix.

- [ ] **Step 3: Capture the def line in the parser**

- `finish_function_body(name, iter)` → add a `line: u32` param and set it in the `FunctionDef`.
- The `name () {…}` form: its caller already has the command start `line` (the line captured before the name word — the same value passed to `parse_simple_with_leading_word`). Pass it to `finish_function_body`.
- `parse_function_keyword_def(iter)`: capture `let line = iter.line();` at entry (the `function` keyword line), pass to `finish_function_body`.
- `zero_lines_in_command`'s `FunctionDef` arm: also set the `line = 0` (currently it only recurses into the body).

Verify the captured line is the `f`/`function` line by comparing against bash's entry `$LINENO` in the harness.

- [ ] **Step 4: Store the absolute def line**

In `shell_state.rs`: add `pub function_def_line: std::collections::HashMap<String, u32>` (init empty, parallel to `function_source`). `define_function` gains a `line: u32` param; store `self.function_def_line.insert(name.clone(), if line == 0 { 0 } else { self.line_base() + line })`. In the function-removal path (where `function_source.remove` is called), also `self.function_def_line.remove(name)`. Update the `define_function` call in `executor.rs`'s `Command::FunctionDef` arm to destructure `line` and pass it.

- [ ] **Step 5: Fire DEBUG on entry in `call_function`**

In `call_function` (executor.rs), after `shell.call_stack.push(frame)` + `sync_call_arrays()` + the `local_scopes.push`, and BEFORE `run_command(&body, …)`:
```rust
if let Some(&def_line) = shell.function_def_line.get(name)
    && def_line != 0
{
    shell.current_lineno = def_line;
}
match crate::traps::fire_debug_trap(shell) {
    crate::traps::DebugDecision::Proceed => {}
    crate::traps::DebugDecision::SkipCommand => {
        // skip the body: pop frame/locals like the normal exit and return.
        // (Match the existing exit-cleanup at the end of call_function.)
        // If honoring skip/return at entry proves intricate, fire with
        // `let _ =` (Proceed only) and file a follow-up — the FIRE + $LINENO is
        // the #274 target. Decide against bash: `set -T; shopt -s extdebug;
        // tr(){ …return 1;}; trap tr DEBUG; f(){ echo a; }; f; echo $?`.
    }
    crate::traps::DebugDecision::ReturnFromSub(n) => { /* FunctionReturn via the normal path */ }
}
```
Simplest correct approach if skip/return at entry is deferred: `let _ = crate::traps::fire_debug_trap(shell);` (fire only), and file a follow-up issue for entry-fire extdebug skip/return. Confirm the deferred choice against bash and note it.

The frame is pushed before the fire so the action sees the right `FUNCNAME` and the functrace gate sees the Function frame; `fire_debug_trap`'s `$LINENO` reframe uses the stamped def line.

- [ ] **Step 6: Confirm the harness passes** vs bash (entry fires, def line, no-tracing, funcname). Iterate the def-line capture until the `$LINENO` values match bash.

- [ ] **Step 7: Regression + dbg-support measurement**
```bash
cargo test -p huck-syntax --lib --jobs 1 -- --test-threads 1   # green (fix FunctionDef constructors in tests)
cargo test -p huck-engine --lib --jobs 1 -- --test-threads 1   # green
ulimit -v 1500000
for h in debug_function_entry functrace return_in_trap_action debug_firing_points extdebug_skip lineno_fidelity; do
  bash tests/scripts/${h}_diff_check.sh >/dev/null 2>&1 && echo "$h OK" || echo "$h FAIL"
done
for t in trap_integration trap_pseudo_signals_integration functions_integration funcname; do
  cargo test -p huck --test "$t" --jobs 1 -- --test-threads 1 2>&1 | grep "test result"
done
ulimit -v 2000000
HUCK_BASH_TEST_CATEGORY=dbg-support2 HUCK_TEST_TIMEOUT=90 BASH_SOURCE_DIR=/tmp/bash-5.2.21 \
  timeout 200 bash tests/bash-test-suite/runner.sh 2>&1 | grep -iE "dbg-support2 \|"   # PASS
HUCK_BASH_TEST_CATEGORY=dbg-support HUCK_TEST_TIMEOUT=120 BASH_SOURCE_DIR=/tmp/bash-5.2.21 \
  timeout 240 bash tests/bash-test-suite/runner.sh > /tmp/dbgs.md 2>&1
SC=$(grep -oE "/tmp/huck-bash-tests[^ ]*" /tmp/dbgs.md | head -1)
echo "dbg-support diff: $(wc -l < $SC/dbg-support.diff) lines (was ~635)"
```
dbg-support2 MUST stay PASS; dbg-support should drop substantially from ~635.

- [ ] **Step 8: Full sweep**
```bash
cargo build --release -p huck
ulimit -v 1500000; timeout 550 bash tests/scripts/run_diff_checks.sh   # green (coproc flake pre-existing)
```

- [ ] **Step 9: fmt + commit**
```bash
cargo fmt --all
git add crates/huck-syntax/src/command.rs crates/huck-syntax/src/parser.rs crates/huck-engine/src/shell_state.rs crates/huck-engine/src/executor.rs tests/scripts/debug_function_entry_diff_check.sh
git commit -m "$(cat <<'EOF'
v329: DEBUG trap fires on function entry with the definition line (#274)

Under function-tracing, bash fires DEBUG once on function entry ($LINENO = the
definition line); huck missed it. Store the def line (FunctionDef.line ->
function_def_line map) and fire the entry DEBUG in call_function after the frame
push. The dominant remaining debug-lineno class in the dbg-support category.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-Review

- **Spec coverage:** def-line storage (command.rs/parser/shell_state), the entry fire (executor), harness, dbg-support measurement, regressions — all Task 1.
- **Placeholders:** the entry-fire skip/return handling is explicitly allowed to be deferred to Proceed-only with a follow-up if intricate (the FIRE + `$LINENO` is the target).
- **Type consistency:** `FunctionDef { …, line: u32 }`; `function_def_line: HashMap<String,u32>`; `define_function(name, body, line)`; `iter.line() -> u32`.
- **Scope:** entry fire + def line only; caller / `$LINENO`-in-trap residuals are later sub-arc iterations.
