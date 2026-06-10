# v131 — PS4 depth-repeat + expansion in xtrace Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `set -x` repeat the first char of `$PS4` by command-sub/`eval` nesting depth (`+`/`++`/`+++`) and expand `$PS4` (escapes + `$VAR`, via huck's `expand_prompt`), matching bash for the supported cases.

**Architecture:** A `Shell.xtrace_depth` counter (incremented on the command-substitution clone in `run_substitution` and around `process_line` in `builtin_eval`); `ps4()` expands PS4 via `prompt::expand_prompt` then replicates the first char `xtrace_depth + 1` times.

**Tech Stack:** Rust. Tests: cargo integration tests capturing STDERR + a bash-diff harness comparing stderr only.

**GIT SAFETY:** Do NOT `git checkout <sha>` — stay on `v131-ps4-depth-repeat`; edit, build, commit in place. Commit trailer on every commit: `Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`.

**Reference:** spec `docs/superpowers/specs/2026-06-10-ps4-depth-repeat-design.md`. Key locations: `Shell` struct (shell_state.rs:234, fields `in_subshell`:295 / `in_completion`:302 / `getopts_sp: usize`:247), `Shell::new()` (shell_state.rs:379, inits at ~396/411-412); `run_substitution` (expand.rs:1172, `let mut cloned = shell.clone();` then `execute_capturing`); `builtin_eval` (builtins.rs:4698, calls `crate::shell::process_line(&joined, shell, true)`); `ps4()` (executor.rs:2805); `prompt::expand_prompt(template: &str, shell: &Shell) -> String` (prompt.rs:13).

---

### Task 1: Depth counter + increments + `ps4()` rewrite

All four code components land together (the depth is only observable when something increments it AND `ps4` reads it).

**Files:**
- Create: `tests/ps4_depth_repeat_integration.rs`
- Modify: `src/shell_state.rs` (field + init), `src/expand.rs` (run_substitution), `src/builtins.rs` (builtin_eval), `src/executor.rs` (ps4)

- [ ] **Step 1: Write the failing integration tests** — create `tests/ps4_depth_repeat_integration.rs`:

```rust
//! v131: PS4 depth-repeat (first char of PS4 repeated by command-sub/eval
//! nesting) + PS4 expansion (escapes + $VAR via expand_prompt).
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }
fn run(script: &str) -> (String, String, i32) {
    let mut child = Command::new(huck_bin())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().expect("spawn huck");
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(),
     String::from_utf8_lossy(&out.stderr).into_owned(),
     out.status.code().unwrap_or(-1))
}
fn lines(stderr: &str) -> Vec<String> { stderr.lines().map(String::from).collect() }
fn has(stderr: &str, line: &str) -> bool { lines(stderr).iter().any(|l| l == line) }

#[test]
fn nested_command_sub_depth() {
    let (_o, e, _c) = run("set -x\na=$(echo $(echo hi))\n");
    assert!(has(&e, "+++ echo hi"), "stderr: {e}");
    assert!(has(&e, "++ echo hi"), "stderr: {e}");
    assert!(has(&e, "+ a=hi"), "stderr: {e}");
}

#[test]
fn command_sub_in_function_depth() {
    let (_o, e, _c) = run("set -x\nf() { echo $(echo x); }\nf\n");
    assert!(has(&e, "+ f"), "stderr: {e}");
    assert!(has(&e, "++ echo x"), "stderr: {e}");
    assert!(has(&e, "+ echo x"), "stderr: {e}");
}

#[test]
fn eval_adds_depth() {
    let (_o, e, _c) = run("set -x\neval \"echo ev\"\n");
    assert!(has(&e, "+ eval 'echo ev'"), "stderr: {e}");
    assert!(has(&e, "++ echo ev"), "stderr: {e}");
}

#[test]
fn function_call_no_depth() {
    let (_o, e, _c) = run("set -x\ng() { echo y; }\nf() { g; }\nf\n");
    // f, g, echo y all at depth 1 (functions don't add depth).
    assert!(has(&e, "+ f"), "stderr: {e}");
    assert!(has(&e, "+ g"), "stderr: {e}");
    assert!(has(&e, "+ echo y"), "stderr: {e}");
}

#[test]
fn subshell_no_depth() {
    let (_o, e, _c) = run("set -x\n( echo s )\n");
    assert!(has(&e, "+ echo s"), "stderr: {e}");
}

#[test]
fn custom_first_char_repeats() {
    let (_o, e, _c) = run("set -x\nPS4='> '\na=$(echo hi)\n");
    assert!(has(&e, ">> echo hi"), "stderr: {e}");
    assert!(has(&e, "> a=hi"), "stderr: {e}");
}

#[test]
fn multi_char_ps4_repeats_first_only() {
    let (_o, e, _c) = run("set -x\nPS4='XY '\na=$(echo hi)\n");
    assert!(has(&e, "XXY echo hi"), "stderr: {e}");
    assert!(has(&e, "XY a=hi"), "stderr: {e}");
}

#[test]
fn ps4_var_expansion() {
    let (_o, e, _c) = run("P=Q\nset -x\nPS4='$P '\necho z\n");
    assert!(has(&e, "Q echo z"), "stderr: {e}");
}

#[test]
fn default_ps4_no_regression() {
    let (_o, e, _c) = run("set -x\necho hi\n");
    assert!(has(&e, "+ echo hi"), "stderr: {e}");
}
```

- [ ] **Step 2: Run to verify failures** — `cargo test --test ps4_depth_repeat_integration 2>&1 | tail -25`. Expected: the depth tests (`nested_command_sub_depth`, `command_sub_in_function_depth`, `eval_adds_depth`), `custom_first_char_repeats`, `multi_char_ps4_repeats_first_only`, `ps4_var_expansion` FAIL; `function_call_no_depth`, `subshell_no_depth`, `default_ps4_no_regression` PASS (depth 0 today).

- [ ] **Step 3: Add the `xtrace_depth` field** — in `src/shell_state.rs`, add to the `Shell` struct near `in_completion` (~line 302):

```rust
    /// xtrace (`set -x`) nesting depth: the PS4 first character is repeated
    /// `xtrace_depth + 1` times. Incremented inside a command substitution
    /// (the `run_substitution` clone) and around `eval`. Functions and plain
    /// subshells do NOT change it (matching bash).
    pub xtrace_depth: usize,
```
and in `Shell::new()` (near the `in_completion: false,` init ~line 412):
```rust
            xtrace_depth: 0,
```

- [ ] **Step 4: Increment in `run_substitution`** — in `src/expand.rs:1172`, after the clone and before `execute_capturing`:
```rust
pub fn run_substitution(seq: &Sequence, shell: &mut Shell) -> String {
    let mut cloned = shell.clone();
    cloned.xtrace_depth += 1; // PS4 depth-repeat: $() / backticks add a level (bash)
    let (output, status) = executor::execute_capturing(seq, &mut cloned);
    ...
```

- [ ] **Step 5: Save/increment/restore in `builtin_eval`** — in `src/builtins.rs:4698`, wrap the `process_line` call:
```rust
    let joined = args.join(" ");
    if joined.trim().is_empty() {
        return ExecOutcome::Continue(0);
    }
    // PS4 depth-repeat: eval's body traces one level deeper (bash). The
    // `+ eval '…'` line was already emitted at the outer depth before dispatch.
    let saved = shell.xtrace_depth;
    shell.xtrace_depth += 1;
    let r = crate::shell::process_line(&joined, shell, true);
    shell.xtrace_depth = saved;
    r
```

- [ ] **Step 6: Rewrite `ps4()`** — in `src/executor.rs:2805`:
```rust
fn ps4(shell: &Shell) -> String {
    // bash expands $PS4 (prompt escapes + $VAR, via the PS1/PS2 expander), THEN
    // replicates the FIRST char of the EXPANDED value once per nesting level.
    let raw = shell.lookup_var("PS4").unwrap_or_else(|| "+ ".to_string());
    let expanded = crate::prompt::expand_prompt(&raw, shell);
    let mut chars = expanded.chars();
    let Some(first) = chars.next() else { return String::new(); };
    let rest: String = chars.collect();
    let level = shell.xtrace_depth + 1;
    let mut out = String::with_capacity(level + rest.len());
    for _ in 0..level { out.push(first); }
    out.push_str(&rest);
    out
}
```
(Confirm `crate::prompt::expand_prompt` is the correct path and takes `(&str, &Shell)`. `ps4` takes `&Shell` — `expand_prompt` also takes `&Shell`, so no borrow conflict.)

- [ ] **Step 7: Run to verify all pass** — `cargo test --test ps4_depth_repeat_integration 2>&1 | tail -20`. Expected: all 9 pass.

- [ ] **Step 8: Build + full suite + clippy** — `cargo build 2>&1 | tail -3`; `cargo test 2>&1 | grep -E "FAILED|error\[|panicked|test result: FAILED" | head` (none — esp. the v103 `set_x_integration` + v130 `setx_trace_fidelity_integration` must stay green, default-PS4 depth-0 output is unchanged); `cargo clippy --all-targets 2>&1 | tail -3` (clean).

- [ ] **Step 9: Sanity vs bash** (report output):
```
for f in 'a=$(echo $(echo hi))' 'f() { echo $(echo x); }; f' 'eval "echo ev"' 'PS4="> "; a=$(echo hi)' 'PS4="XY "; a=$(echo hi)'; do
  b=$(printf 'set -x\n%s\n' "$f" | bash 2>&1 >/dev/null)
  h=$(printf 'set -x\n%s\n' "$f" | ./target/debug/huck 2>&1 >/dev/null)
  [ "$b" = "$h" ] && echo "MATCH: $f" || { echo "DIFF: $f"; diff <(echo "$b") <(echo "$h"); }
done
```
(All should MATCH. If a `$()`-ordering case differs, report it.)

- [ ] **Step 10: Commit**
```bash
git add src/shell_state.rs src/expand.rs src/builtins.rs src/executor.rs tests/ps4_depth_repeat_integration.rs
git commit -m "$(cat <<'EOF'
feat(v131): PS4 depth-repeat + expansion in xtrace

set -x now repeats the first char of $PS4 by command-sub/eval nesting depth
(+/++/+++) and expands $PS4 (escapes + $VAR via the PS1/PS2 expander). New
Shell.xtrace_depth incremented on the run_substitution clone ($()/backticks) and
around builtin_eval's process_line; ps4() expands then replicates the first char.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Bash-diff harness + docs (narrow L-21, add L-29)

**Files:**
- Create: `tests/scripts/ps4_depth_repeat_diff_check.sh`
- Modify: `docs/bash-divergences.md`

- [ ] **Step 1: Write the harness** — create `tests/scripts/ps4_depth_repeat_diff_check.sh`:

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v131: PS4 depth-repeat + the PS4
# expansion huck supports (escapes + $VAR). Compares STDERR only (set -x writes
# there), stdout discarded. Does NOT test $(...)/$((...))/$LINENO in PS4 — those
# are the known L-29 residual (huck's expand_prompt does not expand them).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(printf 'set -x\n%s\n' "$frag" | bash 2>&1 >/dev/null)
    h=$(printf 'set -x\n%s\n' "$frag" | "$HUCK_BIN" 2>&1 >/dev/null)
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

check "default depth nested"  'a=$(echo $(echo hi))'
check "cmdsub in function"    'f() { echo $(echo x); }; f'
check "eval depth"            'eval "echo ev"'
check "function no depth"     'g() { echo y; }; f() { g; }; f'
check "subshell no depth"     '( echo s )'
check "custom first char"     'PS4="> "; a=$(echo hi)'
check "multichar ps4"         'PS4="XY "; a=$(echo hi)'
check "triple nest custom"    'PS4="# "; a=$(echo $(echo $(echo deep)))'
check "ps4 var expansion"     'P=Q; PS4="$P "; echo z'
check "default no regression" 'echo hi'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```
`chmod +x tests/scripts/ps4_depth_repeat_diff_check.sh`.

- [ ] **Step 2: Build + run** — `cargo build 2>&1 | tail -2`; `bash tests/scripts/ps4_depth_repeat_diff_check.sh`. Expected `Fail: 0`. If a case fails, report the diff (do not mask). Note: the `\h` escape case is intentionally NOT in the harness (hostname makes the first repeated char host-dependent, hard to assert cross-machine deterministically) — it is covered conceptually by `ps4_var_expansion` in the integration tests.

- [ ] **Step 3: Narrow L-21 in `docs/bash-divergences.md`** — find `### L-21`. Currently lists items (a)–(e): (a) Flat `$PS4` (no depth-repeat + no escape/`$VAR` expansion); (b) finer compound traces; (c) decl-RHS-cmdsub edge; (d) `2>` no-suppress; (e) pipeline-stage order. v131 fixes the depth-repeat and escape/`$VAR` expansion. Edit:
  - REMOVE item (a) entirely.
  - RELETTER the remaining four: (b)→(a), (c)→(b), (d)→(c), (e)→(d).
  - Update the **Status** line: drop the "Flat `$PS4`" framing; append "; v131 — PS4 depth-repeat + `$VAR`/escape expansion now match bash".
  - Update the **bash** line: drop "depth-repeated `$PS4` with `$VAR`/escape expansion;".
  - Update **Why intentional**: drop the depth-repeat clause.
  - The prose says "four residual differences" — after removing (a) that count is now correct (four remain). Confirm the wording reads "four".

- [ ] **Step 4: Add L-29** — add a new Tier-4 entry (in the `## Tier 4` section, near L-27/L-28-area ordering) :
```markdown
- **L-29: command substitution / arithmetic / `$LINENO` not expanded in `$PS4` (and prompts)** — `[deferred]`, low. bash expands `$PS4` fully (prompt escapes + `$VAR` + `$(...)` + `$((...))` + `$LINENO`) before the xtrace depth-repeat. huck (v131) reuses `prompt::expand_prompt`, which handles Tier-A escapes (`\h`/`\u`/`\w`/…) and `$VAR`/`${VAR}` but NOT command substitution, arithmetic, or `$LINENO` (huck has no `LINENO` variable). So `PS4='[$(date)] '`, `PS4='$((x+1)) '`, and `PS4='$LINENO '` trace with those forms unexpanded (or `$LINENO`→empty). Same limitation affects PS1/PS2 (the shared `expand_prompt`). Resolving it means giving `expand_prompt` a command/arith-substitution pass (and adding `$LINENO` line-tracking) — a broader prompt-expansion enhancement. Low impact: the common `PS4='+ '`/`$VAR` cases work; depth-repeat works.
```
  - Increment the Tier-4 count in the summary table (line ~33, `| Low-impact (Tier 4) | 24 | …`) from 24 to 25.

- [ ] **Step 5: Verify docs** — `grep -n "L-21\|L-29" docs/bash-divergences.md` → L-21 present (narrowed), L-29 present. `grep -n "Flat \`\$PS4\`" docs/bash-divergences.md` → no match (removed).

- [ ] **Step 6: Full regression** — `cargo test 2>&1 | grep -E "FAILED|error\[|test result: FAILED" | head` (none); `cargo clippy --all-targets 2>&1 | tail -3` (clean); smoke: `bash tests/scripts/setx_trace_fidelity_diff_check.sh | tail -1` (the v130 harness still passes — default PS4 unchanged).

- [ ] **Step 7: Commit**
```bash
git add tests/scripts/ps4_depth_repeat_diff_check.sh docs/bash-divergences.md
git commit -m "$(cat <<'EOF'
test+docs(v131): PS4 depth-repeat harness; narrow L-21; add L-29

Add the bash-diff harness (depth + supported-expansion cases), narrow L-21 (remove
the now-fixed flat-PS4/no-depth-repeat item), and log L-29 (cmdsub/arith/$LINENO
not expanded in PS4/prompts; Tier-4 24->25) for a future iteration.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Self-review notes
- **Spec coverage:** Task 1 = Components 1-4 (field, run_substitution +1, builtin_eval save/restore, ps4 expand+repeat) + integration tests for every spec test row; Task 2 = harness + L-21 narrow + L-29 add.
- **Type/symbol consistency:** `xtrace_depth: usize` defined in shell_state.rs (Task 1 Step 3), read in `ps4` (Step 6), written in `run_substitution` (Step 4) and `builtin_eval` (Step 5). `expand_prompt(&str, &Shell)` exists (prompt.rs:13). `ps4`/`xtrace_emit` already wired into all four emit sites (v130) — no emit-site changes needed.
- **No-regress:** default `PS4='+ '` at depth 0 → `+ ` (one `+`), identical to v130; the v103/v130 set_x tests pin this and must stay green (Task 1 Step 8).
- **Ordering:** command substitutions execute during expansion (before the outer trace emit), so inner depth lines print before outer — no reordering code needed (spec "Correctness/ordering").
