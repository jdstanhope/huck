# `~/.bashrc` builtin gaps (M-103 / M-79 / M-49) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Three small independent `~/.bashrc` gaps: `[[ -o optname ]]` option test (M-103), `declare -g` global scope (M-79), `unset -f`/`-v` function/variable removal (M-49).

**Architecture:** Gap 1 touches `src/command.rs` (enum + parser table) + `src/executor.rs` (eval arm) + makes `option_get` `pub(crate)` in `src/builtins.rs`. Gaps 2 & 3 are confined to `src/builtins.rs`. No AST/scope-model changes.

**Tech Stack:** Rust. Tests: `cargo test --bin huck`, `cargo test --test <name>`, `bash tests/scripts/<name>_diff_check.sh`.

**Spec:** `docs/superpowers/specs/2026-06-07-bashrc-builtin-gaps-design.md`. Read it first.

**Commit trailer (MANDATORY, canonical — every commit):**
```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

Anchors from investigation (verify exact lines before editing — code shifts):
- `TestUnaryOp` enum ~`src/command.rs:387`; `try_unary_op` table ~`:1987`; `parse_test_atom` ~`:2120`.
- `eval_test_expr` Unary arm ~`src/executor.rs:1210` (`VarSet` special-cased ~`:1212`); `eval_unary` ~`:1246` (`VarSet` unreachable ~`:1260`).
- `option_get` ~`src/builtins.rs:3948`; `SETO_TABLE` ~`:3906`.
- `builtin_declare` flag loop ~`:896-944` (`-g` rejected ~`:926`); `builtin_declare_decl` flag loop ~`:1520`; `snapshot_for_local_scope` call ~`:986`, def ~`:815`; `shell.local_scopes` `src/shell_state.rs:315`.
- `builtin_unset` ~`src/builtins.rs:525-583` (identifier error ~`:566`, `shell.unset` ~`:576`); `shell.functions` `src/shell_state.rs:240`.

---

## Task 1: `[[ -o optname ]]` (M-103)

**Files:** `src/command.rs`, `src/executor.rs`, `src/builtins.rs`.

- [ ] **Step 1: Failing tests**

Add to a NEW `tests/bashrc_builtin_gaps_integration.rs` (copy the `run(script) -> (stdout,stderr,code)` helper from `tests/set_x_integration.rs`):

```rust
#[test]
fn dbracket_o_option_off() {
    let (out, _e, _c) = run("[[ -o emacs ]] && echo on || echo off\n");
    assert_eq!(out, "off\n");
}

#[test]
fn dbracket_o_option_on() {
    let (out, _e, _c) = run("set -o pipefail\n[[ -o pipefail ]] && echo on || echo off\n");
    assert_eq!(out, "on\n");
}

#[test]
fn dbracket_o_reflects_errexit_and_unknown_and_negation() {
    assert_eq!(run("set -e\n[[ -o errexit ]] && echo on || echo off\n").0, "on\n");
    assert_eq!(run("[[ -o bogusname ]] && echo on || echo off\n").0, "off\n");
    assert_eq!(run("[[ ! -o pipefail ]] && echo y || echo n\n").0, "y\n");
}

#[test]
fn dbracket_o_git_prompt_shape() {
    // [ -z ... ] || [[ -o PROMPT_SUBST ]] || fallback  — must parse + run.
    let (out, _e, _c) = run("[ -z \"${ZSH_VERSION-}\" ] || [[ -o PROMPT_SUBST ]] || echo fallback\n");
    assert_eq!(out, "fallback\n");
}
```
Verify each expected value against bash first (e.g. `bash -c '[[ -o emacs ]] && echo on || echo off'` → `off`). Run `cargo test --test bashrc_builtin_gaps_integration dbracket_o 2>&1 | tail` → FAIL (unterminated `[[ ]]`).

- [ ] **Step 2: Parser — add the `-o` unary op (`src/command.rs`)**

In the `TestUnaryOp` enum (~`:387`), add a variant:
```rust
    /// `[[ -o NAME ]]` — true iff `set -o NAME` is enabled.
    OptEnabled,
```
In `try_unary_op` (~`:1987`, the `match` from literal text to `Option<TestUnaryOp>`), add an arm alongside the existing ones (e.g. near `"-v"`):
```rust
        "-o" => Some(TestUnaryOp::OptEnabled),
```
No other parser change — `parse_test_atom` reads the operand automatically.

- [ ] **Step 3: `option_get` → `pub(crate)` (`src/builtins.rs`)**

Change `fn option_get(` (~`:3948`) to `pub(crate) fn option_get(`. (If it's already `pub(crate)`, no change.)

- [ ] **Step 4: Evaluator (`src/executor.rs`)**

In `eval_test_expr`'s Unary arm (~`:1210`), where `VarSet` is special-cased before the generic `eval_unary`, add an `OptEnabled` case (use the real operand variable name from the `VarSet` arm — likely `s` or `operand`):
```rust
                if matches!(op, crate::command::TestUnaryOp::OptEnabled) {
                    return Ok(crate::builtins::option_get(shell, &operand_string).unwrap_or(false));
                }
```
In `eval_unary` (~`:1246`), add an unreachable arm mirroring `VarSet`:
```rust
        TestUnaryOp::OptEnabled => unreachable!("OptEnabled handled in eval_test_expr"),
```
(Match the exact pattern/style of the existing `VarSet` handling at both sites — read them and mirror.)

- [ ] **Step 5: Run tests + clippy**
- `cargo test --test bashrc_builtin_gaps_integration dbracket_o 2>&1 | tail` → the 4 tests pass.
- `cargo test --bin huck 2>&1 | tail -3` → no regression (existing `[[ ]]`/`test` tests).
- `cargo clippy --all-targets 2>&1 | tail -3` → clean.

- [ ] **Step 6: Commit**
```bash
git add src/command.rs src/executor.rs src/builtins.rs tests/bashrc_builtin_gaps_integration.rs
git commit -m "feat: [[ -o optname ]] option test (M-103)

New TestUnaryOp::OptEnabled + `-o` in the [[ ]] unary-operator table; evaluated
via option_get (now pub(crate)), unknown/off options -> false like bash. Fixes
git-sh-prompt line 406 [[ -o PROMPT_SUBST ]] and the __git_ps1 define cascade.
test/[ keep the POSIX binary -o.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: `declare -g` (M-79)

**Files:** `src/builtins.rs`.

- [ ] **Step 1: Failing tests**

Add to `tests/bashrc_builtin_gaps_integration.rs`:
```rust
#[test]
fn declare_g_survives_function_exit() {
    let (out, _e, _c) = run("f() { declare -g G=1; }\nf\necho \"[${G-}]\"\n");
    assert_eq!(out, "[1]\n");
}

#[test]
fn declare_without_g_is_local() {
    let (out, _e, _c) = run("f() { declare L=1; }\nf\necho \"[${L-}]\"\n");
    assert_eq!(out, "[]\n");
}

#[test]
fn declare_g_toplevel_and_composed() {
    assert_eq!(run("declare -g X=2\necho \"$X\"\n").0, "2\n");
    // -g composes with -x (export)
    let (out, _e, _c) = run("f() { declare -gx E=7; }\nf\necho \"$E\"\n");
    assert_eq!(out, "7\n");
}
```
Verify vs bash. Run `cargo test --test bashrc_builtin_gaps_integration declare_ 2>&1 | tail` → `declare_g_*` FAIL ("not yet implemented").

- [ ] **Step 2: Accept `-g` in both flag loops**

In `builtin_declare` (~`:896-944`): the rejected-flag group includes `g` (~`:926`). Move `g` out of the rejected set and set a `let mut global = false;` (declared with the other flag bools) to `true` when `-g` is seen. Do the SAME in `builtin_declare_decl` (~`:1520`). (Read both loops; mirror how an existing bool flag like the export `-x` flag is captured.)

- [ ] **Step 3: Suppress the local snapshot when `-g`**

In `builtin_declare`'s per-name mutation, the call is `snapshot_for_local_scope(shell, name)` (~`:986`). Guard it:
```rust
        if !global {
            snapshot_for_local_scope(shell, name);
        } else {
            // -g: write to the global map AND drop any outer local snapshot for
            // this name, so the global value is not rolled back on function exit.
            if let Some(frame) = shell.local_scopes.last_mut() {
                frame.remove(name);
            }
        }
```
(Confirm `shell.local_scopes` is accessible here and is a `Vec<HashMap<String, Option<Variable>>>`; `frame.remove(name)` drops the snapshot. If `builtin_declare_decl` has its own snapshot call, guard it the same way.)

- [ ] **Step 4: Run tests + clippy**
- `cargo test --test bashrc_builtin_gaps_integration declare_ 2>&1 | tail` → pass.
- `cargo test --bin huck 2>&1 | tail -3` → no regression (existing declare/local tests).
- clippy clean.

- [ ] **Step 5: Commit**
```bash
git add src/builtins.rs tests/bashrc_builtin_gaps_integration.rs
git commit -m "feat: declare -g (force global scope) (M-79)

Accept -g in both declare flag loops; when set, skip the local-scope snapshot and
drop any outer snapshot for the name, so the write to shell.vars survives function
exit (bash 'declare = local without -g'). No-op at top level. Composes with -x/-i/-r.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: `unset -f` / `unset -v` (M-49)

**Files:** `src/builtins.rs`.

- [ ] **Step 1: Failing tests**

Add to `tests/bashrc_builtin_gaps_integration.rs`:
```rust
#[test]
fn unset_f_removes_function() {
    let (out, _e, _c) = run("f() { echo hi; }\nunset -f f\ntype f >/dev/null 2>&1 && echo found || echo gone\n");
    assert_eq!(out, "gone\n");
}

#[test]
fn unset_v_removes_variable() {
    let (out, _e, _c) = run("v=1\nunset -v v\necho \"[${v-}]\"\n");
    assert_eq!(out, "[]\n");
}

#[test]
fn unset_missing_is_success() {
    let (_o, _e, c) = run("unset -f NOPE_FN\n");
    assert_eq!(c, 0);
}

#[test]
fn unset_f_does_not_touch_samename_var() {
    // a variable x and a function x; unset -f x removes only the function.
    let (out, _e, _c) = run("x=VAR\nx() { :; }\nunset -f x\necho \"$x\"\n");
    assert_eq!(out, "VAR\n");
}
```
Verify vs bash. Run `cargo test --test bashrc_builtin_gaps_integration unset_ 2>&1 | tail` → `unset_f_*` FAIL ("not a valid identifier").

- [ ] **Step 2: Add the `-f`/`-v` flag scan to `builtin_unset`**

In `builtin_unset` (~`:525-583`), before the per-arg loop, consume leading flags:
```rust
    let mut mode_fn = false;   // -f selects the function namespace
    let mut names = args;
    while let Some(first) = names.first() {
        match first.as_str() {
            "-f" => { mode_fn = true; names = &names[1..]; }
            "-v" => { mode_fn = false; names = &names[1..]; }
            "--" => { names = &names[1..]; break; }
            s if s.starts_with('-') && s.len() > 1 => {
                eprintln!("huck: unset: {s}: invalid option");
                return ExecOutcome::Continue(2);
            }
            _ => break,
        }
    }
```
(Adapt to `builtin_unset`'s real signature/return type — it returns `ExecOutcome` per the dispatch; check the exact `args: &[String]` and current return. If it currently iterates `args`, switch to iterating `names`.)

Then, in the per-name loop, branch on `mode_fn`:
```rust
        if mode_fn {
            // identifier validity still applies; remove the function (success even
            // if absent, matching bash).
            shell.functions.remove(name);
            continue;
        }
        // ... existing variable path (array-element / readonly guard / shell.unset)
```
Keep the existing variable path for `-v`/no-flag unchanged.

- [ ] **Step 3: Run tests + clippy**
- `cargo test --test bashrc_builtin_gaps_integration unset_ 2>&1 | tail` → pass.
- `cargo test --bin huck 2>&1 | tail -3` → no regression (existing unset/array-unset tests).
- clippy clean.

- [ ] **Step 4: Commit**
```bash
git add src/builtins.rs tests/bashrc_builtin_gaps_integration.rs
git commit -m "feat: unset -f / unset -v (function / variable removal) (M-49)

Leading -f/-v flag scan in builtin_unset: -f removes a function (shell.functions),
-v / no flag removes a variable (existing path). Missing name is success. Bare
unset stays variable-only.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: bash-diff harness (32nd)

**Files:** Create `tests/scripts/bashrc_builtin_gaps_diff_check.sh`.

- [ ] **Step 1: Write the harness**

Copy `tests/scripts/extglob_command_sub_diff_check.sh`'s structure verbatim (HUCK_BIN, `check()` comparing combined stdout+stderr+exit of `bash --norc --noprofile` vs huck, `Total/Pass/Fail` footer, non-zero exit). `cargo build` first. Fragments (verify each under bash first; use `$'...'` for the multi-line ones):

```
[[ -o emacs ]] && echo on || echo off
set -o pipefail; [[ -o pipefail ]] && echo on || echo off
[[ -o bogusname ]] && echo on || echo off
f() { declare -g G=5; }; f; echo "[${G-}]"
f() { declare L=5; }; f; echo "[${L-}]"
g() { echo hi; }; unset -f g; type g >/dev/null 2>&1 && echo found || echo gone
v=1; unset -v v; echo "[${v-}]"
```

- [ ] **Step 2: Run**
`bash tests/scripts/bashrc_builtin_gaps_diff_check.sh 2>&1 | tail` → `Total: 7, Pass: 7, Fail: 0`. Drop+comment any fragment that legitimately diverges (confirm with both shells); do NOT mask a real bug.

- [ ] **Step 3: Commit**
```bash
git add tests/scripts/bashrc_builtin_gaps_diff_check.sh
git commit -m "test: bash-diff harness for [[ -o ]] / declare -g / unset -f (32nd)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Task 5: Documentation

**Files:** `docs/bash-divergences.md`, `README.md`.

- [ ] **Step 1: Read structure**
`grep -n '^## Change log\|Tier 1\|Last updated\|^### M-103\|^- \*\*M-49\|^- \*\*M-79\|2026-06-07' docs/bash-divergences.md | head` and `grep -n 'v106' README.md`. Read the M-49 and M-79 entries and the v106 change-log/README rows. Confirm next free Tier-1 bug number is **M-103**.

- [ ] **Step 2: Update the three entries**
- Add **M-103 `[fixed v107]`** (Tier-1): `[[ -o optname ]]` option test; new `TestUnaryOp::OptEnabled` + `option_get`; fixes the git-sh-prompt line-346 `__git_ps1` cascade. Bump Tier-1 count.
- **M-49**: flip `[deferred]` → `[fixed v107]`; describe the `-f`/`-v` flag scan.
- **M-79**: flip its deferred `-g` to shipped (`-g` `[fixed v107]` — note `-l`/`-u`/`-n` still deferred).

- [ ] **Step 3: Change-log + README row**
`2026-06-07` v107 change-log entry (style of v106): the three fixes, the git-sh-prompt cascade payoff (and the next `~/.bashrc` gap, reported from the smoke check), 32nd harness, test count. v107 README iteration row after v106.

- [ ] **Step 4: Verify + commit**
`grep -n 'M-103\|fixed v107\|v107' docs/bash-divergences.md README.md` → real numbers, no placeholders.
```bash
git add docs/bash-divergences.md README.md
git commit -m "docs: v107 — [[ -o ]] (M-103) + declare -g (M-79) + unset -f (M-49)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Final (after all tasks)
- [ ] Whole-branch review: `git log --oneline main..HEAD`, `git diff --stat main..HEAD`.
- [ ] `cargo test 2>&1 | tail -5` (green), `cargo clippy --all-targets 2>&1 | tail -2` (clean).
- [ ] All harnesses: `for f in tests/scripts/*_diff_check.sh; do bash "$f" >/dev/null 2>&1 || echo "FAIL $f"; done` (silent = pass).
- [ ] git-sh-prompt smoke: `printf 'source /usr/lib/git-core/git-sh-prompt\ndeclare -F __git_ps1 >/dev/null && echo OK\n' | ./target/debug/huck` → `OK`, no line-346 / `local:` flood. Report the next `~/.bashrc` gap.
- [ ] AskUserQuestion merge gate, then `git merge --no-ff` + push + delete branch, then update memory files.
