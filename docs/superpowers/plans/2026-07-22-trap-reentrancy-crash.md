# v323 — Cross-signal trap re-entrancy crash Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop huck from stack-overflowing when two pseudo-signal traps (DEBUG/ERR/RETURN) whose actions run commands mutually trigger each other, by tracking the *set* of active trap signals instead of a single slot.

**Architecture:** Replace `Shell.firing_trap: Option<TrapSignal>` with `firing_traps: Vec<TrapSignal>` (a stack); the fire helpers guard on membership (`contains`) and push/pop around the action. A trap never re-enters itself; different traps still nest — matching bash.

**Tech Stack:** Rust; huck-engine crate; bash-diff harness.

Spec: `docs/superpowers/specs/2026-07-22-trap-reentrancy-crash-design.md`
Issue: [#256](https://github.com/jdstanhope/huck/issues/256)

## Global Constraints

- bash 5.2.21 fidelity + no crash. `trap false ERR; trap false DEBUG; echo x` must print `x` (rc 0), not abort. A trap must not re-enter itself; different pseudo-signals may nest (bash allows DEBUG during an ERR action, etc.).
- Preserve the v322 DEBUG behavior exactly: `fire_debug_trap` keeps its `$LINENO` reframe, `$?` save/restore, `DebugDecision` return, and `in_subroutine` (Function|Source) predicate — only the single-slot save/restore becomes a vec push/pop with a membership guard.
- Commit trailer on every commit: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`. `cargo fmt --all` before committing.
- Per repo memory: build with `cargo build -p huck`; per-crate tests single-threaded (`cargo test -p huck-engine --lib --jobs 1 -- --test-threads 1`); NEVER `cargo test --workspace` (OOMs this box); guard bash-diff sweeps with `ulimit -v 1500000` + `timeout`. Run `-p huck` trap integration binaries single-threaded before push. No GPL bash text in the repo.

---

### Task 1: Track the set of active trap signals; guard on membership

**Files:**
- Modify: `crates/huck-engine/src/shell_state.rs` (field decl ~line 825; init ~line 1113)
- Modify: `crates/huck-engine/src/traps.rs` (`fire_debug_trap`, `fire_pseudo_trap`, `clear_for_subshell`; update recursion-guard unit tests; add a cross-signal test)
- Create: `tests/scripts/trap_reentrancy_diff_check.sh`

**Interfaces:**
- Changes `Shell.firing_trap: Option<TrapSignal>` → `firing_traps: Vec<TrapSignal>`. The only readers are in `traps.rs` (verified by grep); no other module touches it.

- [ ] **Step 1: Write the crash-repro bash-diff harness (gold-standard red)**

Read `tests/scripts/trap_zero_diff_check.sh` for the exact pattern, then create `tests/scripts/trap_reentrancy_diff_check.sh` with `check`/`check` comparing `bash --norc --noprofile -c "$frag"` vs `"$HUCK_BIN" -c "$frag"` (byte-identical incl. `EXIT:$?`):

```sh
check "err+debug mutual (crash repro)" 'trap false ERR; trap false DEBUG; echo x'
check "debug action fails w/ err trap" 'd=0;e=0; trap '"'"'d=$((d+1)); false'"'"' DEBUG; trap '"'"'e=$((e+1)); false'"'"' ERR; echo start; echo "d=$d e=$e"'
check "debug during err, no err reentry" 'trap '"'"'echo "D:$BASH_COMMAND"'"'"' DEBUG; trap '"'"'echo ERR; false; false'"'"' ERR; echo hi; false'
check "return+debug under functrace" 'set -T; f(){ echo A; false; }; trap false DEBUG; trap false RETURN; f; echo done'
check "lone debug unchanged" 'n=0; trap '"'"'n=$((n+1))'"'"' DEBUG; :; :; echo "n=$n"'
check "lone err once per failure" 'e=0; trap '"'"'e=$((e+1))'"'"' ERR; false; true; false; echo "e=$e"'
```

- [ ] **Step 2: Run the harness to confirm the red state**

```bash
cargo build -p huck
ulimit -v 1500000
bash tests/scripts/trap_reentrancy_diff_check.sh || true
```
Expected: the "err+debug mutual" check FAILs — huck aborts (empty output + `EXIT:134`) where bash prints `x` (`EXIT:0`). (The abort is in the huck subprocess; the harness keeps running.)

- [ ] **Step 3: Write a failing cross-signal unit test (traps.rs)**

Add to the `traps.rs` test module (this references the new `firing_traps` field, so it won't compile until Step 4 — the red state):

```rust
#[test]
fn cross_signal_trap_does_not_reenter_itself() {
    // Debug is already on the active stack (e.g. an ERR action is running
    // inside a DEBUG action). Firing Debug again must be suppressed, even
    // though the top of the stack is Err — the old single-slot guard missed
    // this and recursed to a stack overflow (#256).
    let mut shell = Shell::new();
    shell.firing_traps = vec![TrapSignal::Debug, TrapSignal::Err];
    shell
        .traps
        .insert(TrapSignal::Debug, Some("FOO=should_not_run".to_string()));
    assert_eq!(fire_debug_trap(&mut shell), DebugDecision::Proceed);
    assert_eq!(shell.get("FOO"), None);

    // A DIFFERENT signal not on the stack still fires.
    let mut shell2 = Shell::new();
    shell2.firing_traps = vec![TrapSignal::Debug];
    shell2
        .traps
        .insert(TrapSignal::Err, Some("BAR=err_ran".to_string()));
    fire_err_trap(&mut shell2);
    assert_eq!(shell2.get("BAR"), Some("err_ran"));
}
```

- [ ] **Step 4: Implement the field + guard change**

In `shell_state.rs`: change the field decl to `pub firing_traps: Vec<crate::traps::TrapSignal>,` and its initializer from `firing_trap: None,` to `firing_traps: Vec::new(),`.

In `traps.rs` `fire_pseudo_trap`:
```rust
fn fire_pseudo_trap(shell: &mut Shell, sig: TrapSignal) {
    if shell.firing_traps.contains(&sig) {
        return;
    }
    let action = match shell.traps.get(&sig) {
        Some(Some(text)) => text.clone(),
        _ => return,
    };
    shell.firing_traps.push(sig);
    let _ = crate::shell::process_line(&action, shell, false);
    shell.firing_traps.pop();
}
```

In `traps.rs` `fire_debug_trap`, change ONLY the guard and the save/restore (keep everything else — the `eval_frame` reframe, `saved_status`, decision, `set_last_status`):
```rust
    if shell.firing_traps.contains(&TrapSignal::Debug) {
        return DebugDecision::Proceed;
    }
    // ... action lookup, eval_frame reframe (unchanged) ...
    let saved_status = shell.last_status();
    shell.firing_traps.push(TrapSignal::Debug);
    let _ = crate::shell::process_line(&action, shell, false);
    shell.firing_traps.pop();
    // ... restore eval_frame, compute in_subroutine + decision (unchanged) ...
```
(Remove the old `let prev_firing = shell.firing_trap.replace(...)` / `shell.firing_trap = prev_firing;` lines — the push/pop replaces them.)

In `clear_for_subshell`: change `shell.firing_trap = None;` to `shell.firing_traps.clear();`.

- [ ] **Step 5: Update the existing recursion-guard unit tests**

The three `*_recursion_guard_suppresses_reentry` tests and `..._different_signal_in_flight_does_not_suppress` and `clear_for_subshell_resets_firing_trap_and_err_depth` currently set/assert `firing_trap`. Change:
- `shell.firing_trap = Some(TrapSignal::X);` → `shell.firing_traps = vec![TrapSignal::X];`
- the different-signal test asserts firing restored: `assert_eq!(shell.firing_trap, Some(TrapSignal::Debug));` → `assert_eq!(shell.firing_traps, vec![TrapSignal::Debug]);`
- `clear_for_subshell` test: `assert_eq!(shell.firing_trap, None);` → `assert!(shell.firing_traps.is_empty());`

- [ ] **Step 6: Run unit tests + harness → green**

```bash
cargo test -p huck-engine --lib --jobs 1 -- --test-threads 1 trap
cargo build -p huck
ulimit -v 1500000
bash tests/scripts/trap_reentrancy_diff_check.sh
```
Expected: all unit tests pass; harness all checks PASS (the crash repro now prints `x`, no abort).

- [ ] **Step 7: Regression guards**

```bash
cargo test -p huck-engine --lib --jobs 1 -- --test-threads 1   # full engine lib
for t in trap_integration trap_pseudo_signals_integration; do
  cargo test -p huck --test "$t" --jobs 1 -- --test-threads 1 2>&1 | grep "test result"
done
cargo build --release -p huck
ulimit -v 1500000; timeout 300 bash tests/scripts/run_diff_checks.sh
# dbg-support2 must stay PASS:
ulimit -v 2000000; HUCK_BASH_TEST_CATEGORY=dbg-support2 HUCK_TEST_TIMEOUT=60 \
  BASH_SOURCE_DIR=/tmp/bash-5.2.21 timeout 150 bash tests/bash-test-suite/runner.sh 2>&1 | grep -iE "dbg-support2 \|"
```
Expected: engine lib green; both trap integration binaries green; full sweep green (new check auto-discovered); `dbg-support2 | PASS`.

- [ ] **Step 8: `cargo fmt --all` and commit**

```bash
cargo fmt --all
git add crates/huck-engine/src/shell_state.rs crates/huck-engine/src/traps.rs tests/scripts/trap_reentrancy_diff_check.sh
git commit -m "$(cat <<'EOF'
v323: track the set of active trap signals to fix the DEBUG<->ERR crash (#256)

firing_trap (a single Option slot) is replaced by firing_traps: Vec: the
recursion guard now suppresses a trap that is anywhere on the active chain,
not just at the top. Fixes the DEBUG<->ERR/RETURN stack overflow (their
single-slot value alternated so neither same-signal guard fired). A trap
never re-enters itself; different pseudo-signals still nest, matching bash.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-Review

- **Spec coverage:** the field change, both fire helpers, `clear_for_subshell`, the unit tests (incl. the cross-signal defect test), and the diff harness are all in Task 1 (spec §Design, §Testing).
- **Placeholders:** none; the `fire_debug_trap` "unchanged" regions are explicitly the v322 code already in the file (guard + save/restore are the only edits).
- **Type consistency:** `firing_traps: Vec<TrapSignal>`; `TrapSignal` is `Copy + PartialEq + Eq` so `contains`/`push`/`pop`/`vec![]` all work.
- **Scope:** only the trap recursion-guard representation changes; DEBUG decision/`$LINENO`/`$?` logic and RETURN/ERR/real-signal actions are otherwise untouched; no depth backstop (guard-only, per the design decision).
