# huck v142 — the `builtin` builtin Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement `builtin NAME [args…]` — run the shell builtin `NAME` directly, bypassing functions/aliases, erroring if `NAME` is not a builtin (fixes mise's `cd` wrapper: `builtin cd "$@"`).

**Architecture:** Mirror the existing `command` machinery in the executor (`run_exec_single`, `src/executor.rs`): a pre-resolve declaration interception (`builtin local x=1` recurses into the declaration path) + a post-resolve `while resolved.program == "builtin"` strip loop that sets `bypass_functions` and a new `require_builtin` flag, then a "not a shell builtin" guard before dispatch. Register `"builtin"` in `BUILTIN_NAMES`.

**Tech Stack:** Rust; `src/executor.rs` (`run_exec_single`, reuses `word_static_text`/`is_declaration_command`/`is_builtin`), `src/builtins.rs` (`BUILTIN_NAMES`, `run_builtin`).

**Reference:** spec at `docs/superpowers/specs/2026-06-11-builtin-builtin-design.md`.

**GIT SAFETY:** Do NOT `git checkout <sha>` (a detached HEAD lost commits before). Stay on `v142-builtin-builtin`. Every commit ends with `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

**Build note:** BINARY crate — `cargo test --bin huck <one filter>`, `cargo test --test <name>`, `cargo clippy --all-targets` (NOT `--lib`). Builds take minutes.

**CRITICAL ordering:** The pre-resolve declaration interception (Step 5) and the strip loop (Step 6) MUST both land in Task 1. Without the interception, `builtin local x=5` reaches the strip loop → `program=local`, `decl_args=None` → dispatch calls `run_builtin("local")` → PANIC (the declaration-command assert). Do not split them.

---

### Task 1: the `builtin` builtin (executor wiring + registration)

**Files:**
- Modify: `src/builtins.rs` (`BUILTIN_NAMES` ~line 24-31; `run_builtin` match ~line 67-130)
- Modify: `src/executor.rs` (`run_exec_single`: pre-resolve interception ~3004, strip loop + guard ~3059)
- Create: `tests/builtin_builtin_integration.rs`

- [ ] **Step 1: Write the failing integration tests** — create `tests/builtin_builtin_integration.rs`:

```rust
//! v142: the `builtin NAME [args]` builtin — runs the named shell builtin directly,
//! bypassing functions/aliases; errors if NAME is not a builtin. Fixes mise's
//! `cd(){ builtin cd "$@"; }` wrapper.
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

fn huck_c(script: &str) -> (String, String, i32) {
    let o = Command::new(huck_bin())
        .arg("-c").arg(script)
        .stdin(Stdio::null())
        .output()
        .expect("spawn huck");
    (
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
        o.status.code().unwrap_or(-1),
    )
}

#[test]
fn builtin_echo() {
    let (out, _e, code) = huck_c("builtin echo hi");
    assert_eq!(out, "hi\n", "out={out:?}");
    assert_eq!(code, 0);
}

#[test]
fn builtin_cd_runs_cd() {
    let (out, _e, code) = huck_c("builtin cd /tmp; pwd");
    assert_eq!(out, "/tmp\n", "out={out:?}");
    assert_eq!(code, 0);
}

#[test]
fn builtin_not_a_builtin_errors() {
    let (_o, err, code) = huck_c("builtin nosuchthing");
    assert!(err.contains("builtin: nosuchthing: not a shell builtin"), "err={err:?}");
    assert_eq!(code, 1);
}

#[test]
fn builtin_alone_is_noop() {
    let (out, _e, code) = huck_c("builtin; echo done");
    assert_eq!(out, "done\n", "out={out:?}");
    assert_eq!(code, 0);
}

#[test]
fn builtin_cd_wrapper_no_recursion() {
    // The mise pattern: a cd() function calling `builtin cd` must not recurse.
    let (out, _e, code) = huck_c(r#"cd(){ builtin cd "$@"; }; cd /tmp; pwd"#);
    assert_eq!(out, "/tmp\n", "out={out:?}");
    assert_eq!(code, 0);
}

#[test]
fn builtin_bypasses_cd_function() {
    // A user cd() function is bypassed (not run) by `builtin cd`.
    let (out, _e, _c) = huck_c(r#"cd(){ echo SHADOW; }; builtin cd /tmp; pwd"#);
    assert_eq!(out, "/tmp\n", "out={out:?}");
}

#[test]
fn builtin_declaration_local() {
    // `builtin local x=5` must work (declaration builtin via the pre-resolve path).
    let (out, _e, _c) = huck_c(r#"f(){ builtin local x=5; echo "$x"; }; f"#);
    assert_eq!(out, "5\n", "out={out:?}");
}

#[test]
fn type_recognizes_builtin() {
    let (out, _e, _c) = huck_c("type builtin");
    assert!(out.contains("builtin is a shell builtin"), "out={out:?}");
}
```

- [ ] **Step 2: Run — verify failure**

Run: `cargo test --test builtin_builtin_integration 2>&1 | tail -25`
Expected: all FAIL — `builtin` is `command not found` (rc 127) / `type builtin` → not found. Record.

- [ ] **Step 3: Register `"builtin"` in `BUILTIN_NAMES`** — `src/builtins.rs` ~line 29. The list has `":", "true", "false", "command",`. Add `"builtin"`:
```rust
    ":", "true", "false", "command", "builtin",
```

- [ ] **Step 4: Add a defensive `run_builtin` arm** — `src/builtins.rs`, in the `run_builtin` match (near the `"command" => …` arm), add:
```rust
        // `builtin` is normally consumed by the executor's strip loop before
        // dispatch; this guards a bare `builtin` that reaches run_builtin.
        "builtin" => ExecOutcome::Continue(0),
```
(Place it as its own arm; do NOT remove or alter the `"command"` arm.)

- [ ] **Step 5: Pre-resolve declaration interception** — `src/executor.rs`, in `run_exec_single`, immediately AFTER the existing `command` declaration block (the `if word_static_text(&cmd.program).as_deref() == Some("command") && let Some(k) = command_decl_operand_index(&cmd.args) { … return run_exec_single(&inner, …); }` block that ends ~line 3004) and BEFORE `let mut resolved = match resolve(cmd, shell) {`:
```rust
    // `builtin <decl-builtin> …` (v142): a declaration builtin reached via `builtin`
    // (e.g. `builtin local x=1`). Rewrite to the inner declaration command and
    // recurse so the normal flow builds correct decl_args + dispatches
    // run_declaration_builtin (declaration builtins can't be function-shadowed, so
    // the bypass is moot — same rationale as the `command` block above).
    if word_static_text(&cmd.program).as_deref() == Some("builtin")
        && cmd
            .args
            .first()
            .and_then(word_static_text)
            .map(|s| builtins::is_declaration_command(&s))
            .unwrap_or(false)
    {
        let inner = ExecCommand {
            inline_assignments: cmd.inline_assignments.clone(),
            program: cmd.args[0].clone(),
            args: cmd.args[1..].to_vec(),
            stdin: cmd.stdin.clone(),
            stdout: cmd.stdout.clone(),
            stderr: cmd.stderr.clone(),
        };
        return run_exec_single(&inner, shell, sink);
    }
```
(`word_static_text` takes `&Word`; `cmd.args.first()` is `Option<&Word>`, so `.and_then(word_static_text)` yields `Option<String>`. If the compiler objects to passing the fn directly, use `.and_then(|w| word_static_text(w))`.)

- [ ] **Step 6: Strip loop + guard** — `src/executor.rs`, immediately AFTER the existing `while resolved.program == "command" { … }` loop (it closes ~line 3059) and BEFORE the inline-assignment block (`let snap = match apply_inline_assignments(…`):
```rust
    // `builtin NAME args` (v142): run NAME as a shell BUILTIN ONLY, suppressing
    // function/alias lookup; error if NAME is not a builtin. Sibling to `command`.
    // (A declaration target is intercepted pre-resolve and never reaches here.)
    let mut require_builtin = false;
    while resolved.program == "builtin" {
        match resolved.args.first() {
            None => return ExecOutcome::Continue(0), // `builtin` alone
            Some(_) => {
                let new_program = resolved.args[0].clone();
                let new_args = resolved.args[1..].to_vec();
                resolved.program = new_program;
                resolved.args = new_args;
                resolved.decl_args = None;
                bypass_functions = true;
                require_builtin = true;
                // loop: collapse `builtin builtin …`
            }
        }
    }
    if require_builtin && !builtins::is_builtin(&resolved.program) {
        eprintln!("huck: builtin: {}: not a shell builtin", resolved.program);
        return ExecOutcome::Continue(1);
    }
```
(`bypass_functions` is the existing `let mut bypass_functions` from the `command` block — in scope here. `require_builtin` is new.)

- [ ] **Step 7: Build, run tests, clippy**

Run: `cargo build 2>&1 | tail -5`
Run: `cargo test --test builtin_builtin_integration 2>&1 | tail -20` → all 8 PASS.
Run: `cargo test --bin huck command 2>&1 | tail -8` → the existing `command`-builtin tests still green (the `command` path must be unchanged).
Run: `cargo clippy --all-targets 2>&1 | tail -8` → no new warnings.

- [ ] **Step 8: Commit**

```bash
git add src/builtins.rs src/executor.rs tests/builtin_builtin_integration.rs
git commit -m "$(printf 'feat: the builtin builtin (builtin NAME args, bypass functions)\n\nMirrors the command machinery: pre-resolve declaration interception +\na post-resolve strip loop + a not-a-shell-builtin guard. Fixes mise cd.\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 2: Bash-diff harness (the 62nd)

**Files:**
- Create: `tests/scripts/builtin_builtin_diff_check.sh`

- [ ] **Step 1: Write the harness** — create `tests/scripts/builtin_builtin_diff_check.sh`:
```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v142: the `builtin` builtin.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(bash -c "$frag" 2>&1; echo "rc=$?")
    h=$("$HUCK_BIN" -c "$frag" 2>&1; echo "rc=$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
check "builtin echo"     'builtin echo hi'
check "builtin cd"       'builtin cd /tmp; pwd'
check "builtin alone"    'builtin; echo "rc=$?"'
check "cd wrapper"       'cd(){ builtin cd "$@"; }; cd /tmp; pwd'
check "bypass cd fn"     'cd(){ echo SHADOW; }; builtin cd /tmp; pwd'
check "builtin local"    'f(){ builtin local x=5; echo "$x"; }; f'
check "builtin pwd"      'builtin cd /tmp; builtin pwd'
check "command -v"       'command -v builtin'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```
NOTE: the `builtin nosuchthing` error case is deliberately OMITTED from the harness — bash's message has its `bash: line N:` prefix vs huck's `huck:` prefix (the established program-name-prefix divergence class), so stderr won't be byte-identical. That case is covered by the integration test (which matches the message body) instead.

- [ ] **Step 2: chmod + build + run**

Run: `chmod +x tests/scripts/builtin_builtin_diff_check.sh && cargo build 2>&1 | tail -2 && bash tests/scripts/builtin_builtin_diff_check.sh`
Expected: `Total: 8, Pass: 8, Fail: 0`. If any FAILs, paste the diff and STOP (real divergence — do not weaken).

- [ ] **Step 3: Commit**

```bash
git add tests/scripts/builtin_builtin_diff_check.sh
git commit -m "$(printf 'test: 62nd bash-diff harness for the builtin builtin\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 3: Docs — can't-shadow-`builtin` divergence

**Files:**
- Modify: `docs/bash-divergences.md`

- [ ] **Step 1: Extend L-19 for `builtin`**

L-19 documents the `command CMD` bare-form edges, including "(c) a function named `command` cannot shadow the builtin". `builtin` has the same property (the executor interception runs before function lookup). Find L-19 (`grep -n "L-19" docs/bash-divergences.md`) and add a sentence noting that a user function named `builtin` ALSO cannot shadow the builtin (same `[intentional]` rationale — the unconditional interception is what makes `builtin`/`command` reliably bypass functions). Do NOT change the Tier-4 count (this folds into the existing L-19 entry).

- [ ] **Step 2: Commit**

```bash
git add docs/bash-divergences.md
git commit -m "$(printf 'docs: note builtin cannot be function-shadowed (folds into L-19)\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 4: Full regression

**Files:** none (verification only)

- [ ] **Step 1: Full test suite**

Run: `cargo test 2>&1 | tail -30`
Expected: ALL pass (baseline after v141 was 3080 tests; v142 adds 8 integration tests). Zero failures. Paste any failure.

- [ ] **Step 2: `command` / function / declaration suites explicitly (the paths v142 touches)**

Run: `cargo test --bin huck command 2>&1 | tail -8` (the `command` builtin path — must be unchanged).
Run: `cargo test --test builtin_builtin_integration 2>&1 | tail -8` → 8 pass.
Run: `cargo test --bin huck declar local 2>&1 | tail -8` (declaration builtins — run filters separately; must not regress).

- [ ] **Step 3: All bash-diff harnesses**

Run: `cargo build 2>&1 | tail -2 && for f in tests/scripts/*_diff_check.sh; do printf '== %s == ' "$f"; bash "$f" | tail -1; done`
Expected: every harness ends with `Fail: 0` (incl. the new `builtin_builtin_diff_check.sh` → `Pass: 8`).

- [ ] **Step 4: Clippy**

Run: `cargo clippy --all-targets 2>&1 | tail -8`
Expected: clean.

- [ ] **Step 5: Payoff — mise cd works end-to-end**

Build: `cargo build --release 2>&1 | tail -2`. Then (mise is installed at `~/.local/bin/mise` on this box):
Run: `target/release/huck -c 'eval "$(~/.local/bin/mise activate bash)"; cd /tmp && pwd'` → prints `/tmp` (NOT `command not found: builtin`). Paste the output. This is the real-world fix.

- [ ] **Step 6: Commit (only if a verification-driven fix was needed)**

If Steps 1-4 surfaced a real issue, make the SMALLEST fix, re-run, commit with the trailer. Otherwise no commit — verification only.

---

## Notes for the implementer
- **Pre-resolve interception (Step 5) and strip loop (Step 6) ship together** — splitting them leaves `builtin local` panicking via `run_builtin`.
- **`$((`-style edge n/a here** — `builtin` only rewrites program/args; redirects on `builtin cd >file` ride the existing redirect machinery (untouched `cmd.stdin/stdout/stderr`).
- **`require_builtin` guard runs before inline-assignments/xtrace** — an error exits early (matches bash erroring before running).
- **Do NOT alter the `command` block/loop** — the `builtin` loop is added immediately after it, gated on `program == "builtin"`.
- **`word_static_text` is `fn(&Word) -> Option<String>`**, `is_declaration_command` / `is_builtin` are `builtins::` fns — all already used in `run_exec_single`/`command_decl_operand_index`.
