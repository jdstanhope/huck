# Unified error-message emitter (`sh_error!`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Route every huck error diagnostic through one small emitter family so the bash-compatible prologue (`<name>: [-c: ][line N: ][cmd: ]`) is applied uniformly, killing the double-prefix, sink-bypassing `eprintln!`, and the missing `-c:` segment.

**Architecture:** Add an emitter family in huck-engine (`emit_error`/`sh_error!`, `emit_syntax_error`, `emit_cli_error`) that shares one prologue builder `error_prefix(Diag)`. Convert all ~374 literal `"huck: "` emission sites to the family. `error_prefix` becomes `pub(crate)`. The bash test suite's prefix categories are the iterative oracle; an `error_message_diff_check.sh` harness plus an `include_str!` invariant are the regression nets.

**Tech Stack:** Rust (huck-engine, huck-syntax, huck-cli), bash 5.2.21 differential harnesses.

## Global Constraints

- **Commit trailer** on every commit: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- **Tests (memory-constrained box):** run per-crate single-threaded, never `--workspace`: `( ulimit -v 2500000; cargo test -p <crate> --jobs 1 --lib -- --test-threads 1 )`. Integration tests: `( ulimit -v 2500000; cargo test -p huck-engine --jobs 1 --test <name> -- --test-threads 1 )`. Build the binary with `cargo build -p huck` (huck-cli does NOT build it). Guard bash-diff harnesses with `ulimit -v 1500000` + `timeout` per harness.
- **The prologue matrix is the spec** — `docs/superpowers/specs/2026-07-07-unified-error-emitter-design.md` §3. `-c:` appears **iff** (syntax error **and** `-c` mode); runtime errors never get `-c:`; pre-shell CLI = basename + no line.
- **The conversion transform (HYBRID — updated after the T3 route_err_to_out finding):** strip only the literal `"huck: "`; everything after it (including any `cd:`/`<builtin>:` segment) stays verbatim in the body — do NOT reflow or hoist a `cmd` segment into the `Some(..)` arg (`None` + full body is byte-identical). Then pick the variant by **whether a writer is in scope**:
  - **Writer in hand (the builtin path):** if an `out`/`err` writer descending from `run_builtin(program, args, out, err, shell)` is reachable, use `sh_error_to!(shell, err, None, "REST")`. This is the redirect-aware channel; it is MANDATORY for builtins because the bare-builtin `2>&1`/`>&2` in-memory swap (`route_err_to_out`) lives only in that writer, not in the thread-local. A converted `e!(err, "huck: REST")` → `sh_error_to!(shell, err, None, "REST")`.
  - **No writer (deep helpers / non-builtin executor paths):** use the thread-local `sh_error!(shell, None, "REST")`.
  Interactive output is unchanged (`error_prefix` returns `"huck: "` interactively); non-interactive gains the bash `<name>: line N:` prologue. Regression gate for the builtin path: `x=$(<builtin that errors> 2>&1); [ -n "$x" ]` must capture the diagnostic (matches bash).
- **No message-body changes.** Only the prologue and emission path change.
- **GPL:** never copy bash test-suite bytes into committed files; per-category `.diff` inspection stays local.

---

### Task 1: Emitter family + `Diag` + full prologue matrix + `is_command_string`

Foundation. Adds the three emitters, converts `error_prefix` to a `Diag`-driven builder implementing the complete matrix, adds the `is_command_string` flag, and repoints the ~15 existing `error_prefix` callers. Touches no literal-`"huck: "` site.

**Files:**
- Create: `crates/huck-engine/src/error_emit.rs`
- Modify: `crates/huck-engine/src/shell_state.rs` (`error_prefix` → `pub(crate) fn error_prefix(&self, Diag)`; add `is_command_string: bool` field + `Shell::new` init; update the three `error_prefix_*` unit tests)
- Modify: `crates/huck-engine/src/lib.rs` (`mod error_emit;` + re-exports `emit_error`, `emit_syntax_error`, `emit_cli_error`, and `Diag`)
- Modify: `crates/huck-cli/src/repl.rs:92` (`RunMode::Command` arm — set `is_command_string = true` before `engine.run`)
- Modify: the ~15 existing `error_prefix(...)` call sites (grep `error_prefix(` — in `builtins.rs`, `expand.rs`, `param_expansion.rs`, `shell_state.rs`) → `sh_error!` or the new `Diag` signature.

**Interfaces:**
- Produces:
  - `pub enum Diag<'a> { Runtime(Option<&'a str>), Syntax { line: u32 } }`
  - `pub(crate) fn Shell::error_prefix(&self, kind: Diag) -> String`
  - `pub fn emit_error(shell: &Shell, cmd: Option<&str>, body: std::fmt::Arguments)`
  - `pub fn emit_syntax_error(shell: &Shell, line: u32, body: std::fmt::Arguments)`
  - `pub fn emit_cli_error(prog: &str, body: std::fmt::Arguments)`
  - `#[macro_export] macro_rules! sh_error { ($shell, $cmd, $($arg)*) => { $crate::emit_error($shell, $cmd, format_args!($($arg)*)) } }`
  - `Shell.is_command_string: bool`
- Consumes: `with_err` (`err_thread_local.rs`), `Shell` state (`is_interactive`, `shell_argv0`, `current_lineno`, `BASH_SOURCE[0]`).

- [ ] **Step 1: Write failing unit tests for `error_prefix(Diag)` — the full matrix.**

In `shell_state.rs` tests, replace/extend the three `error_prefix_*` tests to the new signature and add the syntax + `-c:` cases:

```rust
#[test]
fn error_prefix_runtime_matrix() {
    let mut sh = Shell::new();
    sh.is_interactive = false; sh.shell_argv0 = "s.sh".into(); sh.current_lineno = 5;
    assert_eq!(sh.error_prefix(Diag::Runtime(None)), "s.sh: line 5: ");
    assert_eq!(sh.error_prefix(Diag::Runtime(Some("cd"))), "s.sh: line 5: cd: ");
    sh.is_interactive = true;
    assert_eq!(sh.error_prefix(Diag::Runtime(None)), "huck: ");
}
#[test]
fn error_prefix_syntax_matrix() {
    let mut sh = Shell::new();
    sh.is_interactive = false; sh.shell_argv0 = "s.sh".into();
    // script mode: no -c:
    sh.is_command_string = false;
    assert_eq!(sh.error_prefix(Diag::Syntax { line: 2 }), "s.sh: line 2: ");
    // -c mode: -c: present
    sh.is_command_string = true; sh.shell_argv0 = "bash5".into();
    assert_eq!(sh.error_prefix(Diag::Syntax { line: 1 }), "bash5: -c: line 1: ");
}
#[test]
fn emit_cli_error_is_basename_no_line() {
    // capture sink asserts "huck: boom\n"
}
```

- [ ] **Step 2: Run tests to verify they fail** (compile error / wrong signature).
Run: `( ulimit -v 2500000; cargo test -p huck-engine --jobs 1 --lib error_prefix -- --test-threads 1 )`
Expected: FAIL (signature mismatch / `Diag` undefined).

- [ ] **Step 3: Implement `Diag` + the matrix in `error_prefix`.**

```rust
pub enum Diag<'a> { Runtime(Option<&'a str>), Syntax { line: u32 } }

pub(crate) fn error_prefix(&self, kind: Diag) -> String {
    let name = if !self.is_interactive {
        self.get_indexed("BASH_SOURCE").and_then(|m| m.get(&0))
            .filter(|s| !s.is_empty()).cloned()
            .unwrap_or_else(|| self.shell_argv0.clone())
    } else { "huck".to_string() };
    let mut out = format!("{name}: ");
    match kind {
        Diag::Runtime(cmd) => {
            if !self.is_interactive && self.current_lineno > 0 {
                out.push_str(&format!("line {}: ", self.current_lineno));
            }
            if let Some(c) = cmd { out.push_str(&format!("{c}: ")); }
        }
        Diag::Syntax { line } => {
            if !self.is_interactive && self.is_command_string { out.push_str("-c: "); }
            if !self.is_interactive { out.push_str(&format!("line {line}: ")); }
        }
    }
    out
}
```
Add `is_command_string: bool` to the struct + `Shell::new` (`false`) + `Mark`/clone paths if `Shell` derives them (grep the struct's other bool fields to match).

- [ ] **Step 4: Create `error_emit.rs`** with `emit_error`, `emit_syntax_error`, `emit_cli_error`, and the `sh_error!` macro exactly as in spec §1. Wire `mod error_emit;` + re-exports in `lib.rs`.

- [ ] **Step 5: Repoint the ~15 existing `error_prefix` callers.**
Grep `error_prefix(` across `crates/`. Each current `let prefix = shell.error_prefix(None); e!(err, "{prefix}{rest}")` (or inline) becomes `sh_error!(shell, None, "{rest}")`; `error_prefix(Some("cd"))` → `sh_error!(shell, Some("cd"), "{rest}")` OR `sh_error!(shell, None, "cd: {rest}")` (equivalent). After this step `error_prefix` has no caller outside `error_emit.rs`.

- [ ] **Step 6: Set `is_command_string` in the `-c` dispatch.**
`repl.rs:92` `RunMode::Command` arm: set the flag on the engine's shell before `engine.run(&command)` (add an engine setter if needed, mirroring `shell_argv0` assignment at `engine.rs:147/181`).

- [ ] **Step 7: Run tests to verify they pass.**
Run: `( ulimit -v 2500000; cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 )`
Expected: PASS (full lib suite green; new matrix tests pass).

- [ ] **Step 8: Commit.**
```bash
git add -A && git commit -m "v269 T1: emitter family + Diag prologue matrix + is_command_string

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Syntax-error path + pre-shell CLI + the diff-check harness

Makes `ParseError`/`LexError` Display body-only, routes syntax errors through `emit_syntax_error` (killing the `huck: bash5: line 1:` double prefix, adding `-c:`), routes pre-shell CLI errors through `emit_cli_error`, and lands the full-matrix regression harness (using the already-converted `cd`/`readonly` runtime errors as the runtime exemplars).

**Files:**
- Modify: `crates/huck-syntax/src/errors.rs` (the 3 `"huck: "` occurrences → body-only; no name/line/`-c:`), `command.rs:676`/`lexer.rs:36` Display remain body-only.
- Modify: `crates/huck-cli/src/repl.rs` (`Err(e)` arms at 46/103/128/366; the `eprintln!("huck: syntax error: …")` at 229) + the parse-error print site inside `engine.run`/`process_line` (grep for where a `-c` parse error currently prints `<name>: line N:` — the double-prefix origin).
- Modify: `crates/huck-cli/src/repl.rs:47,129` (pre-shell `parse_cli`/editor-init → `emit_cli_error(basename(&args[0]), …)`).
- Create: `tests/scripts/error_message_diff_check.sh`

**Interfaces:**
- Consumes: `emit_syntax_error`, `emit_cli_error` (Task 1). The `ParseError` line accessor (AST `line: u32` / spanned tokens — confirm the exact field during implementation; huck already emits *a* line for `-c` syntax errors, so the source exists).

- [ ] **Step 1: Write the failing harness `error_message_diff_check.sh`.**
Standard `checkf`/`checkd` shape (model on `tests/scripts/subscript_lvalue_diff_check.sh`). `HUCK=target/debug/huck`. One fragment per matrix cell, comparing bash vs huck stderr byte-for-byte (normalize only the `<name>` field, which is the binary path — assert the `[-c: ][line N: ]<rest>` tail):
  - runtime: `cd /nope` in `-c`, in a script file, and via stdin → tail `line 1: cd: /nope: No such file or directory` (no `-c:`).
  - syntax `-c`: `huck -c 'if'` → tail `-c: line 1: <body>`; assert `-c:` present.
  - syntax script: same fragment in a file → tail `line 1: <body>`; assert `-c:` ABSENT.
  - custom `$0`: `huck -c 'cd /nope' myprog` → begins `myprog: `.
  - double-prefix regression: `huck -c 'if'` stderr contains exactly one `: ` prologue segment before the body (no `huck: <name>:`).
  - sink routing: `huck -c 'readonly x=1; x=2' 2>&1` — the diagnostic appears on stdout.
  - pre-shell: `huck --badoption` → `huck: <msg>` (basename, no line).
Compare each against the same fragment run through `bash`.

- [ ] **Step 2: Run harness to verify it fails.**
Run: `cargo build -p huck && ( ulimit -v 1500000; timeout 60 bash tests/scripts/error_message_diff_check.sh )`
Expected: FAIL on the syntax `-c:` cases and the double-prefix case.

- [ ] **Step 3: Make `ParseError`/`LexError` Display body-only.**
Audit `errors::parse_error_message_impl` and the 3 `errors.rs` `"huck: "` sites; remove any `huck:`/`<name>:`/`line N:`/`-c:` text so Display is pure message body.

- [ ] **Step 4: Route syntax errors through `emit_syntax_error`.**
Find the site(s) that print a parse error for the `-c`/script/stdin paths (the current `huck: bash5: line 1:` origin — grep `line` formatting near the parse-error handling in `engine.rs`/`shell.rs`/`repl.rs`). Replace with `emit_syntax_error(shell, <line from ParseError>, format_args!("{e}"))`. Fix `repl.rs:229` similarly. Remove the outer `eprintln!("huck: {e}")` wrapping (repl.rs:47 is CLI/pre-shell — Step 6, not this).

- [ ] **Step 5: Run the syntax portion.**
Run: `cargo build -p huck && ( ulimit -v 1500000; timeout 60 bash tests/scripts/error_message_diff_check.sh )`
Expected: syntax + double-prefix cells PASS.

- [ ] **Step 6: Route pre-shell CLI errors through `emit_cli_error`.**
`repl.rs:47` (`parse_cli` failure) and `repl.rs:129` (editor init) → `emit_cli_error(&basename(&args[0]), format_args!("{e}"))`. Derive basename from `args[0]` (fallback `"huck"`).

- [ ] **Step 7: Run the full harness + both suites.**
Run: `cargo build -p huck && ( ulimit -v 1500000; timeout 60 bash tests/scripts/error_message_diff_check.sh )` → all cells PASS.
Run: `( ulimit -v 2500000; cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1 )` and same for `huck-engine` → green.

- [ ] **Step 8: Commit.**
```bash
git add -A && git commit -m "v269 T2: syntax-error emitter (-c: segment, no double prefix) + pre-shell CLI + diff harness

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Convert `builtins.rs` (~201 sites)

Pure mechanical conversion of the largest file per the Global-Constraints transform rule.

**Files:**
- Modify: `crates/huck-engine/src/builtins.rs`

- [ ] **Step 1: Convert every `"huck: "` emission site.**
For each `with_err(|err| e!(err, "huck: REST"))` (and any `e!(err, "…huck: …")` variant), rewrite to `sh_error!(shell, None, "REST")`. The `shell`/`self` handle is already in scope at builtin sites. Leave `warning:`-class lines that bash does NOT prologue as-is only if verified; default is to convert (bash prologues its warnings).

- [ ] **Step 2: Fix any exact-match test breakage.**
Run the lib suite (Step 3). For each failing test that pinned a literal `"huck: X"`, change the expectation to assert the message **body**/`: X` suffix (matching the established `contains(": line N: …")` style), not the `<name>` prefix. Do NOT weaken assertions beyond removing the `<name>` pin.

- [ ] **Step 3: Run the lib suite.**
Run: `( ulimit -v 2500000; cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 )`
Expected: PASS.

- [ ] **Step 4: Spot-check bash parity.**
Run: `cargo build -p huck && ( ulimit -v 1500000; timeout 60 bash tests/scripts/error_message_diff_check.sh )`
Expected: PASS (unchanged — cd/readonly still covered).

- [ ] **Step 5: Commit.**
```bash
git add -A && git commit -m "v269 T3: convert builtins.rs error sites to sh_error!

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Convert `executor.rs` (~85 sites) + `cwd_scope.rs` raw `eprintln!`

**Files:**
- Modify: `crates/huck-engine/src/executor.rs`, `crates/huck-engine/src/cwd_scope.rs`

- [ ] **Step 1: Convert `executor.rs` sites** — nearly all are local redirect-aware `{ let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: REST") }`. KEEP the `err_writer` local writer and emit via `sh_error_to!(shell, &mut *err, None, "REST")` (NOT thread-local `sh_error!` — that misroutes under inner `2>&1`). Use `sh_error!` only where a site genuinely has NO local `err_sink`/`sink`/writer.
- [ ] **Step 2: Convert `cwd_scope.rs:29`** `eprintln!("huck: cwd: {}: {}", …)` → `sh_error!(shell, None, "cwd: {}: {}", …)` (routes through the sink — a `2>&1` correctness fix). Confirm a `&Shell` is reachable here; if not, thread one in (flag if a signature change is required).
- [ ] **Step 3: Fix any pinned-prefix test breakage** (as Task 3 Step 2).
- [ ] **Step 4: Run the lib suite** (`huck-engine`, per Global Constraints) → PASS.
- [ ] **Step 5: Run the harness** → PASS.
- [ ] **Step 6: Commit** (`v269 T4: convert executor.rs + cwd_scope.rs error sites`).

---

### Task 5: Convert `expand.rs` + `param_expansion.rs` + `completion_builtins.rs` (~43 sites)

**Files:**
- Modify: `crates/huck-engine/src/expand.rs`, `crates/huck-engine/src/param_expansion.rs`, `crates/huck-engine/src/completion_builtins.rs`

- [ ] **Step 1: Convert all sites** in the three files per the transform rule.
- [ ] **Step 2: Fix any pinned-prefix test breakage.**
- [ ] **Step 3: Run the lib suite** → PASS.
- [ ] **Step 4: Run the harness** → PASS.
- [ ] **Step 5: Commit** (`v269 T5: convert expand/param_expansion/completion_builtins error sites`).

---

### Task 6: Convert the tail files + error-value constructors (~33 sites)

**Files:**
- Modify: `crates/huck-engine/src/shell_state.rs` (incl. the "callers translate" value-constructors at ~173/189/192 → return **body only**, and the translating callers emit with `sh_error!`), `restricted.rs`, `shell.rs`, `stdin_pipe.rs`, `history.rs`, `engine.rs`

- [ ] **Step 1: Convert the `with_err`/`e!` emission sites** in each file per the transform rule.
- [ ] **Step 2: Convert the error-value constructors (spec §6).** The `shell_state.rs` methods returning `"huck: {cmd}: …"` strings return the body without `"huck: "`; update each translating caller to `sh_error!`.
- [ ] **Step 3: Fix any pinned-prefix test breakage.**
- [ ] **Step 4: Run the lib suite** → PASS.
- [ ] **Step 5: Run the harness** → PASS.
- [ ] **Step 6: Commit** (`v269 T6: convert shell_state value-constructors + tail files`).

---

### Task 7: Enforcement invariant + bash-suite prologue verification (final gate)

**Files:**
- Modify: a huck-engine test module (new `error_emit.rs` test or `shell_state.rs` tests) — the `include_str!` invariant.

- [ ] **Step 1: Write the invariant test.**
Model on v268's `lexer_has_no_production_parser_dependency`: `include_str!` each emission source (`builtins.rs`, `executor.rs`, `expand.rs`, `param_expansion.rs`, `completion_builtins.rs`, `shell_state.rs`, `restricted.rs`, `shell.rs`, `stdin_pipe.rs`, `history.rs`, `engine.rs`, `cwd_scope.rs`), strip `#[cfg(test)]` modules, and assert zero `"huck: "` occurrences EXCEPT inside `error_prefix`/`error_emit.rs` (the emitter family). Assert the same for `crates/huck-cli/src/repl.rs` outside `emit_cli_error` usage.

- [ ] **Step 2: Run it — verify it passes** (all sites converted by T3–T6).
Run: `( ulimit -v 2500000; cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 )`
Expected: PASS. If it fails, it names the residual literal site → convert it, re-run.

- [ ] **Step 3: Iterative bash-suite prologue verification.**
For each of `parser`, `execscript`, `type`, `dirstack`, `alias`, `printf`, `errors`, `comsub`:
`BASH_SOURCE_DIR=/tmp/bash-5.2.21 HUCK_BASH_TEST_CATEGORY=<cat> ( ulimit -v 2500000; timeout 60 bash tests/bash-test-suite/runner.sh > /tmp/cat.md )` and read the per-category `.diff` in the scratch dir. Confirm no diff line is a *prologue* mismatch (a `huck:` vs `<name>: [-c: ]line N:` difference). Body-only diffs (wording, source-line echo, `-o`) are expected and out of scope. Fix any prologue residual via the emitter/matrix and re-run.

- [ ] **Step 4: Full regression pass.**
Run both suites (`huck-syntax`, `huck-engine`) per Global Constraints → green.
Run the harness → PASS.

- [ ] **Step 5: Commit** (`v269 T7: emitter invariant test + bash-suite prologue verification`).

---

## Post-implementation (controller, after final review)

- Whole-branch opus review of the full diff (per subagent-driven-development).
- Merge `--no-ff`, push, delete branch (with `AskUserQuestion` confirmation).
- `docs/bash-divergences.md`: the L-56/L-26 prefix entries — trim any now-resolved prologue divergences; add a `[deferred]` note for the residual per-category *body* blockers (message wording, source-line echo) that still gate those categories.
- Memory: record v269 in `project_huck_iterations.md` + `MEMORY.md`; update `huck-error-prologue-staged.md` (the staged rollout is now COMPLETE — the emitter family + matrix shipped).
