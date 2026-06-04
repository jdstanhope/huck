# huck v85 — `!` pipeline negation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax. Fresh subagent per task with spec-compliance + code-quality review between tasks.

**Goal:** Implement `!` pipeline negation (`! cmd`, `if ! cmd`, `! a | b`, `! { … }`, `! ! cmd`) — negate a pipeline's exit status, exempt from `set -e`, with `$PIPESTATUS` left raw.

**Architecture:** Add `negate: bool` to `Pipeline`. Detect a run of standalone `!` words at command position (top of `parse_command`); odd count → negate. `run_pipeline` inverts the `Continue` status when `negate`. `execute_sequence_body` exempts negated pipelines from `set -e`/ERR.

**Tech Stack:** Rust 1.85+, no new dependencies.

**Spec:** `docs/superpowers/specs/2026-06-04-huck-bang-negation-design.md` (verified bash semantics + design).

**Branch:** `v85-bang-negation` (create from `main` in Preamble).

**Commit trailer (every commit):**
```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Preamble P.1: Branch setup

- [ ] **Step 1:** `git status && git rev-parse --abbrev-ref HEAD` → clean tree on `main`.
- [ ] **Step 2:** `git checkout -b v85-bang-negation`.
- [ ] **Step 3:** Baseline: `cargo test --quiet 2>&1 | grep -E "^test result" | awk '{s+=$4} END{print "Baseline:", s}'` → expect **2382**.
- [ ] **Step 4:** `cargo clippy --all-targets 2>&1 | tail -2` → clean.

---

## File-structure map

| File | Responsibility | Tasks |
|------|----------------|-------|
| `src/command.rs` | `Pipeline.negate`; bang-run detection in `parse_command` (rename body→`parse_command_inner`, wrap); update all `Pipeline {…}` literals; parser unit tests | 1 |
| `src/executor.rs` | `run_pipeline` negates the `Continue` status; `execute_sequence_body` exempts negated pipelines from ERR/errexit; executor/integration tests | 2 |
| `tests/bang_negation_integration.rs` | NEW. Binary-driven integration tests | 2 |
| `tests/scripts/bang_negation_diff_check.sh` | NEW. huck's 12th bash-diff harness | 3 |
| `docs/bash-divergences.md`, `README.md` | new `[fixed v85]` entry; update M-22/M-50/M-08 "moot" notes; changelog; README row | 3 |

---

## Task 1: AST + parser (`Pipeline.negate` + bang detection)

**Files:**
- Modify: `src/command.rs` — add `negate` field; bang detection; update all `Pipeline {…}` literals; parser unit tests.

This task only PARSES the negation (sets the flag). Execution is Task 2, so behavior won't change yet (a parsed `! false` still runs `false` and returns 1 until Task 2) — but it must no longer be a "command not found: !" and the full suite must stay green.

- [ ] **Step 1: Add the `negate` field to `Pipeline`**

In `src/command.rs` (~line 361):
```rust
pub struct Pipeline {
    /// True if the pipeline is prefixed with `!` (negate the exit status).
    pub negate: bool,
    pub commands: Vec<Command>,
}
```

- [ ] **Step 2: Build to enumerate the construction sites, add `negate: false` to each**

Run: `cargo build 2>&1 | grep -E "missing field|Pipeline" | head -40`
Every `Pipeline { commands: … }` literal that doesn't set `negate` is now an error. Add `negate: false,` to each (the parser sites AND the ~25 test sites in `src/command.rs` + `src/executor.rs`). `grep -rn "Pipeline {" src/` lists them. Do NOT change the (Task-1, next-step) negated-parse path — it sets `negate` explicitly. Re-build until clean.

- [ ] **Step 3: Write the failing parser tests**

Add to `src/command.rs` tests (use the existing parse helpers; there's a `first_pipeline(seq)` helper and `parse_one`-style helpers — mirror them):

```rust
#[test]
fn parses_bang_simple_negates() {
    let seq = parse_seq("! false");
    let p = first_pipeline(&seq);
    assert!(p.negate);
    assert_eq!(p.commands.len(), 1);
}

#[test]
fn parses_bang_pipeline_negates_whole() {
    let seq = parse_seq("! a | b");
    let p = first_pipeline(&seq);
    assert!(p.negate);
    assert_eq!(p.commands.len(), 2);
}

#[test]
fn parses_double_bang_cancels() {
    let seq = parse_seq("! ! false");
    assert!(!first_pipeline(&seq).negate);
}

#[test]
fn parses_bang_before_if_wraps() {
    // `! if true; then :; fi` → Pipeline{negate:true, [Command::If]}
    let seq = parse_seq("! if true; then :; fi");
    let p = first_pipeline(&seq);
    assert!(p.negate);
    assert_eq!(p.commands.len(), 1);
    assert!(matches!(p.commands[0], Command::If(_)));
}

#[test]
fn bang_inside_test_command_is_an_argument_not_negation() {
    // `[ ! -e x ]` → `!` is an ARG of `[`, the pipeline is NOT negated.
    let seq = parse_seq("[ ! -e x ]");
    let p = first_pipeline(&seq);
    assert!(!p.negate);
    // first stage is the `[` simple command with `!` among its args
}
```
> Use whatever sequence-parse helper the existing tests use (e.g. a local `parse_seq`/`parse_one` that lexes + parses a string to a `Sequence`). If `first_pipeline` panics on a non-Pipeline first command, that's fine — all these are pipelines.

- [ ] **Step 4: Run, expect failure**

Run: `cargo test --quiet --bin huck "bang" 2>&1 | tail -8` → the new `*bang*`/negation tests FAIL (negation not parsed yet).

- [ ] **Step 5: Add bang detection to `parse_command`**

In `src/command.rs`, rename the CURRENT `fn parse_command` (the one with the `match iter.peek().and_then(keyword_of) {…}` body, ~line 686) to `fn parse_command_inner` (keep its body + signature identical). Then add a new wrapper `parse_command` above it:

```rust
fn parse_command<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<Command, ParseError> {
    // Pipeline negation: a run of standalone `!` words at command position
    // negates the pipeline's exit status. An even count cancels (`! !` is a
    // no-op), odd negates — matching bash. `!` is detected only here (command
    // position), so `[ ! -e x ]` keeps `!` as an argument of `[`.
    let mut bangs = 0usize;
    while iter.peek().map(is_bang_word).unwrap_or(false) {
        iter.next(); // consume `!`
        bangs += 1;
    }
    if bangs == 0 {
        return parse_command_inner(iter);
    }
    let inner = parse_command_inner(iter)?;
    let negate = bangs % 2 == 1;
    Ok(match inner {
        Command::Pipeline(mut p) => {
            p.negate = negate;
            Command::Pipeline(p)
        }
        // A compound (if/while/for/case/select/{}/subshell/[[ ]]): wrap in a
        // 1-element pipeline so the negation applies to its status.
        other => Command::Pipeline(Pipeline { negate, commands: vec![other] }),
    })
}
```
> `is_bang_word` already exists (`src/command.rs:1824`) and takes `&Token`; reuse it (same call shape as `parse_test_not`). Bare `!` (a terminator/EOF follows the bang run) → `parse_command_inner` will fail to parse a command and return a `ParseError`; that's acceptable (bash errors on bare `!` in scripts; pathological). Do NOT special-case it.

- [ ] **Step 6: Run parser tests + full suite + clippy**

```bash
cargo test --quiet --bin huck "bang" 2>&1 | tail -8       # the new tests pass
cargo test --quiet 2>&1 | grep -E "^test result" | awk '{p+=$4;f+=$6} END{print "PASS="p" FAIL="f}'
cargo clippy --all-targets 2>&1 | tail -2
```
Expected: new tests pass; FAIL=0 (baseline 2382 + new; behavior unchanged since execution ignores `negate` until Task 2). Smoke that it no longer errors: `printf 'if ! false; then echo yes; fi\n' | ./target/debug/huck` — note it currently prints nothing (negation not executed yet; `! false`→runs false→1→`if` false-branch) but must NOT print "command not found: !".

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
v85 task 1: parse `!` pipeline negation (Pipeline.negate)

Pipeline gains a `negate` bool. parse_command consumes a run of standalone
`!` words at command position (odd count negates, `! !` cancels) and attaches
negation to the parsed pipeline, wrapping a compound command in a 1-element
pipeline. `!` is detected only at command position, so `[ ! -e x ]` keeps `!`
as an argument. Execution of the flag is Task 2 — parsing only here. All
Pipeline {…} literals updated with negate: false.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Execution — negate status + `set -e`/ERR exemption

**Files:**
- Modify: `src/executor.rs` — `run_pipeline` negation; `execute_sequence_body` exemption; tests.
- Create: `tests/bang_negation_integration.rs`.

- [ ] **Step 1: Negate the status in `run_pipeline`**

Replace `run_pipeline` (`src/executor.rs:925`):
```rust
fn run_pipeline(pipeline: &Pipeline, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    let outcome = if pipeline.commands.len() == 1 {
        // Single-stage: run directly in the parent (covers simple + compound stages).
        run_command(&pipeline.commands[0], shell, sink)
    } else {
        run_multi_stage(&pipeline.commands, shell, sink)
    };
    if pipeline.negate {
        // Negate the exit status only; $PIPESTATUS (set by the stage(s) above)
        // stays raw, and control-flow outcomes propagate unchanged.
        if let ExecOutcome::Continue(s) = outcome {
            return ExecOutcome::Continue(if s == 0 { 1 } else { 0 });
        }
    }
    outcome
}
```

- [ ] **Step 2: Exempt negated pipelines from `set -e`/ERR in `execute_sequence_body`**

In `src/executor.rs::execute_sequence_body`, there are two ERR/errexit sites (the `seq.first` site ~line 106 and the `seq.rest` loop site ~line 139), each shaped:
```rust
if c != 0 && shell.err_suppressed_depth == 0 && !next_is_or {
    crate::traps::fire_err_trap(shell);
    if let Some(out) = maybe_errexit(shell, c) { return out; }
}
```
Add a negated-pipeline guard. First add a small helper near `run_pipeline`:
```rust
/// True if `cmd` is a `!`-negated pipeline — exempt from `set -e`/ERR (bash).
fn is_negated_pipeline(cmd: &crate::command::Command) -> bool {
    matches!(cmd, crate::command::Command::Pipeline(p) if p.negate)
}
```
Then extend BOTH conditions to also require the command is not a negated pipeline:
- seq.first site: `&& !is_negated_pipeline(&seq.first)`.
- seq.rest loop site: `&& !is_negated_pipeline(command)` (the loop binds `(connector, command)`).
(`$?` is still set to the negated status; only ERR/errexit are skipped.)

- [ ] **Step 3: Build + write integration tests**

`cargo build 2>&1 | tail -3` (clean). Create `tests/bang_negation_integration.rs` (mirror an existing `tests/*_integration.rs` spawn helper):

```rust
//! Integration tests for v85 `!` pipeline negation.
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
fn bang_basic() {
    assert_eq!(run("! false\necho $?\n").0, "0\n");
    assert_eq!(run("! true\necho $?\n").0, "1\n");
}

#[test]
fn bang_in_if_condition() {
    assert_eq!(run("if ! false; then echo yes; fi\n").0, "yes\n");
    assert_eq!(run("if ! true; then echo yes; else echo no; fi\n").0, "no\n");
}

#[test]
fn bang_in_while_condition() {
    // `while ! true` never enters the loop.
    assert_eq!(run("while ! true; do echo x; done\necho done\n").0, "done\n");
}

#[test]
fn bang_with_and() {
    assert_eq!(run("! false && echo ran\n").0, "ran\n");
}

#[test]
fn bang_pipeline_status_and_pipestatus() {
    // negate the whole pipeline; PIPESTATUS stays raw.
    assert_eq!(run("! false | true\necho \"$? ${PIPESTATUS[@]}\"\n").0, "1 1 0\n");
}

#[test]
fn bang_exempt_from_errexit() {
    // set -e; ! true (result 1) must NOT exit the shell.
    assert_eq!(run("set -e\n! true\necho survived\n").0, "survived\n");
}

#[test]
fn bang_with_pipefail() {
    assert_eq!(run("set -o pipefail\n! false | true\necho $?\n").0, "0\n");
}

#[test]
fn bang_before_compound() {
    assert_eq!(run("! { false; }\necho $?\n").0, "0\n");
    assert_eq!(run("! (exit 3)\necho $?\n").0, "0\n");
}

#[test]
fn double_bang_cancels() {
    assert_eq!(run("! ! false\necho $?\n").0, "1\n");
}
```

- [ ] **Step 4: Run integration tests**

Run: `cargo test --test bang_negation_integration 2>&1 | grep -E "^test result"` → all pass (9). If `bang_exempt_from_errexit` fails, the errexit guard isn't catching the negated pipeline — re-check `is_negated_pipeline` is applied at BOTH sites and the command being checked is the negated `Command::Pipeline`.

- [ ] **Step 5: Full suite + clippy + commit**

```bash
cargo test --quiet 2>&1 | grep -E "^test result" | awk '{p+=$4;f+=$6} END{print "PASS="p" FAIL="f}'
cargo clippy --all-targets 2>&1 | tail -2
git add -A
git commit -m "$(cat <<'EOF'
v85 task 2: execute `!` negation + set -e/ERR exemption

run_pipeline inverts a Continue status (0<->non-zero) when pipeline.negate,
after pipefail and leaving $PIPESTATUS raw. execute_sequence_body skips ERR
and errexit for a negated pipeline (matches bash: `set -e; ! true` survives).
9 integration tests cover if/while/&&/pipeline/pipefail/errexit/compound/
double-bang.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: bash-diff harness + docs

**Files:**
- Create: `tests/scripts/bang_negation_diff_check.sh` (+x).
- Modify: `docs/bash-divergences.md`, `README.md`.

- [ ] **Step 1: Create the harness** (huck's 12th; mirror `tests/scripts/loop_levels_diff_check.sh`):

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v85 `!` pipeline negation.
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
check "bang false"        '! false; echo $?'
check "bang true"         '! true; echo $?'
check "bang if"           'if ! false; then echo yes; fi'
check "bang while"        'while ! true; do echo x; done; echo done'
check "bang and"          '! false && echo ran'
check "bang pipeline ps"  '! false | true; echo "$? ${PIPESTATUS[@]}"'
check "bang errexit"      'set -e; ! true; echo survived'
check "bang pipefail"     'set -o pipefail; ! false | true; echo $?'
check "bang brace"        '! { false; }; echo $?'
check "bang subshell"     '! (exit 3); echo $?'
check "double bang"       '! ! false; echo $?'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```
`chmod +x tests/scripts/bang_negation_diff_check.sh`.

- [ ] **Step 2: Build + run harness; iterate to all-pass**

```bash
cargo build --quiet
tests/scripts/bang_negation_diff_check.sh
```
Expected: `Total: 11, Pass: 11, Fail: 0`. If a fragment diverges, `diff` shows it.

- [ ] **Step 3: Docs — `docs/bash-divergences.md`**

Add a new entry (a follow-on near the pipeline/control-flow entries):
```
- **M-08c: `!` pipeline negation** — `[fixed v85]` medium. `!` at command
  position negates a pipeline's exit status: `! cmd`, `if ! cmd`, `while ! cmd`,
  `! a | b`, `! cmd && …`, and `!` before compounds (`! { … }`, `! ( … )`,
  `! if …`). `! !` cancels (count parity). `$PIPESTATUS` stays raw; negation
  applies after pipefail; a `!`-pipeline is exempt from `set -e`/ERR (matches
  bash). New `Pipeline.negate`; detected at command position in `parse_command`
  (so `[ ! -e x ]` keeps `!` as an argument). Discovered loading a Debian
  ~/.bashrc (`if ! shopt -oq posix`).
```
Update the three notes that called `!` "moot"/unparsed: **M-22** (ERR-exemption note), **M-50** (pipefail/`!` note), **M-08** (set-flags) — change them to reference `!` as fixed in v85. Add a `2026-06-04` change-log entry. Update the Summary "Last updated" stamp + Tier-2 count/Notes consistently.

- [ ] **Step 4: `README.md`** — add after the v84 row:
```
| v85       | `!` pipeline negation (`if ! cmd`, `! a | b`)                   |
```

- [ ] **Step 5: Final full suite + all 12 harnesses + clippy**

```bash
cargo test --quiet 2>&1 | grep -E "^test result" | awk '{p+=$4;f+=$6} END{print "PASS="p" FAIL="f}'
cargo clippy --all-targets 2>&1 | tail -2
cargo build --quiet
for h in arrays ifs test_combinators completion function_keyword arith_for loop_levels select script_mode pipefail param_operand bang_negation; do
  echo -n "$h: "; tests/scripts/${h}_diff_check.sh 2>&1 | tail -1
done
```
Expected: FAIL=0; all 12 harnesses `Fail: 0`.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
v85 task 3: ! negation bash-diff harness + docs

tests/scripts/bang_negation_diff_check.sh (huck's 12th harness, 11 fragments
byte-identical to bash). docs: M-08c [fixed v85]; M-22/M-50/M-08 "moot" notes
updated to reference the fix; changelog + README row.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Final review checklist (before merge)

- [ ] All tests pass (`FAIL=0`); clippy clean.
- [ ] All 12 bash-diff harnesses `Fail: 0` (no regression in the prior 11).
- [ ] `! false`→0, `! true`→1; `if ! cmd`/`while ! cmd` work; `! a|b` → `$?` negated, `$PIPESTATUS` raw.
- [ ] `set -e; ! true` survives; `set -o pipefail; ! false|true` → 0.
- [ ] `! { … }` / `! ( … )` / `! if …` negate; `! !` cancels.
- [ ] `[ ! -e x ]` and `[[ ! … ]]` unaffected (the `!` there is not pipeline negation).
- [ ] No regression in existing if/while/pipeline/errexit tests.

## Merge

`AskUserQuestion` before merging (per CLAUDE.md). Then `git merge --no-ff` into `main`, push, delete branch; update memory files.
