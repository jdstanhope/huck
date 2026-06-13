# huck v153 — `BASH_SOURCE` / `BASH_LINENO` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Implement bash's `BASH_SOURCE` / `BASH_LINENO` call-stack arrays (parallel to `FUNCNAME`), with full base-case parity and tab completion; fix v151's missing-`main`-frame `FUNCNAME` in script mode.

**Architecture:** Replace v151's `function_arg0: Vec<String>` with a unified `call_stack: Vec<Frame>` (function / `source` / base-`main` frames). A `sync_call_arrays` materializes `FUNCNAME`/`BASH_SOURCE`/`BASH_LINENO` into the vars table from the stack on every frame change (so they tab-complete and show in `declare -p`). Per-function def-source via a new `function_source` map; call-site lines from v152's `current_lineno`.

**Tech Stack:** Rust.

**Reference:** spec at `docs/superpowers/specs/2026-06-13-bash-source-lineno-design.md`.

**GIT SAFETY:** Do NOT `git checkout <sha>`. Stay on `v153-bash-source-lineno`. Commit trailer: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

**Verified facts:**
- v151's `Shell.function_arg0: Vec<String>` (shell_state.rs:284, init :476) is the function-name stack. Uses: `call_function` push/pop (executor.rs:2925/2953) each followed by `shell.sync_funcname()` (:2927/:2954); `$0` in `lookup_var` (shell_state.rs:559 `self.function_arg0.last().cloned().unwrap_or_else(|| self.shell_argv0.clone())`); and `expand.rs:220`.
- `Shell::sync_funcname` (shell_state.rs:1122) rebuilds the `FUNCNAME` `VarValue::Indexed` from `function_arg0` (reversed), or removes it when empty. Mirrors `set_pipestatus` (shell_state.rs:1093).
- `Shell.current_lineno: u32` (v152) holds the executing command's line; `run_exec_single` sets it before dispatch, so at a `call_function` push it equals the calling command's line (= the call-site line).
- `define_function` (shell_state.rs:1138); `remove_function` exists (unset -f). `functions: Rc<HashMap<String, Box<Command>>>` (COW-cloned).
- Script files run via `crate::builtins::run_sourced_contents(&contents, &path, shell)` at `shell.rs:175`. `source`/`.` runs via `builtin_source` → `source_in_sink` → `run_sourced_contents_in_sink` (builtins.rs:5687/5732). These are DISTINCT callers (push base-`main` at the script caller; push `source` at the `source` caller).
- Tab completion: `CompletionContext::Variable { prefix }` → `complete_variable(prefix, &shell.var_names())` (completion.rs:404/570). `var_names()` enumerates the vars table — so stored arrays complete automatically.
- `is_interactive` / `-c` vs script: `run` (shell.rs) reads a script path into `contents`; `-c` goes through a different path (`process_line`). Only the script-file path gets the base `main` frame.

---

### Task 1: `Frame` + `call_stack` replacing `function_arg0` (behavior-preserving)

**Files:** `src/shell_state.rs`, `src/executor.rs`, `src/expand.rs`.

**Goal:** swap the data model with ZERO behavior change — `FUNCNAME` and `$0` stay exactly as v151. Only function frames exist; no `BASH_SOURCE`/`BASH_LINENO` yet.

- [ ] **Step 1: Add `Frame`/`FrameKind` + `call_stack` + `function_source`.** In `src/shell_state.rs`:
```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameKind { Function, Source, Main }

#[derive(Debug, Clone)]
pub struct Frame {
    pub funcname: String,   // function name, "source", or "main"
    pub source: String,     // file where this frame's code is defined (def-source)
    pub call_line: u32,     // line in the caller where this frame was invoked (0 for base)
    pub kind: FrameKind,
}
```
Replace the `function_arg0: Vec<String>` field with `pub call_stack: Vec<Frame>` (init `Vec::new()`), and add `pub function_source: std::collections::HashMap<String, String>` (init empty). Fix the field inits in `Shell::new` (and any other constructor the compiler flags).

- [ ] **Step 2: `current_function_name` helper for `$0`.** In `impl Shell`:
```rust
/// Innermost real-function frame name (skips `source`/`main` frames). For `$0`.
pub fn current_function_name(&self) -> Option<String> {
    self.call_stack.iter().rev()
        .find(|f| f.kind == FrameKind::Function)
        .map(|f| f.funcname.clone())
}
```
Update `lookup_var` `"0"` (shell_state.rs:559) to `self.current_function_name().unwrap_or_else(|| self.shell_argv0.clone())`, and `expand.rs:220` (the `function_arg0` use) to the equivalent `current_function_name()` call. Behavior is identical (`function_arg0.last()` == innermost function frame).

- [ ] **Step 3: `sync_call_arrays` (FUNCNAME-only for now) + a small indexed-var helper.** Rename `sync_funcname` → `sync_call_arrays`. Add a helper and the FUNCNAME-only body (identical output to v151):
```rust
fn set_indexed_var(&mut self, name: &str, elements: std::collections::BTreeMap<usize, String>) {
    self.vars.insert(name.to_string(), Variable {
        value: VarValue::Indexed(elements), exported: false, readonly: false, integer: false,
    });
}
/// Rebuild FUNCNAME/BASH_SOURCE/BASH_LINENO from `call_stack`. (Task 1: FUNCNAME only.)
pub(crate) fn sync_call_arrays(&mut self) {
    if self.call_stack.is_empty() {
        self.vars.remove("FUNCNAME");
        return;
    }
    let n = self.call_stack.len();
    let funcnames: std::collections::BTreeMap<usize, String> =
        (0..n).map(|i| (i, self.call_stack[n - 1 - i].funcname.clone())).collect();
    self.set_indexed_var("FUNCNAME", funcnames);
}
```

- [ ] **Step 4: `call_function` pushes a function frame.** In `src/executor.rs`, replace `shell.function_arg0.push(name.to_string());` + `shell.sync_funcname();` (2925/2927) with:
```rust
    let frame = crate::shell_state::Frame {
        funcname: name.to_string(),
        source: shell.function_source.get(name).cloned().unwrap_or_else(|| "environment".to_string()),
        call_line: shell.current_lineno,
        kind: crate::shell_state::FrameKind::Function,
    };
    shell.call_stack.push(frame);
    shell.sync_call_arrays();
```
and replace `shell.function_arg0.pop(); shell.sync_funcname();` (2953/2954) with `shell.call_stack.pop(); shell.sync_call_arrays();`. Fix the v151 tests that referenced `function_arg0` (executor.rs:6565-6567) to use `call_stack`.

- [ ] **Step 5: Build + FULL regression (gate — behavior-preserving).**
`cargo build 2>&1 | tail -5` → clean.
`cargo test 2>&1 | grep -E "test result: FAILED|[0-9]+ failed|error\[" | head || echo NONE` → NONE. (v151's FUNCNAME unit/integration tests + `funcname_diff_check.sh` must stay green; `$0` unchanged.)
`bash tests/scripts/funcname_diff_check.sh 2>&1 | tail -2` → all pass.
`cargo clippy --all-targets 2>&1 | grep -E "^warning|^error" | head || echo CLEAN` → CLEAN.

- [ ] **Step 6: Commit.**
```bash
git add src/shell_state.rs src/executor.rs src/expand.rs
git commit -m "$(printf 'refactor: call_stack of Frames replaces function_arg0 (behavior-preserving)\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 2: def-source capture + `BASH_SOURCE`/`BASH_LINENO` (the `-c`-function cases)

**Files:** `src/shell_state.rs` (sync + define_function), `src/builtins.rs` (remove_function if there), `tests/bash_source_lineno.rs`.

- [ ] **Step 1: Capture def-source in `define_function`.** In `define_function` (shell_state.rs:1138), after (or before) inserting the body, record the def-source:
```rust
    let src = self.call_stack.last().map(|f| f.source.clone())
        .unwrap_or_else(|| "environment".to_string());
    self.function_source.insert(name.clone(), src);
```
In `remove_function` (unset -f), add `self.function_source.remove(name);`.

- [ ] **Step 2: Extend `sync_call_arrays` to emit all three arrays.** Replace the FUNCNAME-only body with:
```rust
pub(crate) fn sync_call_arrays(&mut self) {
    if self.call_stack.is_empty() {
        self.vars.remove("FUNCNAME");
        self.vars.remove("BASH_SOURCE");
        self.vars.remove("BASH_LINENO");
        return;
    }
    let n = self.call_stack.len();
    let mut funcnames = std::collections::BTreeMap::new();
    let mut sources = std::collections::BTreeMap::new();
    let mut linenos = std::collections::BTreeMap::new();
    for i in 0..n {
        let f = &self.call_stack[n - 1 - i]; // i = 0 -> top/current
        funcnames.insert(i, f.funcname.clone());
        sources.insert(i, f.source.clone());
        linenos.insert(i, f.call_line.to_string());
    }
    self.set_indexed_var("BASH_SOURCE", sources);
    self.set_indexed_var("BASH_LINENO", linenos);
    // FUNCNAME is unset when the top frame is the base `main` (Task 3 adds Main frames).
    if matches!(self.call_stack.last().map(|f| &f.kind), Some(crate::shell_state::FrameKind::Main)) {
        self.vars.remove("FUNCNAME");
    } else {
        self.set_indexed_var("FUNCNAME", funcnames);
    }
}
```
(Use the in-module `FrameKind` path correctly — this is inside `impl Shell` in `shell_state.rs`, so `FrameKind::Main`.)

- [ ] **Step 2b: Write integration tests for the `-c` function cases** in `tests/bash_source_lineno.rs`:
```rust
use std::process::Command;
fn huck(s: &str) -> String {
    let o = Command::new(env!("CARGO_BIN_EXE_huck")).args(["-c", s]).output().unwrap();
    String::from_utf8_lossy(&o.stdout).into_owned()
}
#[test] fn c_function_bash_source_is_environment() {
    // verified vs bash: f@line1 -> SRC=environment LN=1
    assert_eq!(huck("f(){ echo \"[${BASH_SOURCE[@]}] [${BASH_LINENO[@]}] [${FUNCNAME[@]}]\"; }; f"),
               "[environment] [1] [f]\n");
}
#[test] fn c_top_level_all_unset() {
    assert_eq!(huck("echo \"[${BASH_SOURCE[@]:-x}] [${BASH_LINENO[@]:-x}] [${FUNCNAME[@]:-x}]\""),
               "[x] [x] [x]\n");
}
```
VERIFY each expected value against real bash (`bash -c '…'`) before trusting it; use bash's actual output.

- [ ] **Step 3: Build + test.**
`cargo build 2>&1 | tail -3 && cargo test --test bash_source_lineno 2>&1 | tail -10` → pass.
`cargo test 2>&1 | grep -E "test result: FAILED|[0-9]+ failed|error\[" | head || echo NONE` → NONE. (FUNCNAME still correct; -c cases gain BASH_SOURCE/BASH_LINENO.)
`cargo clippy --all-targets 2>&1 | grep -E "^warning|^error" | head || echo CLEAN` → CLEAN.

- [ ] **Step 4: Commit.**
```bash
git add src/shell_state.rs src/builtins.rs tests/bash_source_lineno.rs
git commit -m "$(printf 'feat: BASH_SOURCE/BASH_LINENO arrays + per-function def-source\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 3: base `main` frame (script mode) + presence rules + FUNCNAME `main`-frame fix

**Files:** `src/shell.rs`, `tests/bash_source_lineno.rs`.

- [ ] **Step 1: Write failing script-mode tests** (`tests/bash_source_lineno.rs`):
```rust
fn huck_file(body: &str) -> String {
    let f = std::env::temp_dir().join(format!("huck_v153_{}.sh", std::process::id()));
    std::fs::write(&f, body).unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_huck")).arg(&f).output().unwrap();
    std::fs::remove_file(&f).ok();
    String::from_utf8_lossy(&o.stdout).into_owned()
}
#[test] fn script_in_function_has_main_frame() {
    // g@? f@? main : FUNCNAME=[g f main]; verify exact lines vs bash for THIS body
    let body = "g(){ echo \"[${FUNCNAME[@]}] [${BASH_LINENO[@]}]\"; }\nf(){ g; }\nf\n";
    // bash: FUNCNAME=[g f main]; BASH_LINENO=[<line of g-call in f> <line of f-call> 0]
    assert_eq!(huck_file(body), "[g f main] [2 3 0]\n");
}
#[test] fn script_top_level_bash_source_set_funcname_unset() {
    let body = "echo \"[${BASH_SOURCE[@]##*/}] [${#BASH_SOURCE[@]}] [${FUNCNAME[@]:-unset}]\"\n";
    // bash: BASH_SOURCE=[<script basename>] #1 FUNCNAME=unset
    let out = huck_file(body);
    assert!(out.starts_with("[huck_v153_") || out.contains(".sh]"), "got: {out}");
    assert!(out.contains("] [1] [unset]"), "got: {out}");
}
```
VERIFY the exact line numbers / shapes against bash with the same body before finalizing the assertions.

- [ ] **Step 2: Run — verify failure** (no `main` frame yet): `cargo build 2>&1 | tail -3 && cargo test --test bash_source_lineno script_ 2>&1 | tail -15` → FAIL. Record.

- [ ] **Step 3: Push the base `main` frame for script files.** In `src/shell.rs`, in the script-file run path (around line 175, just BEFORE `run_sourced_contents(&contents, &path, shell)`), push the base frame and sync; pop after:
```rust
    shell.call_stack.push(crate::shell_state::Frame {
        funcname: "main".to_string(),
        source: path.to_string_lossy().into_owned(),
        call_line: 0,
        kind: crate::shell_state::FrameKind::Main,
    });
    shell.sync_call_arrays();
    let result = crate::builtins::run_sourced_contents(&contents, &path, shell);
    shell.call_stack.pop();
    shell.sync_call_arrays();
    match result { /* existing arms */ }
```
(Adapt to the exact surrounding match. Do NOT push this for `-c`/interactive — only this script-file path. The `sync_call_arrays` from Task 2 already applies the FUNCNAME-unset-when-top-is-Main rule, so script top-level gets `BASH_SOURCE=[script]` with `FUNCNAME` unset, and inside a function `FUNCNAME` gains the bottom `main`.)

- [ ] **Step 4: Run — verify pass:** `cargo test --test bash_source_lineno 2>&1 | tail -12` → all pass.
`cargo test 2>&1 | grep -E "test result: FAILED|[0-9]+ failed|error\[" | head || echo NONE` → NONE.
Verify v151 didn't regress AND the script-mode FUNCNAME is now `[… main]`: `bash tests/scripts/funcname_diff_check.sh 2>&1 | tail -2` (all pass — and note this harness is `-c`-based; a script-mode FUNCNAME check is added in Task 5).
`cargo clippy --all-targets 2>&1 | grep -E "^warning|^error" | head || echo CLEAN` → CLEAN.

- [ ] **Step 5: Commit.**
```bash
git add src/shell.rs tests/bash_source_lineno.rs
git commit -m "$(printf 'feat: base main frame for scripts; FUNCNAME gains main frame in script mode\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 4: `source`/`.` frame

**Files:** `src/builtins.rs`, `tests/bash_source_lineno.rs`.

- [ ] **Step 1: Write failing sourced-file tests** (`tests/bash_source_lineno.rs`). Use a 2-file helper (write a lib + a main that sources it by path) and assert vs bash. Cover: (a) sourced-file top-level — `BASH_SOURCE=[lib main]`, `BASH_LINENO=[<source-call line> 0]`, `FUNCNAME` unset; (b) function defined in the sourced lib, called from main — `FUNCNAME=[libfn caller main]`, `BASH_SOURCE=[lib main main]`; (c) inside a running `source` (a function calls `source`) — `FUNCNAME` includes `source`. Derive exact line numbers from running the same bodies through bash first.

- [ ] **Step 2: Run — verify failure** (no source frame yet): record.

- [ ] **Step 3: Push a `source` frame in the source path.** In `run_sourced_contents_in_sink` (builtins.rs:5732) — the shared engine used by `source`/`.` — push a `source` frame around the execution. IMPORTANT: this engine is ALSO called by the script-file path (Task 3) and Task 3 pushes a `Main` frame there; so push the `source` frame in the `source`-BUILTIN caller (`source_in_sink`), NOT in the shared engine, to avoid double-framing the script path. Push BEFORE running, pop AFTER:
```rust
    // in source_in_sink, around the run_sourced_contents_in_sink call:
    shell.call_stack.push(crate::shell_state::Frame {
        funcname: "source".to_string(),
        source: path.to_string_lossy().into_owned(), // the resolved sourced file path
        call_line: shell.current_lineno,              // line of the `source` command
        kind: crate::shell_state::FrameKind::Source,
    });
    shell.sync_call_arrays();
    let outcome = run_sourced_contents_in_sink(&contents, &path, shell, sink);
    shell.call_stack.pop();
    shell.sync_call_arrays();
```
(Locate the exact `source_in_sink` body and the resolved `path`/`contents` bindings; wrap the existing run call. Confirm the script-file path in shell.rs does NOT also wrap with a source frame.)

- [ ] **Step 4: Run — verify pass + full regression.**
`cargo test --test bash_source_lineno 2>&1 | tail -15` → all pass.
`cargo test 2>&1 | grep -E "test result: FAILED|[0-9]+ failed|error\[" | head || echo NONE` → NONE.
`for f in tests/scripts/*_diff_check.sh; do bash "$f" >/dev/null 2>&1 || echo "FAIL: $f"; done; echo done` → only `done`.
`cargo clippy --all-targets 2>&1 | grep -E "^warning|^error" | head || echo CLEAN` → CLEAN.

- [ ] **Step 5: Commit.**
```bash
git add src/builtins.rs tests/bash_source_lineno.rs
git commit -m "$(printf 'feat: source/. pushes a source frame onto the call stack\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 5: tab completion + bash-diff harness + full matrix

**Files:** `tests/scripts/bash_source_lineno_diff_check.sh`, `src/completion.rs` (test only).

- [ ] **Step 1: Tab-completion test.** In `src/completion.rs` `mod tests`, add a test that with a shell whose `call_stack` has a function frame (so `FUNCNAME`/`BASH_SOURCE`/`BASH_LINENO` are set), `complete_variable("BASH_", &shell.var_names()…)` offers `BASH_SOURCE` and `BASH_LINENO`, and `complete_variable("FUNC", …)` offers `FUNCNAME`. (Mirror the existing `complete_variable_*` tests; construct the shell + push a `Frame` + `sync_call_arrays`, then collect `var_names()`.)

- [ ] **Step 2: bash-diff harness** `tests/scripts/bash_source_lineno_diff_check.sh` — mirror `funcname_diff_check.sh`'s structure with `check_c` (for `-c` fragments) and `check_file`/`check_sourced` (temp-file scripts + sourced libs by path). Cover the full matrix: `-c` top-level (all unset), `-c` in-function (`environment`/line/`f`), script top-level (`BASH_SOURCE=[script]`, FUNCNAME unset), script in-function (`g f main` + sources + call-lines + `${#…}`), sourced-file top-level, function-in-sourced-lib, inside-a-running-`source`, and the scalar/`${#…}`/`${!…}` forms. Assert byte-identical bash↔huck. (Strip absolute temp paths consistently on BOTH sides if needed — e.g. compare `${BASH_SOURCE[@]##*/}` basenames — so the only difference can't be the random tempdir path.)

- [ ] **Step 3: Run harness + full regression + clippy.**
`chmod +x tests/scripts/bash_source_lineno_diff_check.sh && cargo build 2>&1 | tail -2 && bash tests/scripts/bash_source_lineno_diff_check.sh` → all PASS. (Real divergence on a non-edge case → STOP, report BLOCKED with the diff.)
`cargo test 2>&1 | grep -E "test result: FAILED|[0-9]+ failed|error\[" | head || echo NONE` → NONE.
`for f in tests/scripts/*_diff_check.sh; do bash "$f" >/dev/null 2>&1 || echo "FAIL: $f"; done; echo done` → only `done`; report `ls tests/scripts/*_diff_check.sh | wc -l`.
`cargo clippy --all-targets 2>&1 | grep -E "^warning|^error" | head || echo CLEAN` → CLEAN.

- [ ] **Step 4: Commit.**
```bash
git add tests/scripts/bash_source_lineno_diff_check.sh src/completion.rs
git commit -m "$(printf 'test: BASH_SOURCE/BASH_LINENO bash-diff harness + tab-completion test\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Notes for the implementer
- **Task 1 is a behavior-preserving refactor** — `function_arg0`→`call_stack`, `sync_funcname`→`sync_call_arrays` (FUNCNAME-only), `$0` via `current_function_name`. The gate is v151's tests + `funcname_diff_check.sh` staying green with zero change.
- **Frame-kind, not name-matching:** the `main`/`source` distinction uses `FrameKind`, so a user function named `main`/`source` is handled correctly.
- **Don't double-frame the script path:** the script-file path (shell.rs) pushes a `Main` frame; the `source` builtin pushes a `Source` frame — push each at its own caller, not in the shared `run_sourced_contents` engine.
- **call_line = `current_lineno` captured at push** (the caller's line). For the base `main` frame it's 0.
- **VERIFY all expected test values against real bash** (`bash -c …` / `bash file`) before finalizing assertions — especially exact line numbers and array shapes.
- **Tab completion is free** once the arrays are stored in the vars table — no completion code, just the test confirming it.
- **Git safety:** stay on `v153-bash-source-lineno`; do NOT `git checkout <sha>`.
