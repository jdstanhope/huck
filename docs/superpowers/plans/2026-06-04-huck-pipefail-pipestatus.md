# huck v83 — `set -o pipefail` + `$PIPESTATUS` (M-50) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax. Fresh subagent per task with spec-compliance + code-quality review between tasks.

**Goal:** Add `set -o pipefail` (pipeline exit = rightmost non-zero stage) and `$PIPESTATUS` (indexed array of the last simple-pipeline's per-stage exit statuses, written at leaf execution sites).

**Architecture:** `ShellOptions` gains `pipefail` (no short flag). A new `Shell::set_pipestatus(&[i32])` writes a real `PIPESTATUS` indexed-array variable. `wait_pipeline_raw` surfaces the full per-stage vector; `run_multi_stage` writes `$PIPESTATUS` and applies pipefail to its return status; `run_single` and the subshell arm write a 1-element `$PIPESTATUS`. Compound commands (`if`/`for`/`while`/`case`/`{}`) are transparent (they don't write it).

**Tech Stack:** Rust 1.85+, no new dependencies.

**Spec:** `docs/superpowers/specs/2026-06-04-huck-pipefail-pipestatus-design.md` (read it — verified bash semantics + the leaf-site rule).

**Branch:** `v83-pipefail` (create from `main` in Preamble).

**Commit trailer (every commit):**
```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Preamble P.1: Branch setup

- [ ] **Step 1:** `git status && git rev-parse --abbrev-ref HEAD` → clean tree on `main`.
- [ ] **Step 2:** `git checkout -b v83-pipefail` → "Switched to a new branch".
- [ ] **Step 3:** Baseline: `cargo test --quiet 2>&1 | grep -E "^test result" | awk '{s+=$4} END{print "Baseline:", s}'` → expect **2356**.
- [ ] **Step 4:** `cargo clippy --all-targets 2>&1 | tail -2` → clean.

---

## File-structure map

| File | Responsibility | Tasks |
|------|----------------|-------|
| `src/shell_state.rs` | `ShellOptions.pipefail`; `Shell::set_pipestatus(&[i32])`; unit tests | 1, 2 |
| `src/builtins.rs` | `SHELL_OPTIONS` pipefail entry; `option_get`/`option_set` arms; unit tests | 1 |
| `src/executor.rs` | `wait_pipeline_raw`→`AllExited(Vec<i32>)`; `run_multi_stage` PIPESTATUS + pipefail status; `run_single` + subshell-arm PIPESTATUS writes | 2 |
| `tests/pipefail_integration.rs` | NEW. Binary-driven integration tests | 2 |
| `tests/scripts/pipefail_diff_check.sh` | NEW. huck's 10th bash-diff harness | 3 |
| `docs/bash-divergences.md`, `README.md` | M-50 → `[fixed v83]`; changelog; summary; README row | 3 |

---

## Task 1: `set -o pipefail` option

**Files:**
- Modify: `src/shell_state.rs` — `ShellOptions.pipefail`.
- Modify: `src/builtins.rs` — `SHELL_OPTIONS` entry; `option_get`/`option_set` arms; unit tests.

- [ ] **Step 1: Write the failing unit tests** (add to the `set`-builtin test module in `src/builtins.rs`; find it via `grep -n "mod .*tests" src/builtins.rs` near the existing option tests, or the module containing `option_get`/`option_set` tests):

```rust
#[test]
fn pipefail_option_round_trips() {
    let mut sh = Shell::new();
    assert_eq!(option_get(&sh, "pipefail"), Some(false));
    option_set(&mut sh, "pipefail", true).unwrap();
    assert_eq!(option_get(&sh, "pipefail"), Some(true));
    assert!(sh.shell_options.pipefail);
    option_set(&mut sh, "pipefail", false).unwrap();
    assert_eq!(option_get(&sh, "pipefail"), Some(false));
}

#[test]
fn pipefail_not_in_dollar_dash() {
    // pipefail has no short flag, so it must never appear in `$-`.
    let mut sh = Shell::new();
    option_set(&mut sh, "pipefail", true).unwrap();
    assert!(!sh.dollar_dash_value().contains('p'), "$- must not include pipefail");
}

#[test]
fn pipefail_listed_in_shell_options() {
    assert!(SHELL_OPTIONS.iter().any(|o| o.name == "pipefail" && o.short.is_none()));
}
```

- [ ] **Step 2: Run, expect failure**

Run: `cargo test --quiet --bin huck pipefail 2>&1 | tail -6`
Expected: compile error (`pipefail` field/arm missing).

- [ ] **Step 3: Add the `pipefail` field to `ShellOptions`** (`src/shell_state.rs`)

Find `pub struct ShellOptions { pub errexit: bool, pub nounset: bool }` (~line 107). Add the field:
```rust
pub struct ShellOptions {
    pub errexit: bool,
    pub nounset: bool,
    pub pipefail: bool,
}
```
If `ShellOptions` derives `Default`, nothing else is needed (bool→false). If it has a manual `Default`/constructor, add `pipefail: false` there. (`grep -n "ShellOptions {" src/shell_state.rs` to find construction sites; update each.)

- [ ] **Step 4: Wire `SHELL_OPTIONS` + `option_get`/`option_set`** (`src/builtins.rs`)

Add to `SHELL_OPTIONS` (~line 3882), after the nounset entry:
```rust
    OptionInfo { name: "pipefail", short: None },
```
In `option_get` (~line 3887), add an arm:
```rust
        "pipefail" => Some(shell.shell_options.pipefail),
```
In `option_set` (~line 3895), add an arm:
```rust
        "pipefail" => { shell.shell_options.pipefail = value; Ok(()) }
```

- [ ] **Step 5: Run unit tests, expect pass**

Run: `cargo test --quiet --bin huck pipefail 2>&1 | tail -6` → 3 pass.

- [ ] **Step 6: Smoke-test `set -o` from the binary**

```bash
cargo build --quiet
printf 'set -o pipefail\nset -o | grep pipefail\nset +o pipefail\nset -o | grep pipefail\necho "dash=[$-]"\n' | ./target/debug/huck
```
Expected: `pipefail\ton` then `pipefail\toff`, and `dash=[...]` with no extra letter from pipefail. (The exact `set -o` line format mirrors errexit/nounset — confirm it lists pipefail.)

- [ ] **Step 7: Full suite + clippy + commit**

```bash
cargo test --quiet 2>&1 | grep -E "^test result" | awk '{s+=$4} END{print "After Task 1:", s}'
cargo clippy --all-targets 2>&1 | tail -2
git add -A
git commit -m "$(cat <<'EOF'
v83 task 1: set -o pipefail option

ShellOptions gains a pipefail bool (default off); SHELL_OPTIONS gets a
pipefail entry with no short flag; option_get/option_set handle it. So
`set -o pipefail` / `set +o pipefail` toggle it and `set -o` lists it,
while $- is unaffected (no short letter). No behavior change yet — the
status/PIPESTATUS wiring is Task 2.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: `$PIPESTATUS` + pipefail exit status

**Files:**
- Modify: `src/shell_state.rs` — `Shell::set_pipestatus`.
- Modify: `src/executor.rs` — `wait_pipeline_raw` vector; `run_multi_stage`; `run_single`; subshell arm.
- Create: `tests/pipefail_integration.rs`.

- [ ] **Step 1: Add `Shell::set_pipestatus` + a unit test** (`src/shell_state.rs`)

Add to `impl Shell` (near `replace_array`):
```rust
/// Overwrites the `PIPESTATUS` indexed-array variable with the given
/// per-stage exit statuses. Always overwrites (even if a user marked
/// PIPESTATUS readonly) — bash maintains it unconditionally.
pub fn set_pipestatus(&mut self, statuses: &[i32]) {
    let elements: std::collections::BTreeMap<usize, String> =
        statuses.iter().enumerate().map(|(i, s)| (i, s.to_string())).collect();
    self.vars.insert(
        "PIPESTATUS".to_string(),
        Variable {
            value: VarValue::Indexed(elements),
            exported: false,
            readonly: false,
            integer: false,
        },
    );
}
```
(Confirm `self.vars` is the field name and `Variable`/`VarValue` are in scope in this module — they are, per `replace_array`.)

Test (in the `shell_state` tests module):
```rust
#[test]
fn set_pipestatus_writes_indexed_array() {
    let mut sh = Shell::new();
    sh.set_pipestatus(&[0, 1, 0]);
    let arr = sh.get_array("PIPESTATUS").expect("PIPESTATUS array");
    assert_eq!(arr.get(&0).map(String::as_str), Some("0"));
    assert_eq!(arr.get(&1).map(String::as_str), Some("1"));
    assert_eq!(arr.get(&2).map(String::as_str), Some("0"));
    assert_eq!(arr.len(), 3);
}
```

- [ ] **Step 2: Run, expect pass**

Run: `cargo test --quiet --bin huck set_pipestatus 2>&1 | tail -4` → pass.

- [ ] **Step 3: Surface the per-stage vector from `wait_pipeline_raw`** (`src/executor.rs`)

Change the enum (~line 3013):
```rust
enum PipelineWaitResult {
    AllExited(Vec<i32>),
    Stopped(i32),
}
```
Change the final return (~line 3124) from the `.last()` form to the full vector:
```rust
    crate::traps::dispatch_pending_traps(shell);
    let stages: Vec<i32> = stage_status.iter().map(|s| s.unwrap_or(1)).collect();
    PipelineWaitResult::AllExited(stages)
```
(unfilled slots → 1, preserving the prior `unwrap_or` behavior.)

- [ ] **Step 4: `run_multi_stage` — write PIPESTATUS + apply pipefail** (`src/executor.rs`, ~lines 2984-3009)

Where `run_multi_stage` consumes `wait_pipeline_raw`'s result and computes the final status, replace the `AllExited(s) => s` handling with vector handling. The current tail is:
```rust
    let status = match last_status {
        PipelineWaitResult::AllExited(s) => s,
        PipelineWaitResult::Stopped(sig) => 128 + sig,
    };
    ExecOutcome::Continue(status)
```
Change to:
```rust
    let status = match last_status {
        PipelineWaitResult::AllExited(stages) => {
            shell.set_pipestatus(&stages);
            if shell.shell_options.pipefail {
                // rightmost non-zero stage, else 0
                stages.iter().rev().find(|&&s| s != 0).copied().unwrap_or(0)
            } else {
                stages.last().copied().unwrap_or(0)
            }
        }
        PipelineWaitResult::Stopped(sig) => 128 + sig,
    };
    ExecOutcome::Continue(status)
```
Also fix the earlier `if let PipelineWaitResult::Stopped(sig) = last_status` borrow at ~line 2988 if the move of `last_status` into the match now conflicts (it's checked before the final match — keep that check using a `matches!`/reference or reorder so the final match owns `last_status`). Build will tell you; adjust minimally.

- [ ] **Step 5: `run_single` — write a 1-element PIPESTATUS on Continue** (`src/executor.rs`, ~line 1903)

Wrap the existing body so a `Continue` outcome records `$PIPESTATUS`:
```rust
fn run_single(cmd: &SimpleCommand, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    let outcome = match cmd {
        SimpleCommand::Exec(exec) => run_exec_single(exec, shell, sink),
        SimpleCommand::Assign(items) => {
            // (existing Assign body unchanged)
            for a in items {
                let name = a.target.name();
                if shell.is_readonly(name) {
                    eprintln!("huck: {name}: readonly variable");
                    return ExecOutcome::Continue(1);
                }
                if apply_one_assignment(a, shell).is_err() {
                    return ExecOutcome::Continue(1);
                }
            }
            ExecOutcome::Continue(0)
        }
    };
    if let ExecOutcome::Continue(c) = outcome {
        shell.set_pipestatus(&[c]);
    }
    outcome
}
```
> NOTE: the two early `return ExecOutcome::Continue(1)` in the Assign arm bypass the wrapper. For consistency, change them to assign into a result instead, OR (simpler) leave them — a readonly/failed assignment is an error path; bash still sets PIPESTATUS=(1) there. To keep it correct, replace those `return`s with `ExecOutcome::Continue(1)` as the match value (break out via a labeled block) — OR accept the minor gap and document. PREFERRED: restructure the Assign arm to compute an `ExecOutcome` without early `return` so the wrapper always runs. Show the assign arm computing a status:
```rust
        SimpleCommand::Assign(items) => {
            let mut st = 0;
            for a in items {
                let name = a.target.name();
                if shell.is_readonly(name) {
                    eprintln!("huck: {name}: readonly variable");
                    st = 1;
                    break;
                }
                if apply_one_assignment(a, shell).is_err() {
                    st = 1;
                    break;
                }
            }
            ExecOutcome::Continue(st)
        }
```

- [ ] **Step 6: Subshell foreground arm — write a 1-element PIPESTATUS** (`src/executor.rs`, the `Command::Subshell { .. }` arm in `run_command`, ~line 161)

After the parent waits for the subshell child and computes its exit `status` (find the `waitpid`/status computation at the end of that arm, where it currently produces an `ExecOutcome::Continue(status)`), insert `shell.set_pipestatus(&[status]);` immediately before returning the `Continue(status)`. (A subshell is one forked unit → 1-element PIPESTATUS, matching bash `(true|false)` → `(1)`.)

- [ ] **Step 7: Build + write the integration tests**

Run `cargo build 2>&1 | tail -3` (clean). Create `tests/pipefail_integration.rs` (mirror an existing `tests/*_integration.rs` spawn helper):

```rust
//! Integration tests for v83 set -o pipefail + $PIPESTATUS (M-50).
use std::io::Write;
use std::process::{Command, Stdio};

fn run(script: &str) -> (String, i32) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_huck"))
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().expect("spawn huck");
    child.stdin.as_mut().unwrap().write_all(script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let o = child.wait_with_output().unwrap();
    (String::from_utf8_lossy(&o.stdout).into(), o.status.code().unwrap_or(-1))
}

#[test]
fn pipestatus_after_multistage() {
    let (out, _) = run("true | false | true\necho \"${PIPESTATUS[@]}\"\necho \"${PIPESTATUS[1]} ${#PIPESTATUS[@]}\"\n");
    assert_eq!(out, "0 1 0\n1 3\n");
}

#[test]
fn pipefail_off_default_uses_last_stage() {
    let (_o, _c) = run("false | true\necho rc=$?\n");
    assert_eq!(run("false | true\necho rc=$?\n").0, "rc=0\n");
}

#[test]
fn pipefail_on_rightmost_nonzero() {
    assert_eq!(run("set -o pipefail\nfalse | true\necho rc=$?\n").0, "rc=1\n");
    assert_eq!(run("set -o pipefail\n(exit 2) | (exit 3)\necho rc=$?\n").0, "rc=3\n");
    assert_eq!(run("set -o pipefail\ntrue | true\necho rc=$?\n").0, "rc=0\n");
}

#[test]
fn pipestatus_after_simple_command() {
    assert_eq!(run("false\necho \"${PIPESTATUS[@]}\"\n").0, "1\n");
    assert_eq!(run("true\necho \"${PIPESTATUS[@]}\"\n").0, "0\n");
}

#[test]
fn pipestatus_compound_transparency() {
    // if with false condition → PIPESTATUS reflects the condition (1), not the if (0).
    assert_eq!(run("if false; then :; fi\necho \"${PIPESTATUS[@]} rc=$?\"\n").0, "1 rc=0\n");
    // for loop body's last pipeline.
    assert_eq!(run("for i in 1; do true | false; done\necho \"${PIPESTATUS[@]}\"\n").0, "0 1\n");
    // brace group is transparent.
    assert_eq!(run("{ true | false; }\necho \"${PIPESTATUS[@]}\"\n").0, "0 1\n");
}

#[test]
fn pipestatus_subshell_is_one_element() {
    assert_eq!(run("(true | false)\necho \"${PIPESTATUS[@]}\"\n").0, "1\n");
}

#[test]
fn pipestatus_function_is_opaque() {
    assert_eq!(run("f() { true | false; }\nf\necho \"${PIPESTATUS[@]}\"\n").0, "1\n");
    assert_eq!(run("g() { return 5; }\ntrue | false | true\ng\necho \"${PIPESTATUS[@]}\"\n").0, "5\n");
}
```

- [ ] **Step 8: Run integration tests**

Run: `cargo test --test pipefail_integration 2>&1 | grep -E "^test result"` → all pass (7). If `pipestatus_compound_transparency` fails, a compound runner is wrongly writing PIPESTATUS (or `run_single`/subshell isn't) — re-check that only the three leaf sites write it and compounds don't.

- [ ] **Step 9: Full suite + clippy + commit**

```bash
cargo test --quiet 2>&1 | grep -E "^test result" | awk '{p+=$4;f+=$6} END{print "PASS="p" FAIL="f}'
cargo clippy --all-targets 2>&1 | tail -2
git add -A
git commit -m "$(cat <<'EOF'
v83 task 2: $PIPESTATUS array + pipefail exit status

New Shell::set_pipestatus writes a real PIPESTATUS indexed-array var
(overwrites unconditionally). wait_pipeline_raw now returns the full
per-stage Vec<i32>; run_multi_stage writes PIPESTATUS from it and, when
pipefail is on, returns the rightmost non-zero stage status (else the last
stage, unchanged). run_single writes a 1-element PIPESTATUS on Continue
(covers simple/builtin/function-call/assignment); the subshell arm writes
a 1-element PIPESTATUS. Compound commands (if/for/while/case/{}) are
transparent. 7 integration tests cover the array forms, pipefail exit
codes, and compound/subshell/function semantics.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: bash-diff harness + docs

**Files:**
- Create: `tests/scripts/pipefail_diff_check.sh` (+x).
- Modify: `docs/bash-divergences.md`, `README.md`.

- [ ] **Step 1: Create the harness** (huck's 10th; mirror `tests/scripts/loop_levels_diff_check.sh` for the HUCK_BIN check + PASS/FAIL counting). Each fragment runs through `bash` and `huck` via stdin; outputs (stdout+stderr+exit) byte-identical.

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v83 set -o pipefail + $PIPESTATUS.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
check "pipestatus multistage"   'true | false | true; echo "${PIPESTATUS[@]}"'
check "pipestatus index+count"  'true | false | true; echo "${PIPESTATUS[1]} ${#PIPESTATUS[@]}"'
check "pipefail off rc"         'false | true; echo $?'
check "pipefail on rc"          'set -o pipefail; false | true; echo $?'
check "pipefail on rightmost"   'set -o pipefail; (exit 2) | (exit 3); echo $?'
check "pipefail on allzero"     'set -o pipefail; true | true; echo $?'
check "pipestatus simple"       'false; echo "${PIPESTATUS[@]}"'
check "pipestatus if-cond"      'if false; then :; fi; echo "${PIPESTATUS[@]} rc=$?"'
check "pipestatus for-body"     'for i in 1; do true | false; done; echo "${PIPESTATUS[@]}"'
check "pipestatus brace"        '{ true | false; }; echo "${PIPESTATUS[@]}"'
check "pipestatus subshell"     '(true | false); echo "${PIPESTATUS[@]}"'
check "pipestatus function"     'f() { true | false; }; f; echo "${PIPESTATUS[@]}"'
check "set -o lists pipefail"   'set -o pipefail; set -o | grep pipefail'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```
`chmod +x tests/scripts/pipefail_diff_check.sh`.

- [ ] **Step 2: Build + run harness; iterate to all-pass**

```bash
cargo build --quiet
tests/scripts/pipefail_diff_check.sh
```
Expected: `Total: 13, Pass: 13, Fail: 0`. If a fragment diverges, `diff` shows it. **Watch**: the `set -o | grep pipefail` line format must match bash's `pipefail\toff`/`on` exactly; if huck's `set -o` listing differs, that's a pre-existing format detail — if it can't be made identical, drop that fragment and note it (the unit/integration tests already cover the option).

- [ ] **Step 3: Docs — `docs/bash-divergences.md`**

Flip M-50:
```
- **M-50: `set -o pipefail` and `$PIPESTATUS`** — `[fixed v83]` medium. `set -o
  pipefail` (default off, no short flag) makes a pipeline's exit status the
  rightmost non-zero stage (else 0). `$PIPESTATUS` is an indexed array of the
  last simple-command pipeline's per-stage exit statuses, written at leaf sites
  (`run_single` → `[status]`; `run_multi_stage` → per-stage vector via
  `wait_pipeline_raw`; subshell `(...)` → `[status]`); compound commands
  (`if`/`for`/`while`/`case`/`{}`) are transparent (their inner pipelines write
  it), and a function call is opaque (`[its status]`) — all matching bash 5.2.
  New `Shell::set_pipestatus`; `ShellOptions.pipefail`. **Deferred/edges**:
  `! pipeline` negation isn't parsed by huck so the pipefail/`!` interaction is
  moot; `$PIPESTATUS` for a stopped (Ctrl-Z) pipeline is not set.
```
Add a `2026-06-04` change-log entry. Update the Summary table "Last updated" stamp and the Tier-2 Notes (`M-50 fixed by v83`).

- [ ] **Step 4: `README.md`** — add after the v82 row:
```
| v83       | `set -o pipefail` + `$PIPESTATUS` (M-50)                        |
```

- [ ] **Step 5: Final full suite + all 10 harnesses + clippy**

```bash
cargo test --quiet 2>&1 | grep -E "^test result" | awk '{p+=$4;f+=$6} END{print "PASS="p" FAIL="f}'
cargo clippy --all-targets 2>&1 | tail -2
cargo build --quiet
for h in arrays ifs test_combinators completion function_keyword arith_for loop_levels select script_mode pipefail; do
  echo -n "$h: "; tests/scripts/${h}_diff_check.sh 2>&1 | tail -1
done
```
Expected: FAIL=0; all 10 harnesses `Fail: 0`.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
v83 task 3: pipefail bash-diff harness + docs

tests/scripts/pipefail_diff_check.sh (huck's 10th harness): PIPESTATUS
arrays after pipelines/simple/compound/subshell/function + pipefail exit
codes, byte-identical to bash 5.2. docs: M-50 -> [fixed v83], changelog,
summary stamp; README v83 row.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Final review checklist (before merge)

- [ ] All tests pass (`FAIL=0`); clippy clean.
- [ ] All 10 bash-diff harnesses `Fail: 0` (no regression in the prior 9).
- [ ] `set -o pipefail` exit status = rightmost non-zero; default off = last stage.
- [ ] `$PIPESTATUS` correct after: multi-stage pipeline, simple command, `if`/`for`/`{}` (transparent), subshell (1-element), function (opaque).
- [ ] `${PIPESTATUS[@]}` / `[N]` / `${#PIPESTATUS[@]}` all work.
- [ ] `$-` has no pipefail letter; `set -o` lists pipefail.

## Merge

`AskUserQuestion` before merging (per CLAUDE.md). Then `git merge --no-ff` into `main`, push, delete branch; update memory files.
