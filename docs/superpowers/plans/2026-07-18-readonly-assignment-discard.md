# v313 — readonly-assignment error discards the current command — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** a standalone readonly-variable assignment error (`readonly r=1; r=2; echo done`) discards the current command (rc 1, `done` not run) in default non-interactive mode — matching bash — without exiting the shell.

**Architecture:** Reuse v312's DISCARD mechanism (`pending_discard` → `Interrupted(DiscardCommand)` unwind) at `run_assignment_list`'s readonly-error sites, and rename the v312 reason `FatalExpansion` → `DiscardCommand` (it now has two users). Default mode discards; POSIX + non-interactive keeps v226's exit-127.

**Tech Stack:** Rust, the `pending_discard`/`Interrupted` DISCARD machinery from v312, a bash-diff harness.

**Spec:** `docs/superpowers/specs/2026-07-18-readonly-assignment-discard-design.md` — read it first (measured bash table + scope boundaries).

**Issues:** [#31](https://github.com/jdstanhope/huck/issues/31); [#198](https://github.com/jdstanhope/huck/issues/198) (funnel umbrella).

## Global Constraints

- **Branch:** `v313-readonly-discard`. Never commit to `main`; never merge.
- **Commit trailer**, exactly: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- **`cargo fmt --all`** before committing.
- **⚠️ NEVER run `cargo test --workspace` or a bare `cargo test`** — 1 core / 1.9 GB box; it OOM-kills the session. Per-crate: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`. **Run affected `-p huck` integration binaries at `--test-threads 2`** (concurrency-race lesson; CI runs the full suite).
- **⚠️ The `grep` command is BROKEN** here — use `/usr/bin/grep`.
- **DISCARD, not EXITPROG.** Default mode sets `pending_discard = true` (discard current command, rc 1, shell NOT exited). Only `posix && !is_interactive` exits (127). Use the v312 guard exactly.
- **Rename must be behavior-neutral** — the v312 arith harness (`arith_expansion_discard_diff_check.sh`) must stay green after the rename.
- **Scope: only `run_assignment_list`** (standalone assignment statements). Do NOT touch the for-loop/select var binds (`executor.rs:1828`, `:2237`), inline-prefix, or `unset` — those already match bash.

---

### Task 1: rename to DiscardCommand + route readonly assignment through DISCARD + harness

**Files:**
- Create: `tests/scripts/readonly_assign_discard_diff_check.sh`
- Modify (rename, 13 sites): `crates/huck-engine/src/builtins.rs`, `crates/huck-engine/src/shell_state.rs`, `crates/huck-engine/src/shell.rs`, `crates/huck-engine/src/expand.rs`, `crates/huck-engine/src/executor.rs`, `crates/huck-cli/src/repl.rs`
- Modify (fix): `crates/huck-engine/src/executor.rs` (`run_assignment_list` ~4105)

**Interfaces:**
- Consumes: `Shell::pending_discard: bool`, `InterruptReason::DiscardCommand` (renamed this task), the existing `run_andor_group` `Continue`-backstop conversion.

- [ ] **Step 1: Write the failing harness**

Create `tests/scripts/readonly_assign_discard_diff_check.sh`:

```bash
#!/usr/bin/env bash
# v313 (#31): a standalone readonly-variable ASSIGNMENT error DISCARDS the
# current top-level command (bash jump_to_top_level(DISCARD)) — rc 1, unwinds
# out of loops/functions, but does NOT exit the shell (a later script line runs).
# Same DISCARD flavor as #3 (arith). huck used to print the error and CONTINUE
# (rc 0). --posix mode EXITS (127, v226). Inline-prefix / unset / for-var stay
# non-fatal.
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: huck binary not found at $HUCK (build with: cargo build -p huck)" >&2; exit 1; }

FAIL=0
# The readonly message already matches bash; normalize only each shell's own
# prefix (`bash: line N:` / `<huckpath>: line N:`) so the rest compares raw.
norm() { sed -E "s#^(bash|.*/huck): line [0-9]+: #SH: #"; }
check() {
  local label=$1 frag=$2 b h br hr
  b=$(bash -c "$frag" 2>&1 | norm); br=${PIPESTATUS[0]}
  h=$("$HUCK" -c "$frag" 2>&1 | norm); hr=${PIPESTATUS[0]}
  if [ "$b" != "$h" ] || [ "$br" != "$hr" ]; then
    echo "FAIL [$label]"; echo "  bash(rc=$br): [$b]"; echo "  huck(rc=$hr): [$h]"; FAIL=1
  else echo "PASS [$label]"; fi
}
check_script() {
  local label=$1; shift; local f; f=$(mktemp); printf '%s\n' "$@" > "$f"
  local b h br hr
  b=$(bash "$f" 2>&1 | sed -E "s#^.*/[^:]+: line [0-9]+: #SH: #"); br=${PIPESTATUS[0]}
  h=$("$HUCK" "$f" 2>&1 | sed -E "s#^.*/[^:]+: line [0-9]+: #SH: #"); hr=${PIPESTATUS[0]}
  rm -f "$f"
  if [ "$b" != "$h" ] || [ "$br" != "$hr" ]; then
    echo "FAIL [$label]"; echo "  bash(rc=$br): [$b]"; echo "  huck(rc=$hr): [$h]"; FAIL=1
  else echo "PASS [$label]"; fi
}

# --- The fix (red->green): standalone readonly assignment discards.
check 'readonly-assign'   'readonly r=1; r=2; echo done'
check 'uid'               'UID=5; echo done'
check 'bash-versinfo'     'BASH_VERSINFO[0]=9; echo done'
check 'assign-list'       'readonly r=1; a=1 r=2 b=3; echo x'
check 'before-after'      'echo B; r=1; readonly r; r=2; echo A'
check 'loop-unwind'       'readonly r=1; for i in 1 2 3; do echo i$i; r=2; echo t$i; done; echo END'
check 'func-unwind'       'readonly r=1; f(){ echo in; r=2; echo after_in; }; f; echo AF'

# --- Multi-line SCRIPT: discard must NOT exit the shell (later lines run).
check_script 'script-continues' 'readonly r=1' 'r=2' 'echo L2' 'echo L3'

# --- Controls: stay non-fatal / already-correct.
check 'inline-prefix'     'readonly r=1; r=2 echo RAN; echo done'
check 'unset-readonly'    'readonly r=1; unset r; echo done'
check 'for-var-readonly'  'readonly r=1; for r in a b; do echo $r; done; echo END'
check 'good-assign'       'x=1; echo $x done'

if [ $FAIL -ne 0 ]; then echo "readonly_assign_discard_diff_check FAILED" >&2; exit 1; fi
echo "readonly_assign_discard_diff_check OK"
```

- [ ] **Step 2: Run the harness — verify the discard cases FAIL, controls PASS**

Run: `cargo build -q -p huck && bash tests/scripts/readonly_assign_discard_diff_check.sh`
Expected: `readonly-assign`, `uid`, `bash-versinfo`, `assign-list`, `before-after`, `loop-unwind`, `func-unwind` FAIL (huck continues, rc 0); `script-continues` may pass (both continue) or fail; the controls (`inline-prefix`, `unset-readonly`, `for-var-readonly`, `good-assign`) PASS. Overall FAILED. If a control FAILS, stop and report.

- [ ] **Step 3: Rename `InterruptReason::FatalExpansion` → `DiscardCommand` (behavior-neutral)**

Rename every occurrence (13 total) across these files. Use `/usr/bin/grep -rn 'FatalExpansion' crates/` to locate them; rename the identifier AND the comment text (`Interrupted(FatalExpansion)` → `Interrupted(DiscardCommand)`):
- `crates/huck-engine/src/builtins.rs` — the enum variant (line ~22) and the driver `'outer` arm (~7705).
- `crates/huck-engine/src/shell_state.rs` — the `pending_discard` doc comment (~746).
- `crates/huck-engine/src/shell.rs` — the reducer arm (~300).
- `crates/huck-engine/src/expand.rs` — the trigger-site comment (~1205).
- `crates/huck-engine/src/executor.rs` — the comsub arm (~402), the two and-or backstop conversions (~449, ~497), the case-clause conversion (~2339), the comment (~3469), the fork-child reducer (~8104).
- `crates/huck-cli/src/repl.rs` — the interactive arm (~259).

Update the enum variant's doc comment to describe it generally (both users):

```rust
    /// v312/v313: a fatal error that DISCARDS the current top-level command
    /// (bash `jump_to_top_level(DISCARD)`) — unwind out of loops/functions,
    /// status 1, but the shell is NOT exited. Raised by a fatal `$(( ))`
    /// expansion error (#3) and a readonly-variable assignment error (#31);
    /// contained at execution boundaries; the driver loop continues on it.
    DiscardCommand,
```

- [ ] **Step 4: Verify the rename is behavior-neutral**

Run: `cargo build -q -p huck && bash tests/scripts/arith_expansion_discard_diff_check.sh`
Expected: `arith_expansion_discard_diff_check OK` — the v312 arith behavior is unchanged by the rename. (If it fails to compile, a `FatalExpansion` occurrence was missed — `/usr/bin/grep -rn 'FatalExpansion' crates/` must return nothing.)

- [ ] **Step 5: Route the readonly-assignment error through DISCARD**

In `run_assignment_list` (`crates/huck-engine/src/executor.rs` ~4105), there are TWO `shell.posix_fatal(127);` calls — the pre-check readonly site (~4123) and the `apply_one_assignment(...).is_err()` site (~4128). Replace EACH `shell.posix_fatal(127);` with the v312 discard/posix pattern:

```rust
            if shell.shell_options.posix && !shell.is_interactive {
                shell.posix_fatal(127); // EXITPROG (v226): POSIX non-interactive exits 127
            } else {
                shell.pending_discard = true; // DISCARD (v312/#31): discard the current command, rc 1
            }
```

Leave the surrounding `st = 1; break;` exactly as-is — `run_assignment_list` returns `Continue(1)`, and the enclosing `run_andor_group` `Continue`-backstop converts `pending_discard` to `Interrupted(DiscardCommand)` (the identical path as the `$(( ))` fix). Do NOT return `Interrupted` directly from here.

- [ ] **Step 6: Run the harness — verify GREEN**

Run: `cargo build -q -p huck && bash tests/scripts/readonly_assign_discard_diff_check.sh`
Expected: all `PASS`, `readonly_assign_discard_diff_check OK`. In particular `script-continues` (L2/L3 run), `loop-unwind`/`func-unwind` (unwind), and every control staying non-fatal.

- [ ] **Step 7: POSIX + no-regression**

Run:
```bash
H=target/debug/huck
echo "posix bash: [$(bash --posix -c 'readonly r=1; r=2; echo done' 2>&1 | tr '\n' '|')] rc=$(bash --posix -c 'readonly r=1; r=2; echo done' >/dev/null 2>&1; echo $?)"
echo "posix huck: [$($H --posix -c 'readonly r=1; r=2; echo done' 2>&1 | tr '\n' '|')] rc=$($H --posix -c 'readonly r=1; r=2; echo done' >/dev/null 2>&1; echo $?)"
cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1
ulimit -v 6000000
for t in readonly_integration arrays_integration eval_integration; do
  [ -f tests/*.rs ] && cargo test -p huck --test $t --jobs 1 -- --test-threads 2 2>&1 | /usr/bin/grep '^test result' || true
done
```
Expected: posix huck EXITS (rc 1, no `done`) matching bash (v226 preserved); lib tests pass; the integration binaries (whichever exist) pass at 2 threads. (Skip a binary name that doesn't exist.)

- [ ] **Step 8: Commit**

```bash
cargo fmt --all
git add crates/huck-engine/src/builtins.rs crates/huck-engine/src/shell_state.rs crates/huck-engine/src/shell.rs crates/huck-engine/src/expand.rs crates/huck-engine/src/executor.rs crates/huck-cli/src/repl.rs tests/scripts/readonly_assign_discard_diff_check.sh
git commit -m "$(cat <<'EOF'
fix: a readonly-variable assignment error discards the current command (#31)

Measured bash: a standalone readonly assignment error DISCARDS the current
top-level command (rc 1, unwinds loops/functions) but does NOT exit the shell (a
multi-line script continues) — the same DISCARD flavor as #3, not EXITPROG as the
issue's "EXITS" framing suggested. Only --posix exits (127, v226 unchanged).

Reuses v312's pending_discard mechanism at run_assignment_list's two readonly
sites (default -> discard, posix+non-interactive -> exit 127). Renames the v312
reason InterruptReason::FatalExpansion -> DiscardCommand (the DISCARD mechanism
now has two users: arith expansion #3 + readonly assignment #31). Scope:
standalone assignment statements only; for/select var binds, inline-prefix, and
unset already match bash. Third member of the #198 error-fatality funnel.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Verification (controller, before the PR)

- [ ] `cargo fmt --all --check` — clean.
- [ ] `cargo build -p huck --locked` and `cargo build --release -p huck --locked`; forced-rebuild warning count 0.
- [ ] `/usr/bin/grep -rn 'FatalExpansion' crates/` — no output (rename complete).
- [ ] `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1` — pass.
- [ ] `-p huck` integration binaries most likely affected, each at `--test-threads 2` with a `ulimit -v` guard (e.g. `arith_integration`, `eval_integration`, `subshell_integration`, and any `readonly`/`declare`/array binary).
- [ ] `tests/scripts/run_diff_checks.sh` on both binaries — green (new harness auto-picked-up; known flake `pipeline_stage_redirect_fail_diff_check.sh` case `amb-stdin-mid`, [#180](https://github.com/jdstanhope/huck/issues/180)).
- [ ] PR with `Closes #31`; **the user merges, not you.** Wait for CI to finish and pass before saying it is ready.
