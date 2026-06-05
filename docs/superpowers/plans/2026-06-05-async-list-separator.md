# `&` Async List Separator (with And-Or Grouping) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `&` works as a list separator that backgrounds the preceding and-or group and continues — `a & b`, `cmd & cmd2 &`, `for … do cmd & done`, `{ cmd & cmd2; }`, `( a & b )`, and `a && b &` (backgrounds the whole group). Unblocks nvm.sh line 1192.

**Architecture:** Add `Connector::Amp` to the flat `Sequence` model; the parser emits it for `&`-as-separator (trailing `&` keeps `Sequence.background`). The executor becomes group-aware: it partitions the flat list into and-or groups at `Semi`/`Amp` boundaries and runs each group foreground (existing `&&`/`||` logic) or background (wrap in a synthetic `Subshell` and reuse `run_background_subshell`). No `Sequence` AST restructure (deferred — see spec "Future work").

**Tech Stack:** Rust (binary crate `huck`). Unit `cargo test --bin huck`; integration `cargo test --test <name>`; bash-diff harness under `tests/scripts/`.

---

## File Structure

- `src/command.rs` — `Connector::Amp`; `&`-as-separator in the shared list loop + subshell body loop; relocate `UnexpectedBackground`.
- `src/executor.rs` — group-aware `execute_sequence_body` (partition + per-group dispatch); extract `run_andor_group`; reuse `run_background_subshell`/`run_background_sequence`; fold in `execute()`'s trailing-background path.
- `tests/async_list_integration.rs`, `tests/scripts/async_list_diff_check.sh` — NEW.
- `docs/bash-divergences.md`, `README.md` — new Tier-2 entry + deferred nested-AST note + changelog + README row.

---

### Task 1: `&` async list separator (parser + executor, end-to-end)

Coupled change; implement together. TDD: integration test first.

**Files:** `src/command.rs`, `src/executor.rs`

- [ ] **Step 1: Write the failing integration test**

Create `tests/async_list_integration.rs`. Use DETERMINISTIC ordering (background writers append to a file, then `wait`, then `sort`/`cat` — never rely on interleave timing):

```rust
//! v98: `&` async list separator (with and-or grouping).
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

fn run(script: &str) -> (String, i32) {
    let mut child = Command::new(huck_bin())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn huck");
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(), out.status.code().unwrap_or(-1))
}

#[test]
fn amp_separator_both_run() {
    // a & b : both run; serialize via wait + a marker so output is deterministic.
    assert_eq!(run("F=/tmp/huck_al1; : > $F\necho a >> $F &\nwait\necho b >> $F\nsort $F\n").0, "a\nb\n");
}

#[test]
fn amp_in_for_body() {
    assert_eq!(run("F=/tmp/huck_al2; : > $F\nfor i in 1 2 3; do echo $i >> $F & done\nwait\nsort $F\n").0,
               "1\n2\n3\n");
}

#[test]
fn amp_group_backgrounded_true_branch() {
    // `true && echo grouped &` : the group is backgrounded; grouped prints.
    assert_eq!(run("F=/tmp/huck_al3; : > $F\ntrue && echo grouped >> $F &\nwait\ncat $F\n").0, "grouped\n");
}

#[test]
fn amp_group_backgrounded_false_shortcircuit() {
    // `false && echo no &` : group backgrounded, && short-circuits, `no` does NOT print.
    assert_eq!(run("F=/tmp/huck_al4; : > $F\nfalse && echo no >> $F &\nwait\ncat $F\n").0, "");
}

#[test]
fn subshell_amp_backgrounds_left() {
    // ( a & b ): previously huck ran `a` foreground; now `a` is backgrounded. Both print.
    assert_eq!(run("F=/tmp/huck_al5; : > $F\n( echo a >> $F & wait; echo b >> $F )\nsort $F\n").0, "a\nb\n");
}

#[test]
fn bang_pid_set() {
    let (out, _rc) = run("sleep 0 &\ncase \"$!\" in [0-9]*) echo numeric;; *) echo bad;; esac\n");
    assert_eq!(out, "numeric\n");
}

#[test]
fn trailing_amp_status_zero() {
    assert_eq!(run("false &\necho $?\nwait\n").0, "0\n");
}

#[test]
fn regression_semi_and_or_unchanged() {
    assert_eq!(run("true && echo y\nfalse || echo n\necho a; echo b\n").0, "y\nn\na\nb\n");
}
```

Verify each expected output against bash first (`printf '...' | bash`) and adjust to bash's actual output.

- [ ] **Step 2: Run to confirm failure**

Run: `cargo test --test async_list_integration 2>&1 | tail -20`
Expected: FAIL — `&`-as-separator errors `'&' not allowed here`.

- [ ] **Step 3: Add `Connector::Amp`**

In `src/command.rs`, `enum Connector { Semi, And, Or }` → add `Amp`. Update any exhaustive `match Connector` the compiler flags AFTER Step 5 (the executor) — for now it won't compile until the executor arm exists; that's expected, proceed to Step 4-5 together.

- [ ] **Step 4: Parse `&`-as-separator**

**Top-level / shared list loop** (`src/command.rs:~648`, the `Token::Op(Operator::Background)` arm). Replace the current "trailing-only" logic:
```rust
            Token::Op(Operator::Background) => {
                skip_newlines(iter);
                match iter.peek() {
                    // Nothing meaningful follows -> trailing `&`: background the
                    // whole (final group of the) sequence, as today.
                    None => { background = true; break; }
                    Some(tok) if keyword_of(tok).map(|k| stop_at.contains(&k)).unwrap_or(false) => {
                        background = true; break;
                    }
                    Some(Token::Op(Operator::DoubleSemi | Operator::SemiAmp | Operator::DoubleSemiAmp)) => {
                        background = true; break;
                    }
                    // A command follows -> `&` is a separator: background the
                    // preceding group, continue the list.
                    Some(_) => { rest.push((Connector::Amp, parse_command(iter)?)); }
                }
            }
```
(Keep the existing heredoc-trailing-newline handling if relevant; the key change is the `Some(_) => push Amp` branch. Confirm the `at_top_level` gate is no longer needed to REJECT `&`-separators — compound bodies should also accept them. If `at_top_level` was the only thing blocking separators in bodies, remove that block; verify the shared loop is used by compound bodies via `stop_at`.)

**Subshell body loop** (`src/command.rs:~1482`): replace the fake-`Semi` push in the `&` branch with `rest.push((Connector::Amp, cmd))` (so `( a & b )` backgrounds `a`). The trailing-`&`-before-`)` case keeps `background=true`.

**`UnexpectedBackground`**: now only a leading/empty `&` (`& echo`) is invalid. Verify bash errors on `& echo` and keep an error there (or let it fall through to `UnexpectedToken`); remove `UnexpectedBackground` if fully unused, else keep for that case.

- [ ] **Step 5: Group-aware executor**

In `src/executor.rs`, extract the current per-element `&&`/`||` + `$?`/ERR/errexit loop from `execute_sequence_body` into a helper that runs ONE and-or group:
```rust
/// Runs an and-or group (a `first` command + `(And|Or, cmd)` rest, NO Semi/Amp)
/// foreground, with the existing &&/|| short-circuit + $?/ERR/errexit handling.
fn run_andor_group(first: &Command, rest: &[(Connector, &Command)], shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome
```
(Move the existing body of `execute_sequence_body` into this, operating on a group's commands. `rest` here contains only `And`/`Or` connectors.)

Rewrite `execute_sequence_body` to partition + dispatch:
```rust
fn execute_sequence_body(seq: &Sequence, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    // 1. Partition first+rest into groups at Semi/Amp boundaries.
    //    Each group: (first_cmd, Vec<(And|Or, &Command)>, backgrounded: bool).
    //    A group's `backgrounded` = its terminating separator is Amp; the LAST
    //    group's backgrounded = seq.background.
    // 2. Run each group in order:
    //    - backgrounded: wrap the group's commands into a synthetic
    //      `Sequence { first, rest (And/Or only), background: false }`, then a
    //      `Command::Subshell { body }`, and call run_background_subshell(...)
    //      (forks, registers job, sets $!, no wait). Status contribution: it does
    //      not change the list's foreground $? (background launch). Continue.
    //    - foreground: status = run_andor_group(...). Propagate Exit/LoopBreak/
    //      LoopContinue/FunctionReturn immediately (return). Update $? for the
    //      next group.
    // 3. Return the last foreground group's status (or Continue(0) if the list
    //    ended with a backgrounded group).
}
```
Implementation notes:
- Build groups as owned/borrowed slices over `seq.first`/`seq.rest`. A clean shape: iterate `seq.rest`, splitting whenever `connector ∈ {Semi, Amp}`; record the separator that closed each group to set `backgrounded`.
- For the background wrap, reuse the EXISTING `run_background_subshell(&Command::Subshell { body }, shell, sink, source)` path (the same one `execute()` uses for `(multi-cmd) &`). For a single-pipeline group you may reuse `run_background_sequence` to match today's single-`&` behavior — but the uniform subshell-wrap is acceptable and simpler; pick one and keep `$!`/job registration correct.
- `execute()`'s `if seq.background { … }` block (`src/executor.rs:~39`) is now redundant for the multi-group case — the trailing background is just "last group backgrounded." Simplify `execute()` to delegate to `execute_sequence_body` (which now handles trailing `&`), OR keep `execute()`'s fast-path for the pure single-command trailing-`&` case and let `execute_sequence_body` handle the rest — whichever keeps all existing background/job tests passing. Verify `$!`, `wait`, `jobs` still work.
- `set -e`/ERR semantics: unchanged for foreground groups (the logic moved into `run_andor_group`); a backgrounded group does not trigger parent `set -e` (it runs in a child).

- [ ] **Step 6: Build + parser unit tests + fix exhaustiveness**

`cargo build --bin huck`; fix any remaining non-exhaustive `match Connector` (e.g. in tests or other consumers) by handling `Amp`. Add parser unit tests: `a & b` → `rest=[(Amp, b)]`, `background=false`; `a && b &` → `rest=[(And, b)]`, `background=true`; `a & b &` → `rest=[(Amp, b)]`, `background=true`; trailing `a &` unchanged.

- [ ] **Step 7: Run integration + full suite + clippy**

Run: `cargo test --test async_list_integration 2>&1 | tail -20` (all pass).
Run: `cargo test --bin huck 2>&1 | tail -5` and `cargo test 2>&1 | grep -E 'test result' | grep -v 'ok\.' | head` (no failures — ESPECIALLY existing background/job/wait/subshell/pipeline/`set -e` tests).
Run: `cargo clippy --all-targets 2>&1 | tail -3` (clean).

- [ ] **Step 8: Commit**

```bash
git add src/command.rs src/executor.rs tests/async_list_integration.rs
git commit -m "feat: & async list separator with and-or group backgrounding

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```
Trailer mandatory/canonical, exactly as shown.

---

### Task 2: bash-diff harness (23rd)

**Files:** `tests/scripts/async_list_diff_check.sh` (NEW)

- [ ] **Step 1: Create the harness**

Mirror `tests/scripts/dbracket_multiline_diff_check.sh`'s `check` helper + a `mktemp -d` fixture. Use ONLY deterministic fragments (write-to-file + `wait` + `sort`/`cat`; `$?` after `&`; `&&` short-circuit inside a backgrounded group). NO timing-dependent interleaving:
```bash
FIX="$(mktemp -d)"; trap 'rm -rf "$FIX"' EXIT
check "amp sep"        ": > '$FIX/a'; echo x >> '$FIX/a' & wait; echo y >> '$FIX/a'; sort '$FIX/a'"
check "amp in for"     ": > '$FIX/b'; for i in 1 2 3; do echo \$i >> '$FIX/b' & done; wait; sort '$FIX/b'"
check "group bg true"  ": > '$FIX/c'; true && echo g >> '$FIX/c' &; wait; cat '$FIX/c'"
check "group bg false" ": > '$FIX/d'; false && echo no >> '$FIX/d' &; wait; cat '$FIX/d'"
check "trailing status" "false & echo \$?; wait"
check "semi/and/or"    "true && echo y; false || echo n; echo a; echo b"
check "brace amp"      ": > '$FIX/e'; { echo a >> '$FIX/e' & wait; echo b >> '$FIX/e'; }; sort '$FIX/e'"
```
NOTE: the `$!` value is NOT deterministic across shells — do NOT byte-compare `$!`; test it via the integration tests (numeric check) instead. After writing, RUN the script and confirm fragments are well-formed.

- [ ] **Step 2: Run the harness**

Run: `cargo build --bin huck && bash tests/scripts/async_list_diff_check.sh 2>&1 | tail -20`
Expected: every line PASS, `Fail: 0`. If a fragment FAILs, bash is the oracle — investigate; real Task 1 bug → STOP and report; timing-nondeterminism → make the fragment deterministic (more `wait`/`sort`) and note it.

- [ ] **Step 3: Commit**

```bash
git add tests/scripts/async_list_diff_check.sh
git commit -m "test: bash-diff harness for & async list separator (23rd)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```
Trailer mandatory/canonical, exactly as shown.

---

### Task 3: Documentation

**Files:** `docs/bash-divergences.md`, `README.md`

- [ ] **Step 1: Read structure**

`grep -n 'background\|async\|^## Change log\|Missing features (Tier 2)\|2026-06-05' docs/bash-divergences.md | head -20` and `grep -n '| v9' README.md`. Match v96/v97 style; next free `M-` number (highest is M-94).

- [ ] **Step 2: Add the Tier-2 entry**

New Tier-2 entry, next free `M-`, `[fixed v98]`: `&` async list separator — backgrounds the preceding and-or group and continues; supported at top level and in all compound bodies + subshells; `a && b &` backgrounds the whole group (bash-correct, via executor-side and-or grouping reusing the subshell-background path); fixes the prior `( a & b )` fake-`;` bug. Note the implementation keeps the flat `Sequence` model and **defer** the nested-and-or AST restructure as a separate low-priority follow-on (assign another `M-` number or a sub-note — per the spec "Future work"). Bump the Tier-2 count + roster narrative.

- [ ] **Step 3: Change-log + README row**

`2026-06-05` v98 change-log entry (Connector::Amp; group-aware executor; `( a & b )` fix; nvm.sh payoff — parses past 1192; the deferred nested-AST note; 23rd harness). v98 README row after v97.

- [ ] **Step 4: Verify + commit**

`grep -n 'v98\|fixed v98\|async\|Amp' docs/bash-divergences.md README.md` (confirm, no placeholders).
```bash
git add docs/bash-divergences.md README.md
git commit -m "docs: v98 & async list separator — Tier-2 entry, changelog, README, deferred-AST note

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```
Trailer mandatory/canonical, exactly as shown.

---

## Self-Review

- **Spec coverage:** §1 AST + §2 parser + §3 executor → Task 1; testing → Tasks 1/2; new Tier-2 entry + deferred-AST note → Task 3. Covered.
- **Placeholder scan:** none — the parser `&` arm + the `execute_sequence_body` partition contract are shown; the `run_andor_group` extraction is the EXISTING loop body moved verbatim (the implementer relocates it), and the background wrap reuses the named `run_background_subshell` path.
- **Type consistency:** `Connector::Amp`; `Sequence { first, rest: Vec<(Connector, Command)>, background }` unchanged; group dispatch reuses `run_background_subshell(&Command::Subshell { body }, …)`. `run_andor_group(first, rest, shell, sink) -> ExecOutcome`.
- **Edge cases:** trailing `&` (status 0, last group bg); `$!` set per bg group (tested via numeric check, not byte-diff); `( a & b )` semantics fix; `&` short-circuit inside a bg group (`false && x &` → x not run); `set -e` on bg groups exempt; non-`&` lists degenerate to today's behavior (every group foreground); command-substitution still ignores `&`.
