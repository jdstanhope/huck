# v312 — `$(( ))` arithmetic expansion error discards the current command — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** an arithmetic `$(( ))` expansion error discards the current top-level command (unwinds out of loops/functions, rc 1) without exiting the shell, matching bash — for command words, assignment RHS, and array subscripts.

**Architecture:** Reuse huck's existing `ExecOutcome::Interrupted` unwind channel (which propagates reason-generically through loops/functions/lists) with a new `InterruptReason::FatalExpansion`, triggered by a new `pending_discard` shell flag set at the arith-expansion error sites and converted where `pending_fatal_status` is already converted. Contained at the comsub boundary; the driver loop continues (doesn't exit) on it.

**Tech Stack:** Rust, the `ExecOutcome`/`InterruptReason` model, the `pending_fatal_status` pattern as the exact template, a bash-diff harness.

**Spec:** `docs/superpowers/specs/2026-07-18-arith-expansion-discard-design.md` — read it first (has the measured bash table and the 8 handling points).

**Issues:** [#3](https://github.com/jdstanhope/huck/issues/3), [#49](https://github.com/jdstanhope/huck/issues/49) (same root); [#198](https://github.com/jdstanhope/huck/issues/198) (funnel umbrella).

## Global Constraints

- **Branch:** `v312-arith-discard`. Never commit to `main`; never merge.
- **Commit trailer**, exactly: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- **`cargo fmt --all`** before committing — CI enforces `cargo fmt --all --check`.
- **⚠️ NEVER run `cargo test --workspace` or a bare `cargo test`** — 1 core / 1.9 GB box; it OOM-kills the session. Per-crate: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`. **Also run the `-p huck` integration binaries you might affect at `--test-threads 2`** (a concurrency/flake lesson: `--test-threads 1` hides races; and CI runs the full `-p huck` suite).
- **⚠️ The `grep` command is BROKEN** here — use `/usr/bin/grep`.
- **`pending_fatal_status` is the template.** It is the sibling "exit-shell" fatality flag; the new `pending_discard` is the "discard-current-command" flavor. Mirror the template's structure everywhere, but decode it differently (unwind + continue, not exit). Do NOT change any existing `pending_fatal_status` behavior.
- **Scope: only `$(( ))` EXPANSION.** The `(( ))` command, `for ((;;))`, and `let` route through `run_arith`/`eval_arith_word` (a different path) and must stay non-fatal — the harness pins this.
- **Exit status is 1** for every discard (bash's arith-expansion rc).
- **Error-message wording is OUT OF SCOPE** — the harness normalizes the diagnostic line.

---

### Task 1: the discard mechanism + harness

**Files:**
- Create: `tests/scripts/arith_expansion_discard_diff_check.sh`
- Modify: `crates/huck-engine/src/builtins.rs` (`InterruptReason` enum ~line 12; the driver `'outer` loop ~line 182), `crates/huck-engine/src/shell_state.rs` (the `pending_discard` field + accessor; the `Interrupted` decode ~294 lives in `shell.rs`), `crates/huck-engine/src/shell.rs` (reducer ~294), `crates/huck-engine/src/expand.rs` (trigger sites 1206, ~1817), `crates/huck-engine/src/executor.rs` (arg-collection skip 3435/3480/7820, case 2312, the and-or conversion 436/479, comsub containment 372, reducer ~8058)

**Interfaces:**
- Produces (used only within this task): `InterruptReason::FatalExpansion`; `Shell::pending_discard: bool` + `Shell::take_pending_discard(&mut self) -> bool`.

- [ ] **Step 1: Write the failing harness**

Create `tests/scripts/arith_expansion_discard_diff_check.sh`. It compares **which commands run + the rc**, normalizing the arithmetic error-message line (out of scope) so only fatality/ordering is judged.

```bash
#!/usr/bin/env bash
# v312 (#3/#49): a `$(( ))` arithmetic EXPANSION error discards the current
# top-level command (unwinds out of loops/functions, rc 1) WITHOUT exiting the
# shell — bash's jump_to_top_level(DISCARD). huck swallowed it (ran the command
# with an empty value, rc 0). Distinct from set -u/${x?} (which EXIT the shell).
# A comsub boundary CONTAINS the discard (outer command continues).
#
# The arithmetic ERROR MESSAGE wording is out of scope (#60): each shell's
# arith-error diagnostic line is normalized to `ARITH_ERR` so only the abort
# behavior (which commands run) and the rc are compared.
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: huck binary not found at $HUCK (build with: cargo build -p huck)" >&2; exit 1; }

FAIL=0
# Normalize each shell's arith-error diagnostic line (path/prog prefix + wording)
# to a fixed token; everything else (command output, ordering) is compared raw.
norm() {
  sed -E -e 's#^(bash|.*/huck): line [0-9]+: .*(arithmetic|unexpected character|division by 0|operand expected|syntax error).*#ARITH_ERR#' \
         -e 's#^(bash|.*/huck): .*(arithmetic|unexpected character|division by 0|operand expected).*#ARITH_ERR#'
}
check() {  # compares merged stdout+stderr (normalized) AND rc
  local label=$1 frag=$2 b h br hr
  b=$(bash -c "$frag" 2>&1 | norm); br=${PIPESTATUS[0]}
  h=$("$HUCK" -c "$frag" 2>&1 | norm); hr=${PIPESTATUS[0]}
  if [ "$b" != "$h" ] || [ "$br" != "$hr" ]; then
    echo "FAIL [$label]"; echo "  bash(rc=$br): [$b]"; echo "  huck(rc=$hr): [$h]"; FAIL=1
  else echo "PASS [$label]"; fi
}
check_script() {  # same, but runs a multi-line SCRIPT FILE (tests non-exit)
  local label=$1; shift
  local f; f=$(mktemp); printf '%s\n' "$@" > "$f"
  local b h br hr
  b=$(bash "$f" 2>&1 | norm); br=${PIPESTATUS[0]}
  h=$("$HUCK" "$f" 2>&1 | norm); hr=${PIPESTATUS[0]}
  rm -f "$f"
  if [ "$b" != "$h" ] || [ "$br" != "$hr" ]; then
    echo "FAIL [$label]"; echo "  bash(rc=$br): [$b]"; echo "  huck(rc=$hr): [$h]"; FAIL=1
  else echo "PASS [$label]"; fi
}

# --- The fix (red->green): discard the current command.
check 'cmd-word'       'echo $((3.5)); echo done'
check 'before-after'   'echo BEFORE; echo $((3.5)); echo AFTER'
check 'bare-arith'     '$((3.5)); echo done'
check 'div-by-zero'    'echo $((1/0)); echo done'
check 'assignment'     'x=$((3.5)); echo AFTER'
check 'subscript'      'a[$((3.5))]=1; echo AFTER'
check 'loop-unwind'    'for i in 1 2 3; do echo i$i; echo $((3.5)); echo t$i; done; echo END'
check 'func-unwind'    'f(){ echo in; echo $((3.5)); echo after_in; }; f; echo AFTER_F'

# --- Multi-line SCRIPT: the discard must NOT exit the shell (later lines run).
check_script 'script-continues' 'echo $((3.5))' 'echo L2' 'echo L3'

# --- Comsub boundary CONTAINS the discard (outer command continues).
check 'comsub-contained' 'x=$( echo $((3.5)) ); echo "[$x] after"'
check 'comsub-inline'    'echo pre $( echo $((3.5)) ) post; echo NEXT'

# --- set -e: a discarded rc-1 command aborts like any rc-1 command.
check 'set-e'          'set -e; echo $((3.5)); echo done'

# --- Controls: must stay NON-fatal (different code path).
check 'arith-cmd'      '(( 3.5 )); echo done'
check 'cstyle-for'     'for ((i=3.5; i<1; i++)); do :; done; echo done'
check 'let-builtin'    'let "3.5"; echo done'
check 'valid-arith'    'echo $((1+1)); echo done'

if [ $FAIL -ne 0 ]; then echo "arith_expansion_discard_diff_check FAILED" >&2; exit 1; fi
echo "arith_expansion_discard_diff_check OK"
```

- [ ] **Step 2: Run the harness — verify the discard cases FAIL, controls PASS**

Run: `cargo build -q -p huck && bash tests/scripts/arith_expansion_discard_diff_check.sh`
Expected: the discard cases (`cmd-word`, `before-after`, `bare-arith`, `div-by-zero`, `assignment`, `subscript`, `loop-unwind`, `func-unwind`, `set-e`) FAIL; `script-continues`, `comsub-contained`, `comsub-inline` may FAIL or PASS depending on current behavior; the controls (`arith-cmd`, `cstyle-for`, `let-builtin`, `valid-arith`) PASS. Overall FAILED. This is the RED gate. If a CONTROL fails, stop — the harness is wrong.

- [ ] **Step 3: Add `InterruptReason::FatalExpansion`**

In `crates/huck-engine/src/builtins.rs`, extend the enum (~line 12):

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptReason {
    Sigint,
    Timeout,
    /// v312 (#3/#49): a fatal arithmetic EXPANSION error (`$(( ))` syntax /
    /// division). Bash's `jump_to_top_level(DISCARD)`: unwind the current
    /// top-level command (out of loops/functions), status 1, but DO NOT exit
    /// the shell. A SYNCHRONOUS discard, not a signal — it reuses the
    /// `Interrupted` unwind channel because the propagation is identical; only
    /// the boundary/decoder sites treat it differently (contain at a comsub;
    /// continue at the driver loop).
    FatalExpansion,
}
```

- [ ] **Step 4: Add the `pending_discard` flag + accessor**

In `crates/huck-engine/src/shell_state.rs`: add the field next to `pending_fatal_status` (~line 744) and initialize it `false` (~line 1075):

```rust
    /// v312: a fatal arithmetic expansion error is pending — the current command
    /// must be DISCARDED (converted to `Interrupted(FatalExpansion)`). Sibling of
    /// `pending_fatal_status` but the DISCARD flavor (not exit-shell).
    pub pending_discard: bool,
```

Add the accessor next to `take_pending_fatal_status` (~line 3005):

```rust
    /// Returns and clears the pending arithmetic-discard flag.
    pub fn take_pending_discard(&mut self) -> bool {
        std::mem::take(&mut self.pending_discard)
    }
```

- [ ] **Step 5: Trigger at both `$(( ))` expansion-error sites**

`expand.rs:1206` — replace `shell.posix_fatal(127);` with:

```rust
                    // v312 (#3/#49): a `$(( ))` expansion error DISCARDS the
                    // current command (bash jump_to_top_level(DISCARD)), rc 1,
                    // without exiting the shell. Converted to
                    // Interrupted(FatalExpansion) at the executor's post-expansion
                    // check points (mirrors pending_fatal_status).
                    shell.pending_discard = true;
```

`expand.rs:~1817` — the assignment-RHS arith-error site currently only prints. After the `sh_error!` there, add `shell.pending_discard = true;` (same rationale).

(Update the stale "NON-fatal (v215)" / "-c mode divergence L-55" comments at both sites to say the discard is now raised.)

- [ ] **Step 6: Skip the command + convert to the unwind outcome**

Mirror `pending_fatal_status` at its post-expansion check points, adding `pending_discard`:

- **Arg-collection skip** (`executor.rs:3435`, `:3480`, and the subscript `:7820`, and the `case` subject `:2312`): these currently `return Err(...)` / `return Continue(status)` when `pending_fatal_status` is set. Extend each condition so `pending_discard` ALSO skips the command from running (the command must not execute).
- **Conversion to the outcome** (`executor.rs:436` and `:479`, the and-or `Continue`-status blocks): BEFORE the existing `if shell.pending_fatal_status.is_some()` check, add:

```rust
        if shell.take_pending_discard() {
            return ExecOutcome::Interrupted(InterruptReason::FatalExpansion);
        }
```

  and mirror at the `case`-clause point (`:2316`) and any other place `pending_fatal_status` is decoded that a `$(( ))` can reach (assignment path especially — driven by the harness's `assignment`/`subscript` rows).

**The harness is the completeness gate for this step:** add a `pending_discard`
conversion wherever a `$(( ))` context (command word, bare arith, assignment,
subscript) can set the flag, until every discard row in Step 8 goes green. Use
`/usr/bin/grep -n 'pending_fatal_status' crates/huck-engine/src/executor.rs` to
find the sibling sites; each is a candidate.

- [ ] **Step 7: Contain at the comsub boundary; continue at the driver; decode at reducers**

- **Comsub containment** (`execute_capturing`, `executor.rs:372`): the current `Interrupted(reason)` arm re-raises `sigint_flag` so the enclosing list aborts. Add a `FatalExpansion` arm that returns **1 WITHOUT re-raising** — the comsub is contained; the outer command continues:

```rust
                InterruptReason::FatalExpansion => 1,
```

- **Driver loop** (`run_sourced_contents_in_sinks`, `builtins.rs:~182`): the `'outer` loop does `ExecOutcome::Interrupted(r) => return ExecOutcome::Interrupted(r)`. Change so `FatalExpansion` sets the unit's last status to 1 and **continues** the loop (does NOT return/exit); `Sigint`/`Timeout` keep returning:

```rust
                        ExecOutcome::Interrupted(InterruptReason::FatalExpansion) => {
                            last_status = 1;
                            // discard THIS unit; keep reading the next (no shell exit)
                        }
                        ExecOutcome::Interrupted(r) => return ExecOutcome::Interrupted(r),
```

(Confirm the exact `last_status` variable name in that function; set it to 1 and fall through to the next loop iteration.)

- **Reducers** (`executor.rs:~8058` and `shell.rs:294`): where `Interrupted(Sigint) => 130` / `Timeout => 124` decode a final code, add `Interrupted(FatalExpansion) => 1` (defensive — the driver normally handles it, but a `FatalExpansion` reaching a top-level reducer must decode to 1, not panic on a non-exhaustive match).

- [ ] **Step 8: Run the harness — verify GREEN**

Run: `cargo build -q -p huck && bash tests/scripts/arith_expansion_discard_diff_check.sh`
Expected: all `PASS`, `arith_expansion_discard_diff_check OK`. In particular `script-continues` (later lines run), `comsub-contained`/`comsub-inline` (outer continues), `loop-unwind`/`func-unwind` (unwind), and every control staying non-fatal.

- [ ] **Step 9: POSIX-mode check**

Run: `bash --posix -c 'echo $((3.5)); echo done'; echo "bash-posix rc=$?"` and the same with `target/debug/huck --posix` (if huck supports a `--posix` flag; else `set -o posix` inside). If bash `--posix` EXITS (rc 1, no "done" — same as default here) then the unconditional discard is correct. If bash `--posix` differs (e.g. exits with a different code), add a POSIX branch at the trigger sites that sets `pending_fatal_status` instead. Report what bash `--posix` does.

- [ ] **Step 10: No regression — arithmetic + set-e + lib tests**

Run:
```bash
for h in arith_error arith_integration set_e_andor bang_negation; do
  [ -f tests/scripts/${h}_diff_check.sh ] && bash tests/scripts/${h}_diff_check.sh
done
cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1
ulimit -v 6000000; cargo test -p huck --test arith_integration --jobs 1 -- --test-threads 2
```
Expected: each present harness `… OK`; lib tests pass; the `arith_integration` binary passes at 2 threads. (If a harness name doesn't exist, skip it.)

- [ ] **Step 11: Commit**

```bash
cargo fmt --all
git add crates/huck-engine/src/builtins.rs crates/huck-engine/src/shell_state.rs crates/huck-engine/src/shell.rs crates/huck-engine/src/expand.rs crates/huck-engine/src/executor.rs tests/scripts/arith_expansion_discard_diff_check.sh
git commit -m "$(cat <<'EOF'
fix: a $(( )) arithmetic expansion error discards the current command (#3 #49)

huck swallowed a `$(( ))` expansion error — expanded to empty, ran the command,
rc 0. bash discards the current top-level command (unwinds out of loops and
functions, rc 1) but does NOT exit the shell (a later script line runs) — its
jump_to_top_level(DISCARD), distinct from set -u/${x?}'s EXITPROG.

Adds InterruptReason::FatalExpansion + a pending_discard flag: the arith-error
sites raise the flag, the executor converts it (where pending_fatal_status is
converted) to Interrupted(FatalExpansion), which the existing reason-generic
unwind channel propagates through loops/functions/lists. Contained at the comsub
boundary (outer command continues); the driver loop continues on it (no shell
exit). Only `$(( ))` expansion — the `(( ))` command / for((;;)) / let stay
non-fatal. Second member of the #198 error-fatality funnel (the DISCARD flavor).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Verification (controller, before the PR)

- [ ] `cargo fmt --all --check` — clean.
- [ ] `cargo build -p huck --locked` and `cargo build --release -p huck --locked`.
- [ ] `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1` — pass.
- [ ] `-p huck` integration binaries most likely affected, each at `--test-threads 2` with a `ulimit -v` guard: `arith_integration`, `subshell_integration`, `eval_integration`, `trap_integration`, `special_params_integration` (the last caught a flake in v311 — run it).
- [ ] `tests/scripts/run_diff_checks.sh` on both binaries — green (the new harness is picked up automatically; known flake `pipeline_stage_redirect_fail_diff_check.sh` case `amb-stdin-mid`, [#180](https://github.com/jdstanhope/huck/issues/180)).
- [ ] Non-exhaustive-match check: `cargo build -p huck-engine 2>&1 | /usr/bin/grep -c 'non-exhaustive\|match'` — 0 (every `InterruptReason` match handles `FatalExpansion`).
- [ ] PR with `Closes #3` and `Closes #49`; **the user merges, not you.** Wait for CI to finish and pass before saying it is ready.
