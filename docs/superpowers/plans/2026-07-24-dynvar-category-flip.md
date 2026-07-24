# v332 — Flip the `dynvar` bash-suite category Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the three dynamic variables the `dynvar` category needs — `BASH_ARGV0`, `EPOCHREALTIME`, `BASH_COMMAND` — taking dynvar.diff 15 → 0 (Summary PASS 20→21, FAIL 62→61).

**Architecture:** Two are computed reads (`EPOCHREALTIME` like `EPOCHSECONDS`; `BASH_ARGV0` ↔ `shell_argv0`, read + a write hook), all in `shell_state.rs`. `BASH_COMMAND` adds a `current_command` field stamped in the executor from the existing `render_job_simple` renderer. All prototype-verified byte-identical to bash and jointly flipping the category.

**Tech Stack:** Rust; huck-engine (`shell_state.rs`, `executor.rs`); bash-diff harness.

Spec: `docs/superpowers/specs/2026-07-24-dynvar-category-flip-design.md`
Issue: [#286](https://github.com/jdstanhope/huck/issues/286)

## Global Constraints

- bash 5.2.21 fidelity — byte-identical incl. stderr + exit:
  - `BASH_ARGV0=hello; echo "$0 $BASH_ARGV0"` → `hello hello`; assignment sets `$0` (also inside a function).
  - `$EPOCHREALTIME` = `<secs>.<6-digit-micros>` (assert the SHAPE, not the changing value).
  - `$BASH_COMMAND` = the raw source text of the command currently running (`echo $BASH_COMMAND` → `echo $BASH_COMMAND`), also correct when a DEBUG action reads it.
- These are computed dynamics (NOT stored in the vars table) — the #48 edge cases (`set`/`declare -p` listing, `[[ -v ]]`, assignment-shadow, inline-assign scoping) are OUT of scope and stay tracked in #48.
- Do NOT change the existing `EPOCHSECONDS`/`RANDOM`/`SECONDS`/`BASHPID` handlers or the DEBUG-fire machinery.
- Commit trailer `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`; `cargo fmt --all` before committing. Per repo memory: build with `cargo build -p huck`; per-crate tests single-threaded; NEVER `cargo test --workspace` (OOMs); guard sweeps with `ulimit -v 1500000` + `timeout`; run `-p huck` integration bins single-threaded before push; NO GPL bash text; no `Closes #N` in commits (bare `#N`).

---

### Task 1: `BASH_ARGV0` + `EPOCHREALTIME` (computed reads + BASH_ARGV0 write)

**Files:**
- Modify: `crates/huck-engine/src/shell_state.rs` (`lookup_var` reads ~1358; `reseed_special_on_assign` ~1470; `DYNAMIC_SPECIAL_VARS` ~1048)
- Create: `tests/scripts/dynvar_vars_diff_check.sh`

- [ ] **Step 1: Write the harness (red)**

Create `tests/scripts/dynvar_vars_diff_check.sh` (model on an existing `-c` bash-diff harness; a `check "label" 'frag'` comparing `bash --norc --noprofile -c` vs `"$HUCK_BIN" -c`, byte-identical stdout+stderr+`EXIT:$?`, with the huck binary path normalized to `bash`). Cases for this task:
```sh
check "argv0 assign"     'BASH_ARGV0=hello; echo "$0 $BASH_ARGV0"'         # hello hello
check "argv0 in fn"      'setarg0(){ BASH_ARGV0="$1"; }; setarg0 arg0; echo "$0"'   # arg0
check "epochrealtime fmt" '[[ $EPOCHREALTIME =~ ^[0-9]+\.[0-9]{6}$ ]] && echo shape-ok'  # shape-ok
check "epochrealtime pos" '(( ${EPOCHREALTIME%.*} > 0 )) && echo pos-ok'   # pos-ok
```
Build (`cargo build -p huck`) and run — these FAIL (BASH_ARGV0 no-op; EPOCHREALTIME empty → `[[ ]]`/`(( ))` diverge). Confirm each expected output against `bash --norc --noprofile` first.

- [ ] **Step 2: Add the computed reads**

In `shell_state.rs`, `lookup_var`'s special-name match, immediately after the `EPOCHSECONDS` arm (before `"BASHPID"`):
```rust
"EPOCHREALTIME" => {
    return Some(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| format!("{}.{:06}", d.as_secs(), d.subsec_micros()))
            .unwrap_or_else(|_| "0.000000".to_string()),
    );
}
"BASH_ARGV0" => return Some(self.shell_argv0.clone()),
```

- [ ] **Step 3: Add the `BASH_ARGV0` write hook**

In `reseed_special_on_assign`, before the `_ => false` arm (after the `SECONDS` arm):
```rust
// Assigning BASH_ARGV0 sets $0 (shell_argv0); it is computed in
// lookup_var, never stored as an ordinary var.
"BASH_ARGV0" => {
    self.shell_argv0 = value.to_string();
    true
}
```

- [ ] **Step 4: Register for completion**

Add `"EPOCHREALTIME"` and `"BASH_ARGV0"` to `DYNAMIC_SPECIAL_VARS` (after `"EPOCHSECONDS"`).

- [ ] **Step 5: Confirm the harness passes** (these 4 cases) byte-identical to bash.

- [ ] **Step 6: Regression**
```bash
cargo test -p huck-engine --lib --jobs 1 -- --test-threads 1   # green
ulimit -v 1500000; HUCK_BIN=./target/debug/huck bash tests/scripts/dynvar_vars_diff_check.sh && echo PASS
```

- [ ] **Step 7: fmt + commit**
```bash
cargo fmt --all
git add crates/huck-engine/src/shell_state.rs tests/scripts/dynvar_vars_diff_check.sh
git commit -m "$(cat <<'EOF'
v332: add BASH_ARGV0 (read/write $0) and EPOCHREALTIME dynamic vars (#286)

BASH_ARGV0 reads/writes $0 (shell_argv0); EPOCHREALTIME returns
<secs>.<6-digit-micros> (sibling of EPOCHSECONDS). Both computed in lookup_var,
registered for completion. Part of the dynvar bash-suite category flip.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: `BASH_COMMAND` + dynvar category flip

**Files:**
- Modify: `crates/huck-engine/src/shell_state.rs` (`current_command` field + init; `lookup_var` read; `DYNAMIC_SPECIAL_VARS`)
- Modify: `crates/huck-engine/src/executor.rs` (`run_single` stamp)
- Modify: `tests/scripts/dynvar_vars_diff_check.sh` (add BASH_COMMAND cases)

- [ ] **Step 1: Add the harness cases (red)**

Append to `tests/scripts/dynvar_vars_diff_check.sh`:
```sh
check "bashcmd simple"   'echo $BASH_COMMAND'                          # echo $BASH_COMMAND
check "bashcmd in fn"    'f(){ echo $BASH_COMMAND; }; f'               # echo $BASH_COMMAND
check "bashcmd after asn" 'x=1; echo $BASH_COMMAND'                    # echo $BASH_COMMAND
check "bashcmd in debug"  'set -T; trap "echo D:\$BASH_COMMAND" DEBUG; :; true'  # match bash
```
Run — BASH_COMMAND cases FAIL (huck expands it to empty). Confirm expected output against bash first (note the DEBUG case: bash prints `D:<cmd>` for each traced command — match bash exactly, whatever it emits).

- [ ] **Step 2: Add the `current_command` field**

In `shell_state.rs`, the `Shell` struct, right after `pub current_lineno: u32,`:
```rust
/// `$BASH_COMMAND`: the source text of the simple command currently being
/// executed (or about to be, when a DEBUG trap reads it). Stamped by the
/// executor before each command runs.
pub current_command: String,
```
And in `Shell::new()`'s initializer, after `current_lineno: 0,`:
```rust
current_command: String::new(),
```

- [ ] **Step 3: Add the read + registration**

In `lookup_var`, after the `"BASH_ARGV0" =>` arm:
```rust
"BASH_COMMAND" => return Some(self.current_command.clone()),
```
Add `"BASH_COMMAND"` to `DYNAMIC_SPECIAL_VARS` (after `"BASH_ARGV0"`).

- [ ] **Step 4: Stamp `current_command` in the executor**

In `crates/huck-engine/src/executor.rs`, `run_single`, as the FIRST statement (before `let outcome = match cmd {`):
```rust
// $BASH_COMMAND: the source text of the command about to run — stamped
// before the DEBUG trap fires (bash sets it at the same point, so a DEBUG
// action reading $BASH_COMMAND sees the command that triggered it).
shell.current_command = render_job_simple(cmd);
```
`render_job_simple(cmd: &SimpleCommand)` already exists in executor.rs (it renders `inline_assignments + program + args + redirects` via `reconstruct_word_source`). `run_single` runs before the DEBUG fire in both the Exec and Assign paths.

- [ ] **Step 5: Confirm the harness passes** — all cases (Task 1 + BASH_COMMAND + the DEBUG case) byte-identical to bash.

- [ ] **Step 6: `dynvar` category flips + no DEBUG regression**
```bash
cargo build --release -p huck
for c in dynvar dbg-support2; do
  HUCK_BASH_TEST_CATEGORY=$c HUCK_TEST_TIMEOUT=90 BASH_SOURCE_DIR=/tmp/bash-5.2.21 \
    timeout 200 bash tests/bash-test-suite/runner.sh > /tmp/v332_$c.md 2>&1
  grep -iE "\| $c \|" /tmp/v332_$c.md
  sc=$(grep -oE "/tmp/huck-bash-tests[^ ]*" /tmp/v332_$c.md | head -1)
  echo "$c.diff: $(wc -l < $sc/$c.diff) lines"
done
```
Expect: `dynvar` PASS 0-diff; `dbg-support2` PASS 0-diff (DEBUG actions now read a non-empty `$BASH_COMMAND` — MUST stay PASS).

- [ ] **Step 7: Broad regression**
```bash
cargo test -p huck-engine --lib --jobs 1 -- --test-threads 1   # green
# BASH_COMMAND stamps in the hot run_single path — run the trace/trap/var integration bins:
for t in xtrace_integration trap_integration trap_pseudo_signals_integration funcname functions_integration; do
  cargo test -p huck --test "$t" --jobs 1 -- --test-threads 1 2>&1 | grep "test result" || echo "(no bin: $t)"
done
ulimit -v 1500000; timeout 550 bash tests/scripts/run_diff_checks.sh   # green (coproc flake pre-existing)
# previously-flipped categories hold:
for c in parser rhs-exp procsub posix2; do
  HUCK_BASH_TEST_CATEGORY=$c HUCK_TEST_TIMEOUT=90 BASH_SOURCE_DIR=/tmp/bash-5.2.21 \
    timeout 200 bash tests/bash-test-suite/runner.sh 2>&1 | grep -iE "$c \|"
done
```
(Enumerate the actual `-p huck` integration bin names from `tests/` — `--lib` does not run them; a var/trace/trap bin failure would be missed otherwise, per the v331/#284 lesson.)

- [ ] **Step 8: Docs + memory (part of the branch)**
  - `docs/bash-test-suite-baseline.md`: prepend "Updated by v332 (#286, 2026-07-24 UTC): `dynvar` flipped to PASS (0-diff). Summary PASS 20→21, FAIL 62→61."
  - `project_huck_iterations.md` + `MEMORY.md`: record v332 (dynvar flip; the three dynamic vars; BASH_COMMAND also helps dbg-support; durable lesson — prototyping surfaced the hidden 3rd root, BASH_COMMAND, before speccing).

- [ ] **Step 9: fmt + commit**
```bash
cargo fmt --all
git add crates/huck-engine/src/shell_state.rs crates/huck-engine/src/executor.rs tests/scripts/dynvar_vars_diff_check.sh docs/bash-test-suite-baseline.md
git commit -m "$(cat <<'EOF'
v332: add $BASH_COMMAND; flips dynvar to PASS (#286)

Track the currently-executing simple command's source text in a new
current_command field, stamped in run_single before the DEBUG fire (from the
existing render_job_simple renderer), and expand $BASH_COMMAND to it. Completes
the dynvar bash-suite category flip (15 -> 0 diff, Summary PASS 20->21).
dbg-support2 stays PASS (DEBUG actions now read the correct $BASH_COMMAND).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```
(Memory files live outside the repo — update in the same session, not this commit.)

---

## Self-Review

- **Spec coverage:** BASH_ARGV0 + EPOCHREALTIME (Task 1); BASH_COMMAND field/read/stamp (Task 2); harness (Task 1, extended Task 2); category flip + regressions (Task 2). All three spec vars map to a task.
- **Placeholders:** none — exact code for every edit. The `-p huck` integration-bin names in Task 2 Step 7 are flagged best-effort (enumerate the real names from `tests/`).
- **Type consistency:** `reseed_special_on_assign(name, value: &str) -> bool`; `render_job_simple(&SimpleCommand) -> String`; `current_command: String`; `shell_argv0: String`.
- **Scope:** three dynamic vars only; #48 computed-var edges and broader per-construct BASH_COMMAND stamping explicitly out of scope. The review must confirm `dbg-support2` stays PASS (the one real cross-cutting risk of the BASH_COMMAND stamp).
