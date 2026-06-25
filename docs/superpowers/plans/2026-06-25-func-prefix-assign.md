# Prefix assignment must not persist across a function call — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A prefix (inline) assignment `var=val funcname` must NOT persist after the function returns — matching bash 5.2.21, where only POSIX special builtins persist a prefix assignment.

**Architecture:** One-predicate fix in `executor.rs`. The `persistent` flag wrongly includes user functions; drop that term. The existing snapshot/restore machinery (`apply_inline_assignments` snapshots each var's pre-command state, `restore_var` reinstalls it unconditionally) already produces bash-identical results once the predicate is correct.

**Tech Stack:** Rust (`huck-engine`); bash diff harness under `tests/scripts/`.

## Global Constraints

- Commit trailer on EVERY commit, verbatim: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- Run the FULL suite with `cargo test --workspace` (~3688 baseline) — plain `cargo test` skips most crates.
- Byte-faithfulness oracle is system `bash` (5.2.21).
- `cargo build --release --bin huck` is slow — use a long timeout (~480000ms).
- Do NOT special-case POSIX mode: bash does not persist prefix assignments for functions in either mode. Do NOT touch the special-builtin path (it must keep persisting).

---

### Task 1: Drop user functions from the `persistent` predicate

**Files:**
- Modify: `crates/huck-engine/src/executor.rs:4213-4220` (the `persistent` decision)
- Modify/Test: `crates/huck-engine/src/executor.rs` tests module (flip the bug-encoding test at ~8132; add edge tests)
- Create: `tests/scripts/func_prefix_assign_diff_check.sh`

**Interfaces (existing, used by the tests):**
- `exec_script(src: &str, shell: &mut Shell)` — parse+run a script string (executor.rs:8511).
- `Shell::get(&str) -> Option<&str>` / `Shell::is_exported(&str) -> bool`.
- Test helpers `lit_word`, `bare_assign`, and the `SimpleCommand::Exec(ExecCommand{..})` / `Pipeline` / `Sequence` construction pattern already in the tests module.

- [ ] **Step 1: Flip the bug-encoding unit test (make it the failing test)**

In `crates/huck-engine/src/executor.rs`, replace the existing test
`run_exec_single_function_call_inline_assignment_persists` (~line 8132) with the
corrected expectation. `FOO` is unset before the command, so after
`FOO=val myfunc` it must be unset again:

```rust
    #[test]
    fn run_exec_single_function_call_inline_assignment_does_not_persist() {
        let mut shell = Shell::new();
        // Define a no-op function via the parser.
        if let Some(tokens) = crate::lexer::tokenize("myfunc() { echo ok; }").ok()
            && let Ok(Some(seq)) = crate::command::parse(tokens)
        {
            let _ = execute(&seq, &mut shell, "myfunc() { echo ok; }");
        }
        let cmd = SimpleCommand::Exec(ExecCommand {
            inline_assignments: vec![bare_assign("FOO", lit_word("val"))],
            program: lit_word("myfunc"),
            args: vec![],
            redirects: Vec::new(),
            line: 0,
        });
        let pipeline = Pipeline { negate: false, commands: vec![Command::Simple(cmd)] };
        let seq = Sequence { first: Command::Pipeline(pipeline), rest: vec![], background: false };
        let _ = execute(&seq, &mut shell, "FOO=val myfunc");
        // bash: a prefix assignment does NOT persist across a function call.
        assert_eq!(shell.get("FOO"), None);
    }
```

- [ ] **Step 2: Add edge-case unit tests (also failing on current code)**

Add these next to the test above. They pin the tricky restore-clobbers-mutation
behavior, all verified against bash 5.2.21:

```rust
    #[test]
    fn prefix_assign_restores_prior_value_over_function_global_mutation() {
        // Function's own global write to the same var is clobbered by the restore.
        let mut shell = Shell::new();
        exec_script("v=1\nf(){ v=99; }\nv=5 f\n", &mut shell);
        assert_eq!(shell.get("v"), Some("1"));
    }

    #[test]
    fn prefix_assign_restores_prior_value_over_function_local() {
        let mut shell = Shell::new();
        exec_script("v=1\nf(){ local v=99; }\nv=5 f\n", &mut shell);
        assert_eq!(shell.get("v"), Some("1"));
    }

    #[test]
    fn prefix_assign_restores_unset_over_function_unset() {
        // Function unsets the var; restore reinstates the prior value.
        let mut shell = Shell::new();
        exec_script("v=1\nf(){ unset v; }\nv=5 f\n", &mut shell);
        assert_eq!(shell.get("v"), Some("1"));
    }

    #[test]
    fn prefix_assign_with_no_prior_var_is_unset_after_function() {
        let mut shell = Shell::new();
        exec_script("f(){ :; }\nv=5 f\n", &mut shell);
        assert_eq!(shell.get("v"), None);
    }
```

- [ ] **Step 3: Run the new tests to confirm they FAIL**

Run: `cargo test -p huck-engine prefix_assign run_exec_single_function_call_inline_assignment_does_not_persist`
Expected: the 5 tests FAIL on current code (`persistent` includes functions → values persist).

- [ ] **Step 4: Apply the one-predicate fix**

In `crates/huck-engine/src/executor.rs` (~line 4213-4220), replace the comment
block and predicate:

```rust
    // Determine whether the assignments should persist after the command.
    // POSIX special builtins persist their prefix assignments. User functions
    // and regular builtins/externals do NOT — they snapshot/restore (temporary
    // scope), matching bash in both default and posix mode.
    let persistent = builtins::is_special_builtin(&resolved.program);
```

(The removed term was `|| (!bypass_functions && shell.functions.contains_key(&resolved.program))`.)

- [ ] **Step 5: Run the unit tests to confirm they PASS**

Run: `cargo test -p huck-engine prefix_assign run_exec_single_function_call_inline_assignment_does_not_persist run_exec_single_special_builtin_inline_assignment_persists`
Expected: all PASS — including the special-builtin-persists test (unchanged behavior).

- [ ] **Step 6: Add the bash diff harness**

Create `tests/scripts/func_prefix_assign_diff_check.sh`, mirroring an existing
harness's structure (e.g. `tests/scripts/func_redirect_diff_check.sh` — shebang,
`HUCK_BIN` → `target/release/huck`, bash-absent SKIP, a `fragments` array, a
PASS/FAIL loop comparing combined stdout of `bash -c "$frag"` vs
`"$HUCK_BIN" -c "$frag"`, `exit $(( FAIL>0 ? 1 : 0 ))`). Fragments:

```bash
fragments=(
  'v=1; f(){ :; }; v=5 f; echo $v'
  'v=1; f(){ v=99; }; v=5 f; echo $v'
  'v=1; f(){ local v=99; }; v=5 f; echo $v'
  'v=1; f(){ unset v; }; v=5 f; echo "[${v-UNSET}]"'
  'f(){ :; }; v=5 f; echo "[${v-UNSET}]"'
  'v=1; f(){ echo $v; }; v=5 f'
  'f(){ printenv V; }; V=x f'
  'set -o posix; v=1; f(){ :; }; v=5 f; echo $v'
  'set -o posix; v=10; f(){ v=20 return; }; f; echo $v'
)
```

Run:
```bash
cargo build --release --bin huck   # slow, ~480000ms timeout
bash tests/scripts/func_prefix_assign_diff_check.sh | tail -2
```
Expected: `Fail: 0` (every fragment byte-identical to bash).

- [ ] **Step 7: Full suite + regression guard**

Run: `cargo test --workspace`
Expected: PASS (~3688). If any other test breaks, check whether it encoded the
old function-persist behavior (update it — the old assertion was the bug) or is a
genuine regression (then the fix is wrong — STOP and report).

Run the representative bash-test categories as a guard:
```bash
for cat in func varenv exportfunc dollars; do
  BASH_SOURCE_DIR=/tmp/bash-5.2.21 HUCK_BASH_TEST_HELPERS=/tmp/bash-test-helpers \
    HUCK_BASH_TEST_CATEGORY=$cat bash tests/bash-test-suite/runner.sh 2>&1 | grep -E "\| $cat \|"
done
```
Expected: no category that previously PASSed regresses to FAIL. (func stays FAIL —
5 blockers remain; confirm its diff SHRANK by capturing the scratch `func.diff`
and noting the `18c18` AVAR hunk and the `5 30`→`5 20` hunk are gone.)

- [ ] **Step 8: Commit**

```bash
git add crates/huck-engine/src/executor.rs tests/scripts/func_prefix_assign_diff_check.sh
git commit -m "$(printf 'v221: prefix assignment does not persist across a function call\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

## Notes for the implementer

- The fix is genuinely one line plus its comment. The bulk of the work is the
  tests and the harness — they are the deliverable's proof.
- Row 2/4 behavior (a function's own global set/unset of the prefixed var is
  clobbered back to the pre-command value) is bash-faithful and intentional —
  do not try to "preserve" the function's mutation.
- Do NOT touch `apply_inline_assignments`, `restore_inline_assignments`,
  `restore_var`, or the special-builtin path. The only code change is the
  `persistent` predicate + its comment.
