# POSIX-mode Non-interactive-Exit-on-Error (Cluster A) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** In POSIX mode, make a non-interactive huck shell *exit* on the seven error classes where bash 5.2.21 does, by reusing huck's existing pending-fatal-status mechanism.

**Architecture:** Rename `pending_fatal_pe_error` → a general `pending_fatal_status: Option<i32>` (its unwinding + two top-level drains are already bash-correct), add a gated `posix_fatal(status)` setter (no-op unless `posix && !interactive`), and call it at each of the seven detection sites. Case #1 (special-builtin usage/assignment errors) uses a `builtin_usage_error: Option<i32>` signal set by the builtins and consumed by the executor only for *bare* special-builtin invocations.

**Tech Stack:** Rust (workspace crate `huck-engine`, plus the rename touches `huck-cli`); bash 5.2.21 as the oracle; `tests/scripts/*_diff_check.sh` harnesses.

## Global Constraints

- **Commit trailer (verbatim, every commit):** `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- **Tests:** `cargo test --workspace` (plain `cargo test` skips crates). Must stay green (~3714+).
- **bash oracle:** system `bash` is 5.2.21. Never vendor bash source/output into committed files.
- **Gating:** every new exit fires only via `shell.posix_fatal(...)`, which is a no-op unless `posix && !interactive`. Default-mode and interactive behavior must be byte-for-byte unchanged.
- **No regression:** `cprint`/`herestr`/`func` categories stay PASS; the existing `${x?}` pe-fatal behavior (status 1, both modes) is unchanged.
- **No category flip** is expected — acceptance is the diff harness + no regression.
- **Build note:** `cargo build --release --bin huck` is slow (~8-10 min); use `cargo build --bin huck` (debug) for harness runs.

### Verified bash 5.2.21 status table (the source of truth for every expected value)

Posix, non-interactive. "exit N" = shell exits with status N (no `AFTER`); "continue" = prints `AFTER`, shell lives.

| trigger | bash |
| --- | --- |
| `set -o nosuch` / `set -z` (bad option) | exit 2 |
| `unset -z` (bad option) | exit 2 |
| `unset` a readonly var | continue |
| `export -z` / `readonly -z` / `trap -z` / `exec -z` / `. -z` (bad option) | exit 2 |
| `export AA[4]=1` / `readonly AA[4]=1` (bad assignment) | exit 1 |
| `export "AA[4]"` (bad name, no `=`) | continue |
| `shift -z` / `shift 99` | continue |
| `return 2` outside a function | exit 2 |
| `f(){ return 2; }; f` (legit) | continue |
| `break` / `continue` outside a loop | continue |
| `trap x NOSUCHSIG` (bad signal) | continue |
| `exit abc` (non-numeric) | exit 2 |
| `command set -o bad` / `builtin set -o bad` / `command export AA[4]=1` | continue |
| `readonly x=1; x=2` (assign, no command) | exit 127 |
| `readonly x=1; x=2 export y` (assign before special) | exit 127 |
| `readonly i=1; for i in a b; do …; done` (readonly for var) | exit 127 |
| `. /no/such/file` (source not found) | exit 1 |
| `eval(){ :; }` (fn name == special builtin) | exit 2 |
| `echo $(( 1 + ))` (arith syntax error) | exit 127 |

---

### Task 1: Mechanism foundation — rename, `posix_fatal`, `builtin_usage_error`

**Files:**
- Modify (rename, all crates): `crates/huck-engine/src/{shell_state.rs,expand.rs,executor.rs,shell.rs,builtins.rs,param_expansion.rs,completion_spec.rs}`, `crates/huck-cli/src/repl.rs`
- Modify: `crates/huck-engine/src/shell_state.rs` (add setter + `builtin_usage_error` field + init)
- Test: `crates/huck-engine/src/shell_state.rs` (test module)

**Interfaces:**
- Produces: `Shell.pending_fatal_status: Option<i32>` (was `pending_fatal_pe_error`); `Shell::take_pending_fatal_status()`; `Shell::posix_fatal(status: i32)`; `Shell.builtin_usage_error: Option<i32>`.

- [ ] **Step 1: Rename the field and its accessor across the workspace**

Run (mechanical, whole-word rename):
```bash
cd /home/john/projects/huck
grep -rl 'pending_fatal_pe_error' crates/ | xargs sed -i 's/pending_fatal_pe_error/pending_fatal_status/g'
grep -rl 'take_pending_fatal_pe_error' crates/ | xargs sed -i 's/take_pending_fatal_pe_error/take_pending_fatal_status/g'
```
Then verify zero stragglers: `grep -rn 'pending_fatal_pe_error\|take_pending_fatal_pe_error' crates/` → no output.

- [ ] **Step 2: Confirm the rename compiles**

Run: `cargo build -p huck-engine 2>&1 | tail -3`
Expected: `Finished`. (No behavior changed; this is a pure rename.)

- [ ] **Step 3: Add the `builtin_usage_error` field + `posix_fatal` setter**

In `crates/huck-engine/src/shell_state.rs`, add the field to the `Shell` struct near `pending_fatal_status` (its decl is ~line 480):
```rust
    /// Set by a POSIX special builtin when it hits a usage / bad-option /
    /// bad-assignment error (NOT a runtime error). Consumed by the executor's
    /// bare special-builtin dispatch to fire `posix_fatal`. Cleared per command.
    pub builtin_usage_error: Option<i32>,
```
Initialize it in `Shell::new` next to `pending_fatal_status: None,` (~line 765):
```rust
            builtin_usage_error: None,
```
Add the setter method on `impl Shell` (near `take_pending_fatal_status`, ~line 2433):
```rust
    /// Mark a POSIX-mode fatal error: a non-interactive posix shell exits with
    /// `status`. No-op in default mode or interactively (matches bash).
    pub fn posix_fatal(&mut self, status: i32) {
        if self.shell_options.posix && !self.is_interactive {
            self.pending_fatal_status = Some(status);
        }
    }
```

- [ ] **Step 4: Write the failing test (four-quadrant gating)**

In the `#[cfg(test)] mod tests` of `crates/huck-engine/src/shell_state.rs`:
```rust
    #[test]
    fn posix_fatal_is_gated_on_posix_and_noninteractive() {
        let mut sh = Shell::new();
        sh.is_interactive = false;
        // default mode → no-op
        sh.posix_fatal(127);
        assert_eq!(sh.pending_fatal_status, None);
        // posix + non-interactive → sets
        sh.shell_options.posix = true;
        sh.posix_fatal(127);
        assert_eq!(sh.pending_fatal_status, Some(127));
        // posix + interactive → no-op (clear first)
        sh.pending_fatal_status = None;
        sh.is_interactive = true;
        sh.posix_fatal(2);
        assert_eq!(sh.pending_fatal_status, None);
    }
```

- [ ] **Step 5: Run the test**

Run: `cargo test -p huck-engine posix_fatal_is_gated_on_posix_and_noninteractive`
Expected: PASS.

- [ ] **Step 6: Full workspace green**

Run: `cargo test --workspace 2>&1 | grep -E "test result|error\[" | tail`
Expected: 0 failed (the rename touched many files; confirm nothing broke).

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "v226 task 1: rename pending_fatal_status + posix_fatal setter + builtin_usage_error field

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Cases #5, #6, #4 — source-not-found, fn-name-clash, readonly for-var

**Files:**
- Modify: `crates/huck-engine/src/builtins.rs:6066` (source not found, #5)
- Modify: `crates/huck-engine/src/executor.rs:812` (FunctionDef, #6) and `~1843` (run_for_inner loop-var, #4)
- Test: `crates/huck-engine/src/executor.rs` (test module)

**Interfaces:**
- Consumes: `Shell::posix_fatal`, `builtins::is_special_builtin`.

- [ ] **Step 1: Write the failing tests**

In the test module of `crates/huck-engine/src/executor.rs` (it has `exec_script`):
```rust
    #[test]
    fn posix_source_not_found_is_fatal() {
        let mut shell = Shell::new();
        exec_script("set -o posix\n. /no/such/huck_file_xyz\n", &mut shell);
        assert_eq!(shell.pending_fatal_status, Some(1));
    }
    #[test]
    fn default_source_not_found_is_not_fatal() {
        let mut shell = Shell::new();
        exec_script(". /no/such/huck_file_xyz\n", &mut shell);
        assert_eq!(shell.pending_fatal_status, None);
    }
    #[test]
    fn posix_function_named_special_builtin_is_fatal() {
        let mut shell = Shell::new();
        exec_script("set -o posix\neval() { :; }\n", &mut shell);
        assert_eq!(shell.pending_fatal_status, Some(2));
        assert!(!shell.functions.contains_key("eval"), "function not defined");
    }
    #[test]
    fn default_function_named_special_builtin_is_allowed() {
        let mut shell = Shell::new();
        exec_script("eval() { :; }\n", &mut shell);
        assert_eq!(shell.pending_fatal_status, None);
        assert!(shell.functions.contains_key("eval"));
    }
    #[test]
    fn posix_readonly_for_var_is_fatal() {
        let mut shell = Shell::new();
        exec_script("set -o posix\nreadonly i=1\nfor i in a b; do :; done\n", &mut shell);
        assert_eq!(shell.pending_fatal_status, Some(127));
    }
    #[test]
    fn default_readonly_for_var_is_not_fatal() {
        let mut shell = Shell::new();
        exec_script("readonly i=1\nfor i in a b; do :; done\n", &mut shell);
        assert_eq!(shell.pending_fatal_status, None);
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p huck-engine posix_source_not_found_is_fatal posix_function_named_special posix_readonly_for_var`
Expected: the posix ones FAIL (flag stays None today).

- [ ] **Step 3: Case #5 — source not found**

In `crates/huck-engine/src/builtins.rs`, the not-found branch at ~line 6066 currently:
```rust
            { let mut err = crate::executor::err_writer(err_sink, sink); e!(&mut *err, "huck: .: {filename}: file not found"); }
```
Add `shell.posix_fatal(1);` immediately after that error line (same block, before the existing `return`/status). Confirm `shell` is in scope (it is — `source_in_sink` takes `shell`).

- [ ] **Step 4: Case #6 — function name == special builtin**

In `crates/huck-engine/src/executor.rs`, replace the `Command::FunctionDef` arm (lines 812-814):
```rust
        Command::FunctionDef { name, body } => {
            shell.define_function(name.clone(), body.clone());
            ExecOutcome::Continue(0)
        }
```
with:
```rust
        Command::FunctionDef { name, body } => {
            // POSIX: a function may not be named after a special builtin; a
            // non-interactive posix shell errors and exits (default mode allows it).
            if shell.shell_options.posix && builtins::is_special_builtin(name) {
                { let mut err = err_writer(err_sink, sink);
                  e!(&mut *err, "{}{name}: is a special builtin", shell.error_prefix(None)); }
                shell.posix_fatal(2);
                return ExecOutcome::Continue(2);
            }
            shell.define_function(name.clone(), body.clone());
            ExecOutcome::Continue(0)
        }
```
(`shell.error_prefix(None)` yields huck's `name:`/`huck:` prologue; the harness compares stdout+exit, so the exact prologue text is not asserted.)

- [ ] **Step 5: Case #4 — readonly `for` var**

In `crates/huck-engine/src/executor.rs`, the loop-var assignment in `run_for_inner` (~line 1843):
```rust
        if shell.try_set(&clause.var, value).is_err() {
            { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {}: readonly variable", clause.var); }
            return ExecOutcome::Continue(1);
        }
```
Insert `shell.posix_fatal(127);` between the error line and the `return`:
```rust
        if shell.try_set(&clause.var, value).is_err() {
            { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {}: readonly variable", clause.var); }
            shell.posix_fatal(127);
            return ExecOutcome::Continue(1);
        }
```

- [ ] **Step 6: Run the tests**

Run: `cargo test -p huck-engine posix_source_not_found default_source_not_found posix_function_named default_function_named posix_readonly_for_var default_readonly_for_var`
Expected: all PASS.

- [ ] **Step 7: Full crate green + commit**

Run: `cargo test -p huck-engine 2>&1 | grep "test result" | tail`
Expected: 0 failed.
```bash
git add crates/huck-engine/src/builtins.rs crates/huck-engine/src/executor.rs
git commit -m "v226 task 2: posix-fatal source-not-found, fn-name-clash, readonly for-var

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Cases #2, #3 — assignment errors (no command / before special builtin)

**Files:**
- Modify: `crates/huck-engine/src/executor.rs` — `run_assignment_list` (~3886/3891, #2) and the `apply_inline_assignments` `Err` arm in `run_exec_single` (~4168, #3)
- Test: `crates/huck-engine/src/executor.rs` (test module)

**Interfaces:**
- Consumes: `Shell::posix_fatal`, `builtins::is_special_builtin`, `resolved.program`.

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn posix_assignment_no_command_is_fatal() {
        let mut shell = Shell::new();
        exec_script("set -o posix\nreadonly x=1\nx=2\n", &mut shell);
        assert_eq!(shell.pending_fatal_status, Some(127));
    }
    #[test]
    fn posix_assignment_before_special_is_fatal() {
        let mut shell = Shell::new();
        exec_script("set -o posix\nreadonly x=1\nx=2 export y\n", &mut shell);
        assert_eq!(shell.pending_fatal_status, Some(127));
    }
    #[test]
    fn posix_assignment_before_regular_is_not_fatal() {
        // before a REGULAR command → abort-continue (deferred), NOT a shell exit.
        let mut shell = Shell::new();
        exec_script("set -o posix\nreadonly x=1\nx=2 true\n", &mut shell);
        assert_eq!(shell.pending_fatal_status, None);
    }
    #[test]
    fn default_assignment_no_command_is_not_fatal() {
        let mut shell = Shell::new();
        exec_script("readonly x=1\nx=2\n", &mut shell);
        assert_eq!(shell.pending_fatal_status, None);
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p huck-engine posix_assignment_no_command posix_assignment_before_special`
Expected: FAIL (flag None today).

- [ ] **Step 3: Case #2 — assignment error, no command (`run_assignment_list`)**

In `crates/huck-engine/src/executor.rs`, `run_assignment_list` has two error `break` sites (~3886 readonly, ~3891 apply error):
```rust
        if !shell.is_nameref(name) && shell.is_readonly(name) {
            { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {name}: readonly variable"); }
            st = 1;
            break;
        }
        if apply_one_assignment(a, shell, &mut *err_writer(err_sink, sink)).is_err() {
            st = 1;
            break;
        }
```
Add `shell.posix_fatal(127);` immediately before each `st = 1;` line (both error branches — these are genuine assignment errors, distinct from a non-zero RHS command-substitution status which must NOT exit):
```rust
        if !shell.is_nameref(name) && shell.is_readonly(name) {
            { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: {name}: readonly variable"); }
            shell.posix_fatal(127);
            st = 1;
            break;
        }
        if apply_one_assignment(a, shell, &mut *err_writer(err_sink, sink)).is_err() {
            shell.posix_fatal(127);
            st = 1;
            break;
        }
```

- [ ] **Step 4: Case #3 — assignment error before a special builtin**

In `run_exec_single`, the `apply_inline_assignments` `Err` arm (~line 4168):
```rust
        Err(s) => {
            restore_inline_assignments(s, shell);
            drain_procsubs(shell, procsub_base);
            return ExecOutcome::Continue(1);
        }
```
Add a posix_fatal gated on the program being special (at this point `resolved.program` is known — `resolved` is computed earlier at ~4051):
```rust
        Err(s) => {
            restore_inline_assignments(s, shell);
            if builtins::is_special_builtin(&resolved.program) {
                shell.posix_fatal(127);
            }
            drain_procsubs(shell, procsub_base);
            return ExecOutcome::Continue(1);
        }
```
(Regular/external command → no `posix_fatal` → the deferred abort-continue case, unchanged.)

- [ ] **Step 5: Run the tests**

Run: `cargo test -p huck-engine posix_assignment default_assignment`
Expected: all four PASS.

- [ ] **Step 6: Full crate green + commit**

Run: `cargo test -p huck-engine 2>&1 | grep "test result" | tail`
```bash
git add crates/huck-engine/src/executor.rs
git commit -m "v226 task 3: posix-fatal assignment errors (no command / before special builtin)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Case #7 — arith syntax error

**Files:**
- Modify: `crates/huck-engine/src/expand.rs:~1124` (the `WordPart::Arith` `Err` arm — v215's non-fatal site)
- Test: `crates/huck-engine/src/expand.rs` (test module, near `expand_arith_part_division_by_zero_is_nonfatal`)

**Interfaces:**
- Consumes: `Shell::posix_fatal`.

- [ ] **Step 1: Write the failing test**

In the test module of `crates/huck-engine/src/expand.rs`:
```rust
    #[test]
    fn expand_arith_error_is_posix_fatal() {
        let mut shell = Shell::new();
        shell.shell_options.posix = true;
        shell.is_interactive = false;
        let word = Word(vec![arith_part("1 + ")]);
        let _ = expand(&word, &mut shell);
        assert_eq!(shell.pending_fatal_status, Some(127));
    }
```
(The existing `expand_arith_part_division_by_zero_is_nonfatal` already guards the default-mode no-op — keep it; it asserts `pending_fatal_status == None` in default mode.)

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p huck-engine expand_arith_error_is_posix_fatal`
Expected: FAIL (flag stays None).

- [ ] **Step 3: Add the posix-fatal at the arith Err arm**

In `crates/huck-engine/src/expand.rs`, the `WordPart::Arith` `Err(e)` arm (~line 1118-1128):
```rust
                Err(e) => {
                    // Print the error but DO NOT set pending_fatal_status —
                    // bash script-file mode prints and continues. ...
                    let prefix = shell.error_prefix(None);
                    with_err(|err| e!(err, "{prefix}{}", crate::arith::render_error_body(&src, &e)));
                    *has_emitted = true;
                }
```
Add `shell.posix_fatal(127);` after the `with_err(...)` line (still inside the `Err` arm). Update the comment to note default-mode stays non-fatal (v215), posix exits:
```rust
                Err(e) => {
                    // Print the error. Default mode: NON-fatal (v215) — prints and
                    // continues. POSIX non-interactive: the shell exits (127).
                    let prefix = shell.error_prefix(None);
                    with_err(|err| e!(err, "{prefix}{}", crate::arith::render_error_body(&src, &e)));
                    shell.posix_fatal(127);
                    *has_emitted = true;
                }
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p huck-engine expand_arith_error_is_posix_fatal expand_arith_part_division_by_zero_is_nonfatal`
Expected: both PASS (posix fatal; default still non-fatal).

- [ ] **Step 5: Full crate green + commit**

Run: `cargo test -p huck-engine 2>&1 | grep "test result" | tail`
```bash
git add crates/huck-engine/src/expand.rs
git commit -m "v226 task 4: arith syntax error is posix-fatal (default mode stays non-fatal per v215)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Case #1 — special-builtin usage / assignment errors

**Files:**
- Modify: `crates/huck-engine/src/builtins.rs` — set the `builtin_usage_error` signal at the fatal error sites of the special builtins (`set`, `unset`, `export`, `readonly`, `return`, `trap`, `exec`, `exit`, `.`/`source`)
- Modify: `crates/huck-engine/src/executor.rs` — clear the signal at the top of `run_exec_single`; consume it after a *bare* special-builtin dispatch (~after line 4341, the post-dispatch point, gated on `command_prefix.is_empty()`)
- Test: `crates/huck-engine/src/executor.rs` (test module)

**Interfaces:**
- Consumes: `Shell.builtin_usage_error`, `Shell::posix_fatal`, `builtins::is_special_builtin`, `command_prefix` (local in `run_exec_single`).

**The fatal set (from the verified table — mark ONLY these; leave runtime errors unmarked):**
- `set` bad option/flag → `builtin_usage_error = Some(2)`
- `unset` bad option → `Some(2)` (NOT "unset a readonly var" — that's runtime, stays unmarked)
- `export` / `readonly` bad option → `Some(2)`; invalid-identifier **assignment** (`AA[4]=1`) → `Some(1)` (NOT a bad name without `=`, which stays continue)
- `return` used outside a function/sourced script → `Some(2)` (NOT a legitimate `return N`)
- `trap` bad option → `Some(2)` (NOT a bad signal name)
- `exec` bad option → `Some(2)`
- `exit` non-numeric argument → `Some(2)`
- `.`/`source` bad option → `Some(2)` (the not-found case is Task 2 #5)
- **Unmarked entirely:** `shift` (all errors), `break`, `continue`, `eval`, `:` — bash continues on their errors.

- [ ] **Step 1: Write the failing tests (executor test module)**

```rust
    fn posix_run(src: &str) -> Option<i32> {
        let mut shell = Shell::new();
        exec_script(&format!("set -o posix\n{src}\n"), &mut shell);
        shell.pending_fatal_status
    }
    #[test]
    fn posix_special_builtin_usage_errors_exit() {
        assert_eq!(posix_run("set -o nosuchopt"), Some(2), "set bad option");
        assert_eq!(posix_run("unset -z"), Some(2), "unset bad option");
        assert_eq!(posix_run("export -z"), Some(2), "export bad option");
        assert_eq!(posix_run("export AA[4]=1"), Some(1), "export bad assignment");
        assert_eq!(posix_run("readonly AA[4]=1"), Some(1), "readonly bad assignment");
        assert_eq!(posix_run("return 2"), Some(2), "return outside function");
        assert_eq!(posix_run("exec -z"), Some(2), "exec bad option");
    }
    #[test]
    fn posix_special_builtin_runtime_errors_do_not_exit() {
        assert_eq!(posix_run("shift 99"), None, "shift out of range");
        assert_eq!(posix_run("shift -z"), None, "shift bad option");
        assert_eq!(posix_run("break"), None, "break outside loop");
        assert_eq!(posix_run("unset RO; readonly RO=1; unset RO"), None, "unset readonly var");
        assert_eq!(posix_run("eval false"), None, "eval propagates child status");
        assert_eq!(posix_run("f(){ return 2; }; f"), None, "legit return 2");
        assert_eq!(posix_run("trap x NOSUCHSIG"), None, "trap bad signal");
        assert_eq!(posix_run("export \"AA[4]\""), None, "export bad name no =");
    }
    #[test]
    fn posix_command_builtin_wrappers_strip_fatal() {
        assert_eq!(posix_run("command set -o bad"), None, "command strips");
        assert_eq!(posix_run("builtin set -o bad"), None, "builtin strips");
        assert_eq!(posix_run("command export AA[4]=1"), None, "command strips assignment");
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p huck-engine posix_special_builtin_usage_errors_exit`
Expected: FAIL (flag None today).

- [ ] **Step 3: Executor — clear + consume the signal**

In `crates/huck-engine/src/executor.rs`, near the top of `run_exec_single` (right after the function's opening, before any dispatch — a safe spot is immediately before the empty-program check at ~line 3990, but anywhere before dispatch works), clear the signal so it cannot leak from a prior command:
```rust
    shell.builtin_usage_error = None;
```
Then, after the dispatch result is computed and before the inline-scope finalize at the main post-dispatch path (the `let outcome = …;` block ends ~line 4340; insert right after `outcome` is bound and before `finalize_inline_scope`), consume it ONLY for a bare special builtin:
```rust
    // POSIX: a bare special builtin that hit a usage/assignment error exits a
    // non-interactive posix shell. `command`/`builtin` wrappers (command_prefix
    // non-empty, or program == "builtin") do NOT — they leave the signal unconsumed.
    if command_prefix.is_empty() && builtins::is_special_builtin(&resolved.program) {
        if let Some(st) = shell.builtin_usage_error.take() {
            shell.posix_fatal(st);
        }
    }
```
Confirm placement: it must run on the normal dispatch path (after `is_builtin`/control-builtin dispatch produced `outcome`), with `command_prefix` and `resolved` in scope. Do NOT place it on the `exec` early-return path (exec is handled separately and exits via its own mechanism).

- [ ] **Step 4: Builtins — set the signal at each fatal site**

For each special builtin below, set `shell.builtin_usage_error = Some(<status>);` at the named error site (just before it returns that status). Use the exact sites; do NOT mark runtime-error returns. After editing, verify against `bash --posix -c '<case>; echo AFTER'` that each marked case exits and each unmarked one continues.

- `builtin_set`: the invalid-option / invalid `-o` name path(s) that return 2 → `Some(2)`.
- `unset`: the invalid-option path that returns 2 → `Some(2)`. (Leave the "cannot unset readonly" path unmarked.)
- `export` / `readonly` (declaration builtins, `run_declaration_builtin` / the export/readonly arms): the invalid-option path → `Some(2)`; the invalid-identifier *assignment* path (the one hit by `AA[4]=1`, returns 1) → `Some(1)`. Leave the bad-name-without-`=` path unmarked.
- `return` builtin: the "can only return from a function or sourced script" error path → `Some(2)`.
- `trap`: the invalid-option path → `Some(2)`. Leave the bad-signal-name path unmarked.
- `exec` (`run_exec_builtin` flag parse): the bad-flag path that returns 2 → `Some(2)`.
- `exit`: the non-numeric-argument error path → `Some(2)`.
- `.`/`source` (`source_in_sink`/`builtin_source`): the invalid-option path → `Some(2)`.

If a builtin funnels several usage errors through one return-2 site, marking that one site covers them. Each builtin has `&mut Shell` in scope at these sites (they already take `shell`); if a particular error site only has `&mut dyn Write` and not `shell`, thread `shell` is unnecessary — instead set the signal at the nearest enclosing point that has `shell`, or (preferred) confirm the builtin's signature carries `shell` (the special builtins above all do via `run_builtin`/`run_declaration_builtin`).

- [ ] **Step 5: Run the case-#1 tests**

Run: `cargo test -p huck-engine posix_special_builtin_usage_errors_exit posix_special_builtin_runtime_errors_do_not_exit posix_command_builtin_wrappers_strip_fatal`
Expected: all PASS. If a runtime case wrongly exits, you marked a runtime site — remove that mark. If a usage case doesn't exit, you missed its site.

- [ ] **Step 6: Full workspace green + commit**

Run: `cargo test --workspace 2>&1 | grep -E "test result|error\[" | tail`
Expected: 0 failed.
```bash
git add crates/huck-engine/src/builtins.rs crates/huck-engine/src/executor.rs
git commit -m "v226 task 5: special-builtin usage/assignment errors are posix-fatal (case #1)

builtin_usage_error signal set at each special builtin's usage/assignment error
site; executor consumes it only for bare special-builtin invocations (command/
builtin wrappers strip it). Runtime errors (shift, break, trap bad-signal) unmarked.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: Diff harness + verification

**Files:**
- Create: `tests/scripts/posix_exit_on_error_diff_check.sh`

**Interfaces:**
- Consumes: the `huck` binary (`target/debug/huck`), `--posix`.

- [ ] **Step 1: Write the harness (compares STDOUT + exit code only — NOT stderr)**

Create `tests/scripts/posix_exit_on_error_diff_check.sh`:
```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v226: POSIX non-interactive exit-on-error
# (Cluster A). Compares STDOUT + exit code only (huck's error-message prologue
# differs from bash's `script: line N:` — a separate, deferred divergence). Each
# fragment ends in `echo AFTER`; an exit suppresses AFTER and yields bash's status.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
# $1 label, $2 fragment. Runs under bash --posix and huck --posix; compares
# (stdout, exit-code), stderr discarded.
check_posix() {
    local label="$1" frag="$2" bo ho br hr
    bo=$(bash --posix -c "$frag" 2>/dev/null); br=$?
    ho=$("$HUCK_BIN" --posix -c "$frag" 2>/dev/null); hr=$?
    if [[ "$bo" == "$ho" && "$br" == "$hr" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s  bash=(%q,%s) huck=(%q,%s)\n' "$label" "$bo" "$br" "$ho" "$hr"; FAIL=$((FAIL+1)); fi
}
# Default mode (no --posix): must continue, print AFTER, match bash.
check_default() {
    local label="$1" frag="$2" bo ho br hr
    bo=$(bash -c "$frag" 2>/dev/null); br=$?
    ho=$("$HUCK_BIN" -c "$frag" 2>/dev/null); hr=$?
    if [[ "$bo" == "$ho" && "$br" == "$hr" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s  bash=(%q,%s) huck=(%q,%s)\n' "$label" "$bo" "$br" "$ho" "$hr"; FAIL=$((FAIL+1)); fi
}

# --- the seven triggers: posix exits, default continues ---
for pair in \
  'assign-no-cmd|readonly x=1; x=2; echo AFTER' \
  'assign-before-special|readonly x=1; x=2 export y; echo AFTER' \
  'readonly-for-var|readonly i=1; for i in a b; do :; done; echo AFTER' \
  'source-not-found|. /no/such/huck_xyz; echo AFTER' \
  'fn-name-clash|eval(){ :; }; echo AFTER' \
  'arith-error|echo $(( 1 + )); echo AFTER' \
  'special-bad-option|set -o nosuchopt; echo AFTER' \
  'export-bad-assign|export AA[4]=1; echo AFTER' \
  'return-outside-fn|return 2; echo AFTER' ; do
  check_posix   "posix:${pair%%|*}"   "${pair#*|}"
  check_default "default:${pair%%|*}" "${pair#*|}"
done

# --- case #1 boundaries that MUST continue even in posix ---
check_posix "posix:shift-oor-continues"       'shift 99; echo AFTER'
check_posix "posix:eval-false-continues"      'eval false; echo AFTER'
check_posix "posix:legit-return2-continues"   'f(){ return 2; }; f; echo AFTER'
check_posix "posix:break-continues"           'break; echo AFTER'
check_posix "posix:trap-badsig-continues"     'trap x NOSUCHSIG; echo AFTER'
check_posix "posix:command-strips"            'command set -o bad; echo AFTER'
check_posix "posix:builtin-strips"            'builtin set -o bad; echo AFTER'
check_posix "posix:command-strips-assign"     'command export AA[4]=1; echo AFTER'
check_posix "posix:assign-before-regular"     'readonly x=1; x=2 true; echo AFTER'

# --- mechanism edges ---
check_posix "posix:exit-trap-fires"   'trap "echo TRAP" EXIT; . /no/such/huck_xyz; echo AFTER'
check_posix "posix:subshell-isolated" '( . /no/such/huck_xyz; echo INNER ); echo AFTER'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 2: Build debug huck and run the harness**

Run:
```bash
cargo build --bin huck
chmod +x tests/scripts/posix_exit_on_error_diff_check.sh
HUCK_BIN=$(pwd)/target/debug/huck bash tests/scripts/posix_exit_on_error_diff_check.sh
```
Expected: every line PASS, final `Fail: 0`. A FAIL prints both `(stdout, exit)` tuples — fix the underlying behavior (do not weaken the fragment). If a stdout differs only because huck emitted an error to stdout instead of stderr, that's a real bug to fix; the seven triggers must put their diagnostics on stderr (already the case via `err_writer`).

- [ ] **Step 3: No-regression — workspace + currently-PASS categories**

Run:
```bash
cargo test --workspace 2>&1 | grep -E "test result|error\[" | tail
```
Expected: 0 failed.
Then (release build is slow; allow ~10 min) confirm no category regressed:
```bash
for c in func cprint herestr errors posix2; do
  BASH_SOURCE_DIR=/tmp/bash-5.2.21 HUCK_BASH_TEST_HELPERS=/tmp/bash-test-helpers \
    HUCK_BASH_TEST_CATEGORY=$c bash tests/bash-test-suite/runner.sh 2>&1 | grep -iE "\| $c \|"
done
```
Expected: func/cprint/herestr stay PASS; errors/posix2 unchanged (still FAIL, not newly worse — Cluster A flips nothing, as designed). If any previously-PASS category flips to FAIL, investigate before committing.

- [ ] **Step 4: Commit**

```bash
git add tests/scripts/posix_exit_on_error_diff_check.sh
git commit -m "v226 task 6: posix_exit_on_error_diff_check.sh harness (stdout+exit vs bash 5.2.21)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review (controller, before dispatch)

- **Spec coverage:** Section 1 mechanism → Task 1; the seven cases → Tasks 2 (#5/#6/#4), 3 (#2/#3), 4 (#7), 5 (#1); testing/harness → Task 6. All covered.
- **Type/name consistency:** `pending_fatal_status` (renamed), `posix_fatal(i32)`, `builtin_usage_error: Option<i32>` are introduced in Task 1 and consumed in Tasks 2-6. The executor consume (Task 5) references `command_prefix`/`resolved` which exist in `run_exec_single`.
- **No placeholders:** every code step shows complete code; Task 5's per-builtin marking lists each site + status from the verified table, with a bash-verification instruction (the one judgement-heavy task — flagged for an opus implementer).
- **Gating invariant:** every new exit goes through `posix_fatal` (no-op outside posix non-interactive); each behavior task includes a default-mode regression test asserting `pending_fatal_status == None`.
- **Merge bookkeeping** (divergences doc + memory files) is the controller's at merge per CLAUDE.md, not a task.
