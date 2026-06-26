# POSIX-mode Foundation + Posix-gated Special-builtin Persistence — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a real `posix` flag to huck and use it to fix special-builtin prefix-assignment persistence — fixing a default-mode correctness bug and flipping the `func` bash-test category to PASS (suite 8→9).

**Architecture:** A `posix: bool` lands in `ShellOptions`, set from `set -o posix`, `--posix`, invocation as `sh`, or `POSIXLY_CORRECT`. Special-builtin prefix persistence is then gated: `export`/`readonly` absorb their named var in both modes (unchanged), while generic specials (`return`/`:`/…) persist only in posix mode. A names-only shell-managed scope stack in `run_exec_single` lets a posix special-builtin persist survive an enclosing prefix-restore at any nesting depth.

**Tech Stack:** Rust (workspace crates `huck-engine`, `huck-cli`); bash 5.2.21 as the compat oracle; `tests/scripts/*_diff_check.sh` byte-identical harnesses.

## Global Constraints

- **Commit trailer (verbatim, every commit):** `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- **Tests:** run `cargo test --workspace` (plain `cargo test` skips most crates). Full suite ≈3698 tests; must stay green.
- **bash oracle:** system `bash` is 5.2.21. Never vendor bash source/output into committed files.
- **No regression:** the `cprint` and `herestr` bash-test categories must stay PASS; the existing test `run_exec_single_special_builtin_inline_assignment_persists` (export persists in default mode) must stay green.
- **Headline success criterion:** the `func` bash-test category flips to PASS.
- **Build note:** `cargo build --release --bin huck` is slow; prefer `cargo build --bin huck` (debug) for harness runs unless a task says otherwise.

---

### Task 1: The `posix` flag in `ShellOptions` + `set -o`/`set +o` wiring

**Files:**
- Modify: `crates/huck-engine/src/shell_state.rs:217` (add field to `ShellOptions`)
- Modify: `crates/huck-engine/src/builtins.rs:5018` (`option_get`) and `:5035` (`option_set` posix arm)
- Test: `crates/huck-engine/src/executor.rs` (test module, near the `exec_script` tests ~8200)

**Interfaces:**
- Produces: `ShellOptions.posix: bool` (default `false`); `set -o posix`/`set +o posix` now read/write it; `option_get(shell, "posix")` returns real state.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `crates/huck-engine/src/executor.rs` (it already has `exec_script`):

```rust
    #[test]
    fn set_o_posix_toggles_shell_option() {
        let mut shell = Shell::new();
        assert!(!shell.shell_options.posix, "posix defaults off");
        exec_script("set -o posix\n", &mut shell);
        assert!(shell.shell_options.posix, "set -o posix turns it on");
        exec_script("set +o posix\n", &mut shell);
        assert!(!shell.shell_options.posix, "set +o posix turns it off");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p huck-engine set_o_posix_toggles_shell_option`
Expected: FAIL — `set -o posix` is a no-op, so the second assert fails (`posix` stays false). (Compiles, because the field is referenced — if it doesn't compile yet, that's also a fail; proceed to Step 3.)

- [ ] **Step 3: Add the `posix` field to `ShellOptions`**

In `crates/huck-engine/src/shell_state.rs`, add to the `ShellOptions` struct (after the `physical` field, ~line 216):

```rust
    /// `set -o posix` / `--posix` / invoked-as-`sh` / `POSIXLY_CORRECT`: enable
    /// strict POSIX semantics. Currently gates special-builtin prefix-assignment
    /// persistence (executor.rs); more posix-mode behaviors hang off this later.
    pub posix: bool,
```

(`ShellOptions` derives `Default`, so `posix` defaults to `false` automatically.)

- [ ] **Step 4: Wire `option_get` and `option_set`**

In `crates/huck-engine/src/builtins.rs`, add to `option_get` (after the `"physical"` arm, ~line 5017):

```rust
        "posix" => Some(shell.shell_options.posix),
```

Replace the `option_set` `"posix"` no-op arm (~lines 5035-5044) with:

```rust
        "posix" => { shell.shell_options.posix = value; Ok(()) }
```

- [ ] **Step 5: Run the test to verify it passes**

Run: `cargo test -p huck-engine set_o_posix_toggles_shell_option`
Expected: PASS

- [ ] **Step 6: Run the broader option/set tests to confirm no regression**

Run: `cargo test -p huck-engine option_ set_ -- --test-threads=1`
Expected: PASS (existing `set -o` / option tests unaffected; `set -o` listing now shows `posix on/off` truthfully).

- [ ] **Step 7: Commit**

```bash
git add crates/huck-engine/src/shell_state.rs crates/huck-engine/src/builtins.rs crates/huck-engine/src/executor.rs
git commit -m "v225 task 1: real posix flag in ShellOptions + set -o posix wiring

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Gate generic special-builtin persistence on `posix`

**Files:**
- Modify: `crates/huck-engine/src/executor.rs:4239` (the `persistent` decision)
- Test: `crates/huck-engine/src/executor.rs` (test module)

**Interfaces:**
- Consumes: `ShellOptions.posix` (Task 1).
- Produces: `persistent` is true for `export`/`readonly` in both modes, and for any other special builtin only when `posix`. (Section 3 builds on this same `persistent` value.)

**Background (measured bash 5.2.21), do NOT widen:**
- `FOO=val export FOO` → `FOO=val` in BOTH default and posix mode.
- `var=20 return` / `var=20 :` → restored (`0`) in default mode, persisted (`20`) in posix mode.

- [ ] **Step 1: Write the failing tests**

Add to the test module in `crates/huck-engine/src/executor.rs`:

```rust
    #[test]
    fn special_builtin_prefix_does_not_persist_in_default_mode() {
        // `:` is a special builtin; in DEFAULT mode the prefix is temporary.
        let mut shell = Shell::new();
        exec_script("var=0\nvar=20 :\n", &mut shell);
        assert_eq!(shell.get("var"), Some("0"), "default mode restores the prefix");
    }

    #[test]
    fn special_builtin_prefix_persists_in_posix_mode() {
        let mut shell = Shell::new();
        exec_script("set -o posix\nvar=0\nvar=20 :\n", &mut shell);
        assert_eq!(shell.get("var"), Some("20"), "posix mode persists the prefix");
    }

    #[test]
    fn export_prefix_persists_in_default_mode() {
        // export/readonly absorb their named var even in default mode (regression
        // guard alongside run_exec_single_special_builtin_inline_assignment_persists).
        let mut shell = Shell::new();
        exec_script("FOO=val export FOO\n", &mut shell);
        assert_eq!(shell.get("FOO"), Some("val"), "export keeps its named var");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p huck-engine special_builtin_prefix_does_not_persist_in_default_mode`
Expected: FAIL — today huck persists `var=20 :` unconditionally, so `var` is `20`, not `0`. (The posix and export tests already pass today; the default-mode one is the failing driver.)

- [ ] **Step 3: Gate the `persistent` decision**

In `crates/huck-engine/src/executor.rs`, replace line 4239:

```rust
    let persistent = builtins::is_special_builtin(&resolved.program);
```

with:

```rust
    // `export`/`readonly` absorb their named variable in BOTH modes (bash
    // assignment-builtin semantics). Every other special builtin persists its
    // prefix only under `set -o posix`; default mode restores it (POSIX 2.14 is
    // posix-mode-only in bash). `declare`/`typeset`/`local` are not special, so
    // they already restore correctly.
    let persistent = matches!(resolved.program.as_str(), "export" | "readonly")
        || (builtins::is_special_builtin(&resolved.program) && shell.shell_options.posix);
```

- [ ] **Step 4: Run the new tests to verify they pass**

Run: `cargo test -p huck-engine special_builtin_prefix export_prefix`
Expected: PASS (all three).

- [ ] **Step 5: Confirm the existing persistence test still passes**

Run: `cargo test -p huck-engine run_exec_single_special_builtin_inline_assignment_persists`
Expected: PASS (export still persists in default mode).

- [ ] **Step 6: Run the full engine suite to catch any test that encoded the old over-persist**

Run: `cargo test -p huck-engine`
Expected: PASS. If any other test fails because it asserted a non-export special builtin's prefix persisting in default mode, that test encoded the bug — fix its expectation to bash's value (verify the case with `bash -c '<frag>'` first), and note it in the report.

- [ ] **Step 7: Commit**

```bash
git add crates/huck-engine/src/executor.rs
git commit -m "v225 task 2: gate generic special-builtin prefix persistence on posix

export/readonly still absorb their named var in default mode; return/:/eval/...
now persist only under set -o posix, matching bash 5.2.21.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Scope stack — posix persist survives an enclosing prefix-restore (the func flip)

**Files:**
- Modify: `crates/huck-engine/src/shell_state.rs:461` (field decl) and `:753` (init in `Shell::new`)
- Modify: `crates/huck-engine/src/executor.rs` — push after `apply` (~4173), new `finalize_inline_scope` helper (immediately after `restore_inline_assignments`, ~6459), and three call-site replacements (~4266, ~4333, ~4342)
- Test: `crates/huck-engine/src/executor.rs` (test module)

**Interfaces:**
- Consumes: `persistent` (Task 2), `AssignmentSnapshot`, `restore_inline_assignments`, `Shell::restore_var`.
- Produces: `Shell.inline_scopes: Vec<std::collections::HashSet<String>>`; `fn finalize_inline_scope(snap: AssignmentSnapshot, persistent: bool, shell: &mut Shell)`.

**CRITICAL — touch `run_exec_single` ONLY.** `apply_inline_assignments`/`restore_inline_assignments` are also called by `run_double_bracket`, `run_background_sequence`, and `run_multi_stage`. Those must keep calling the plain `restore_inline_assignments` (they never push to `inline_scopes`, so they must never pop). Only `run_exec_single` pushes and uses `finalize_inline_scope`.

- [ ] **Step 1: Write the failing tests**

Add to the test module in `crates/huck-engine/src/executor.rs`:

```rust
    #[test]
    fn posix_special_persist_survives_enclosing_prefix() {
        // func3.sub line 155: outer prefix restore must NOT clobber the inner
        // posix special-builtin persist.
        let mut shell = Shell::new();
        exec_script(
            "set -o posix\nvar=0\nf(){ var=20 return 5; }\nvar=30 f\n",
            &mut shell,
        );
        assert_eq!(shell.get("var"), Some("20"));
        assert!(shell.inline_scopes.is_empty(), "scope stack balanced");
    }

    #[test]
    fn default_special_persist_does_not_survive_enclosing_prefix() {
        let mut shell = Shell::new();
        exec_script(
            "var=0\nf(){ var=20 return 5; }\nvar=30 f\n",
            &mut shell,
        );
        assert_eq!(shell.get("var"), Some("0"));
        assert!(shell.inline_scopes.is_empty());
    }

    #[test]
    fn posix_special_persist_survives_multi_level_enclosing() {
        let mut shell = Shell::new();
        exec_script(
            "set -o posix\na=0\nm(){ a=3 return; }\no(){ a=2 m; }\na=1 o\n",
            &mut shell,
        );
        assert_eq!(shell.get("a"), Some("3"));
        assert!(shell.inline_scopes.is_empty());
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p huck-engine posix_special_persist_survives_enclosing_prefix`
Expected: FAIL — `shell.inline_scopes` doesn't exist yet (compile error), and behaviorally the outer restore clobbers `var` to `0`. (Compile failure counts as the failing state; proceed.)

- [ ] **Step 3: Add the `inline_scopes` field to `Shell`**

In `crates/huck-engine/src/shell_state.rs`, add the field declaration after `pub call_stack: Vec<Frame>,` (~line 461):

```rust
    /// Stack of name-sets for the currently-active inline-assignment scopes in
    /// `run_exec_single` (innermost last). A posix special-builtin persist
    /// deletes its names from all enclosing scopes so the live value survives
    /// their restores. Empty between top-level commands.
    pub inline_scopes: Vec<std::collections::HashSet<String>>,
```

And initialize it in the `Shell::new` struct literal, next to `call_stack: Vec::new(),` (~line 753):

```rust
            inline_scopes: Vec::new(),
```

- [ ] **Step 4: Push the scope after a successful apply**

In `crates/huck-engine/src/executor.rs`, immediately after the `let snap = match apply_inline_assignments(...) { ... };` block (after line 4173, the closing `};`), insert:

```rust
    // Section 3: track this command's snapshotted names on a shell-managed stack
    // so a nested posix special-builtin persist can delete them from enclosing
    // scopes. finalize_inline_scope (every exit path below) pops exactly this.
    shell.inline_scopes.push(snap.iter().map(|(n, _)| n.clone()).collect());
```

(The apply-*error* arm at line 4169 runs before this push and keeps its plain `restore_inline_assignments(s, shell)` — do not change it.)

- [ ] **Step 5: Add the `finalize_inline_scope` helper**

In `crates/huck-engine/src/executor.rs`, add immediately after `restore_inline_assignments` (its closing brace is ~line 6459; place the new helper right after it):

```rust
/// Pops the top `inline_scopes` entry pushed by `run_exec_single` and finalizes
/// this command's inline assignments. NON-persistent: restore LIFO, but skip any
/// name a nested posix special-builtin persist deleted from this scope.
/// PERSISTENT (posix special builtin / export / readonly): keep the live values
/// and delete these names from every enclosing scope so their restores skip them.
fn finalize_inline_scope(snap: AssignmentSnapshot, persistent: bool, shell: &mut Shell) {
    let kept = shell.inline_scopes.pop().unwrap_or_default();
    if persistent {
        for (name, _) in &snap {
            for scope in shell.inline_scopes.iter_mut() {
                scope.remove(name);
            }
        }
    } else {
        for (name, prior) in snap.into_iter().rev() {
            if kept.contains(&name) {
                shell.restore_var(&name, prior);
            }
        }
    }
}
```

- [ ] **Step 6: Replace the three `run_exec_single` exit-path restores**

In `crates/huck-engine/src/executor.rs`, replace each of these three occurrences (all inside `run_exec_single`):

(a) The restricted-name return (~lines 4265-4267):

```rust
        if !persistent {
            restore_inline_assignments(snap, shell);
        }
```
→
```rust
        finalize_inline_scope(snap, persistent, shell);
```

(b) The child-redir-plan-error return (~lines 4332-4334):

```rust
                if !persistent {
                    restore_inline_assignments(snap, shell);
                }
```
→
```rust
                finalize_inline_scope(snap, persistent, shell);
```

(c) The main post-dispatch path (~lines 4341-4343):

```rust
    if !persistent {
        restore_inline_assignments(snap, shell);
    }
```
→
```rust
    finalize_inline_scope(snap, persistent, shell);
```

Leave the `restore_inline_assignments` calls inside `run_double_bracket`, `run_background_sequence`, and `run_multi_stage` untouched.

- [ ] **Step 7: Run the new tests to verify they pass**

Run: `cargo test -p huck-engine posix_special_persist default_special_persist`
Expected: PASS (all three).

- [ ] **Step 8: Run the full engine suite (push/pop balance + no regression)**

Run: `cargo test -p huck-engine`
Expected: PASS. The existing prefix-restore tests (`prefix_assign_restores_*`, `run_exec_single_function_call_inline_assignment_does_not_persist`) exercise the non-persistent path and must stay green.

- [ ] **Step 9: Commit**

```bash
git add crates/huck-engine/src/shell_state.rs crates/huck-engine/src/executor.rs
git commit -m "v225 task 3: scope stack so posix special-builtin persist survives enclosing restore

inline_scopes name-stack in run_exec_single; finalize_inline_scope deletes
persisted names from enclosing scopes. Fixes func3.sub line 155 (5 0 -> 5 20).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: CLI plumbing — `--posix`, invoked-as-`sh`, `POSIXLY_CORRECT`

**Files:**
- Modify: `crates/huck-engine/src/shell.rs` — `CliOptions` (add field + `Default`), `parse_cli` (`--posix` arm + both `CliOptions` constructions), new `startup_posix` helper; add unit tests in the file's test module
- Modify: `crates/huck-cli/src/repl.rs:76` (set `shell_options.posix` before mode dispatch)

**Interfaces:**
- Consumes: `ShellOptions.posix` (Task 1), `parse_cli`/`CliOptions`/`RunMode`.
- Produces: `CliOptions.posix: bool`; `pub fn startup_posix(cli_posix: bool, argv0: &str, posixly_correct: bool) -> bool`.

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block in `crates/huck-engine/src/shell.rs` (it already has `parse_cli_*` tests):

```rust
    #[test]
    fn parse_cli_posix_flag() {
        let o = parse_cli(&["--posix".into(), "script.sh".into()]).unwrap();
        assert!(o.posix);
        assert_eq!(o.mode, RunMode::File { path: PathBuf::from("script.sh"), args: vec![] });
    }

    #[test]
    fn parse_cli_posix_default_off() {
        let o = parse_cli(&["script.sh".into()]).unwrap();
        assert!(!o.posix);
    }

    #[test]
    fn startup_posix_sources() {
        assert!(startup_posix(true, "/usr/bin/huck", false), "--posix");
        assert!(startup_posix(false, "/bin/sh", false), "invoked as sh");
        assert!(startup_posix(false, "sh", false), "argv0 bare sh");
        assert!(startup_posix(false, "/usr/bin/huck", true), "POSIXLY_CORRECT");
        assert!(!startup_posix(false, "/usr/bin/huck", false), "none → off");
        assert!(!startup_posix(false, "/usr/bin/bash", false), "bash basename → off");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p huck-engine parse_cli_posix startup_posix_sources`
Expected: FAIL — `CliOptions` has no `posix` field and `startup_posix` doesn't exist (compile error).

- [ ] **Step 3: Add the `posix` field to `CliOptions`**

In `crates/huck-engine/src/shell.rs`, add to the `CliOptions` struct (after `noexec`):

```rust
    /// `--posix` CLI flag — start in POSIX mode (also set later for invocation
    /// as `sh` or with `POSIXLY_CORRECT`; see `startup_posix`).
    pub posix: bool,
```

Add `posix: false,` to the `impl Default for CliOptions` literal.

- [ ] **Step 4: Parse `--posix` and thread it through both constructions**

In `parse_cli`, add a local `let mut posix = false;` next to `let mut noexec = false;`. Add a match arm (next to the `"-n"` arm):

```rust
            "--posix" => {
                posix = true;
                i += 1;
            }
```

Add `posix: false,` to the `PrintVersion` early-return `CliOptions { ... }` literal, and `posix,` to the final `Ok(CliOptions { ... })` literal.

- [ ] **Step 5: Add the `startup_posix` helper**

In `crates/huck-engine/src/shell.rs` (near `parse_cli`), add:

```rust
/// POSIX mode is enabled at startup when `--posix` was passed, the shell was
/// invoked as `sh` (argv[0] basename), or `POSIXLY_CORRECT` is in the
/// environment. Mirrors bash's startup posix-mode triggers.
pub fn startup_posix(cli_posix: bool, argv0: &str, posixly_correct: bool) -> bool {
    cli_posix
        || posixly_correct
        || std::path::Path::new(argv0)
            .file_name()
            .is_some_and(|n| n == "sh")
}
```

- [ ] **Step 6: Run the unit tests to verify they pass**

Run: `cargo test -p huck-engine parse_cli_posix startup_posix_sources`
Expected: PASS.

- [ ] **Step 7: Apply the flag at startup in `repl.rs`**

In `crates/huck-cli/src/repl.rs`, immediately after the `-n` line (`shell_cell.borrow_mut().shell_options.noexec = opts.noexec;`, line 76), insert:

```rust
    // POSIX mode: --posix, invocation as `sh`, or POSIXLY_CORRECT. Applied before
    // any program/interactive dispatch so it governs the whole session.
    {
        let argv0 = std::env::args().next().unwrap_or_default();
        let posix = huck_engine::shell::startup_posix(
            opts.posix,
            &argv0,
            std::env::var_os("POSIXLY_CORRECT").is_some(),
        );
        shell_cell.borrow_mut().shell_options.posix = posix;
    }
```

(If `huck_engine::shell::startup_posix` is not the correct public path, match how `parse_cli`/`RunMode` are already imported at the top of `repl.rs` and use the same module path.)

- [ ] **Step 8: Build the binary and smoke-test the three sources**

Run:
```bash
cargo build --bin huck
HUCK=target/debug/huck
$HUCK --posix -c 'set -o | grep posix'
$HUCK -c 'set -o | grep posix'
POSIXLY_CORRECT=1 $HUCK -c 'set -o | grep posix'
```
Expected: `posix          \ton` for the first and third; `posix          \toff` for the second.

- [ ] **Step 9: Run the full workspace suite**

Run: `cargo test --workspace`
Expected: PASS (≈3698 tests).

- [ ] **Step 10: Commit**

```bash
git add crates/huck-engine/src/shell.rs crates/huck-cli/src/repl.rs
git commit -m "v225 task 4: --posix CLI flag + invoked-as-sh + POSIXLY_CORRECT startup posix mode

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: `posix_mode_diff_check.sh` harness + func category verification

**Files:**
- Create: `tests/scripts/posix_mode_diff_check.sh`

**Interfaces:**
- Consumes: the `huck` binary (`target/debug/huck` by default), `set -o posix`, and `--posix` (Tasks 1-4).

- [ ] **Step 1: Write the diff harness**

Create `tests/scripts/posix_mode_diff_check.sh` (mirrors the existing `*_diff_check.sh` style; `check` pipes a fragment through both shells, `check_flag` exercises the `--posix` CLI flag):

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v225: posix-gated special-builtin
# prefix-assignment persistence + the posix flag plumbing.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {  # stdin-piped fragment, default mode
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash --norc --noprofile 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
check_flag() {  # --posix CLI flag, fragment via -c
    local label="$1" frag="$2" b h
    b=$(bash --posix -c "$frag" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" --posix -c "$frag" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# Generic special builtin: default restores, posix persists.
check "return default"       'var=0; f(){ var=20 return 5; }; f; echo "$? $var"'
check "return posix"         'set -o posix; var=0; f(){ var=20 return 5; }; f; echo "$? $var"'
check "colon default"        'var=0; var=20 :; echo "$var"'
check "colon posix"          'set -o posix; var=0; var=20 :; echo "$var"'
# Enclosing prefix: the func3.sub case.
check "enclosing default"    'var=0; f(){ var=20 return 5; }; var=30 f; echo "$? $var"'
check "enclosing posix"      'set -o posix; var=0; f(){ var=20 return 5; }; var=30 f; echo "$? $var"'
check "multilevel posix"     'set -o posix; a=0; m(){ a=3 return; }; o(){ a=2 m; }; a=1 o; echo "$a"'
# Assignment-builtin absorption: persists in default mode too.
check "export named default"  'FOO=val export FOO; echo "[${FOO-U}]"'
check "readonly named default" 'BAR=ro readonly BAR; echo "[${BAR-U}]"'
# Flag plumbing.
check "set -o posix listing"  'set -o posix; set -o | grep posix'
check_flag "--posix listing"  'set -o | grep posix'
check_flag "--posix return"   'var=0; f(){ var=20 return 5; }; var=30 f; echo "$? $var"'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 2: Build huck (debug) and run the harness**

Run:
```bash
cargo build --bin huck
chmod +x tests/scripts/posix_mode_diff_check.sh
HUCK_BIN=$(pwd)/target/debug/huck bash tests/scripts/posix_mode_diff_check.sh
```
Expected: every line `PASS`, final `Fail: 0`. If any line FAILs, the diff is shown — fix the underlying behavior (do not weaken the fragment).

- [ ] **Step 3: Verify the `func` bash-test category flips to PASS**

Run (uses the operator bash tree; build a release binary if the runner needs it — check the runner's `HUCK_BIN` default first):
```bash
cargo build --release --bin huck
BASH_SOURCE_DIR=/tmp/bash-5.2.21 HUCK_BASH_TEST_HELPERS=/tmp/bash-test-helpers \
  HUCK_BASH_TEST_CATEGORY=func bash tests/bash-test-suite/runner.sh
```
Expected: `func` reports PASS. If it still FAILs, read the printed "Scratch dir (full diffs)" `func.diff` and report the residual hunk verbatim (do NOT mark the task done — escalate, since the headline criterion is unmet).

- [ ] **Step 4: Confirm `cprint` and `herestr` still PASS (no regression)**

Run the runner with `HUCK_BASH_TEST_CATEGORY=cprint` and again with `herestr`.
Expected: both PASS.

- [ ] **Step 5: Commit**

```bash
git add tests/scripts/posix_mode_diff_check.sh
git commit -m "v225 task 5: posix_mode_diff_check.sh harness (persistence + flag plumbing)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review (controller, before dispatch)

- **Spec coverage:** Section 1 → Task 1 (+CLI in Task 4); Section 2 → Task 2; Section 3 → Task 3; testing/harness → Task 5. All covered.
- **Type consistency:** `inline_scopes: Vec<HashSet<String>>` and `finalize_inline_scope(snap, persistent, shell)` are introduced in Task 3 and used only there. `startup_posix(bool, &str, bool) -> bool` defined and consumed in Task 4. `ShellOptions.posix` defined in Task 1, consumed in Tasks 2-4.
- **No placeholders:** every code step shows full code and an exact command with expected output.
- **Merge bookkeeping** (divergences doc DELETE L-61 + add the `export BAR` `[deferred]` low entry; memory files; baseline 8→9) is handled by the controller at merge per CLAUDE.md, not as a task.
