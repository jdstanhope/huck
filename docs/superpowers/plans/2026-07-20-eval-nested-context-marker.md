# v315 — `eval:` context marker + eval line base Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** a syntax error inside an `eval` string reports bash's `eval:` source marker and the outer line where `eval` was invoked (flipping posix2), and `$LINENO` inside `eval` reports the outer line — both via one `Shell::eval_frame` line-base field.

**Architecture:** Add `Shell::eval_frame: Option<u32>` (the outer eval line) + a `line_base()` helper. `eval_in_sink` sets it (save/restore, mirroring `xtrace_depth`). Three read sites add the base: the syntax-error line (shell.rs), the `$LINENO` stamp (executor.rs ×3), and the `eval:` marker (shell_state.rs `Diag::Syntax`).

**Tech Stack:** Rust (huck-engine crate), bash-diff harnesses.

## Global Constraints

- **Issue:** [#209](https://github.com/jdstanhope/huck/issues/209). Spec: `docs/superpowers/specs/2026-07-20-eval-nested-context-marker-design.md`.
- Byte-exact match with bash 5.2.21 for **single-line** eval strings (posix2 + the common `$LINENO` case). Multi-line eval strings are best-effort (bash off-by-one quirk) — do NOT chase them.
- The `eval:` marker **replaces** `-c:` (bash suppresses `-c:` inside eval).
- Out of scope: the `command substitution:` marker (separate follow-on issue).
- `line_base()` is 0 outside eval → all top-level / in-function behavior is unchanged.
- **Box/build:** `cargo build -p huck --bin huck`; `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`. NEVER `cargo test --workspace`. Run touched `-p huck` integration binaries at `--test-threads 2`. `cargo fmt --all` before commit. `/usr/bin/grep` only.
- Commit trailer (exact): `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

## File structure

- `crates/huck-engine/src/shell_state.rs` — add the `eval_frame` field (~line 864, by `current_lineno`), its initializer (~line 1103), the `line_base()` helper, and the `eval:` marker in the `Diag::Syntax` arm (~line 1244).
- `crates/huck-engine/src/builtins.rs` — `eval_in_sink` sets/restores `eval_frame`.
- `crates/huck-engine/src/shell.rs` — the syntax-error `ln` computation (~line 474) adds `line_base()`.
- `crates/huck-engine/src/executor.rs` — the three `current_lineno = …` stamps (3837, 4191, 6735) add `line_base()`.
- `tests/scripts/eval_line_diag_diff_check.sh` — NEW gold-gate harness.

---

### Task 1: `eval:` marker + eval error-line base (flips posix2)

**Files:**
- Modify: `crates/huck-engine/src/shell_state.rs` (field ~864, init ~1103, `line_base()` helper, `Diag::Syntax` arm ~1244)
- Modify: `crates/huck-engine/src/builtins.rs` (`eval_in_sink`)
- Modify: `crates/huck-engine/src/shell.rs` (syntax-error `ln` ~474)
- Create: `tests/scripts/eval_line_diag_diff_check.sh`

**Interfaces:**
- Produces: `Shell::eval_frame: Option<u32>`; `Shell::line_base(&self) -> u32`.

- [ ] **Step 1: Add the field.** In `shell_state.rs`, immediately after `current_lineno` (~line 864):
```rust
    /// Set to Some(outer_line) while executing an `eval` string: the line where
    /// the `eval` command was invoked. Drives the `eval:` syntax-error marker
    /// and the line base for inner error lines and `$LINENO`. None at top level.
    /// v315 (#209).
    pub eval_frame: Option<u32>,
```
Initialize it in the `Shell` constructor (near `current_lineno: 0,` ~line 1103): `eval_frame: None,`.

- [ ] **Step 2: Add the helper.** In `shell_state.rs` `impl Shell` (near `error_prefix`):
```rust
    /// Line offset added to an eval string's local (1-based) line numbers so
    /// they reflect the outer line where `eval` sits. 0 outside eval. v315 (#209).
    pub fn line_base(&self) -> u32 {
        self.eval_frame.map_or(0, |n| n.saturating_sub(1))
    }
```

- [ ] **Step 3: Write the failing harness** `tests/scripts/eval_line_diag_diff_check.sh` (model on `readonly_assign_discard_diff_check.sh`; runs both shells, normalizes only each shell's own name prefix, diffs stderr + rc). Include ONLY the error-diagnostic cases in this task (the `$LINENO` cases come in Task 2):
```bash
#!/usr/bin/env bash
# v315 (#209): syntax error inside eval reports bash's `eval:` marker + the
# outer line where eval sits.
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: build with cargo build -p huck" >&2; exit 1; }
FAIL=0
norm() { sed -E "s#^(bash|.*/huck): #SH: #"; }
snorm() { sed -E "s#^.*/[^:]+: #SH: #"; }
# Capture merged stdout+stderr (the $LINENO cases print to stdout, the error
# cases to stderr) AND the SHELL's rc — WITHOUT a pipe (a pipe to norm would make
# $? = sed's exit, not the shell's), then normalize the captured text afterward.
# -c fragment cases
check() {
  local label=$1 frag=$2 b h br hr
  b=$(bash -c "$frag" 2>&1); br=$?; b=$(printf '%s' "$b" | norm)
  h=$("$HUCK" -c "$frag" 2>&1); hr=$?; h=$(printf '%s' "$h" | norm)
  if [ "$b" != "$h" ] || [ "$br" != "$hr" ]; then
    echo "FAIL [$label]"; echo "  bash(rc=$br): [$b]"; echo "  huck(rc=$hr): [$h]"; FAIL=1
  else echo "PASS [$label]"; fi
}
# script-file cases (eval on a specific line)
check_script() {
  local label=$1; shift; local f; f=$(mktemp); printf '%s\n' "$@" > "$f"
  local b h br hr
  b=$(bash "$f" 2>&1); br=$?; b=$(printf '%s' "$b" | snorm)
  h=$("$HUCK" "$f" 2>&1); hr=$?; h=$(printf '%s' "$h" | snorm)
  rm -f "$f"
  if [ "$b" != "$h" ] || [ "$br" != "$hr" ]; then
    echo "FAIL [$label]"; echo "  bash(rc=$br): [$b]"; echo "  huck(rc=$hr): [$h]"; FAIL=1
  else echo "PASS [$label]"; fi
}
# --- eval: marker + line (the fix)
check 'eval-c-marker'     'eval "case esac in esac) ;; esac"'
check_script 'eval-script-line3' 'echo a' 'echo b' 'eval "case esac in esac) ;; esac"'
# --- control: a non-eval top-level syntax error still uses -c:, no eval: marker
check 'noneval-control'   'case esac in esac) ;; esac'
if [ $FAIL -ne 0 ]; then echo "eval_line_diag_diff_check FAILED" >&2; exit 1; fi
echo "eval_line_diag_diff_check OK"
```
`chmod +x tests/scripts/eval_line_diag_diff_check.sh`. Run it: `cargo build -p huck --bin huck && bash tests/scripts/eval_line_diag_diff_check.sh` — expect `eval-c-marker` and `eval-script-line3` to FAIL (huck says `-c: line 1`), `noneval-control` to PASS. This is the red state.

- [ ] **Step 4: Set the frame in `eval_in_sink`** (`builtins.rs`). Add the save/set/restore around the existing `process_line_in_sinks` call, mirroring the `xtrace_depth` save/restore:
```rust
    let saved_frame = shell.eval_frame;
    shell.eval_frame = Some(shell.current_lineno.max(1));
    let saved = shell.xtrace_depth;
    shell.xtrace_depth += 1;
    let r = crate::shell::process_line_in_sinks(&joined, shell, true, sink, err_sink);
    shell.xtrace_depth = saved;
    shell.eval_frame = saved_frame;
    r
```
(`.max(1)` handles the top-level `current_lineno == 0` initial state so `-c 'eval "…"'` reports `eval: line 1`.)

- [ ] **Step 5: Add the base to the syntax-error line** (`shell.rs` ~474). The `Err(e)` arm currently computes `let ln = 1 + line.as_bytes()[..off].iter().filter(|&&b| b == b'\n').count() as u32;`. Prepend the base:
```rust
            let ln = shell.line_base() + 1 + line.as_bytes()[..off]
                .iter()
                .filter(|&&b| b == b'\n')
                .count() as u32;
```

- [ ] **Step 6: Add the `eval:` marker** (`shell_state.rs` `Diag::Syntax` arm ~1244):
```rust
            Diag::Syntax { line } => {
                if !self.is_interactive {
                    if self.eval_frame.is_some() {
                        out.push_str("eval: ");
                    } else if self.source_depth == 0 && self.is_command_string {
                        out.push_str("-c: ");
                    }
                    out.push_str(&format!("line {line}: "));
                }
            }
```

- [ ] **Step 7: Verify green.** `cargo build -p huck --bin huck && bash tests/scripts/eval_line_diag_diff_check.sh` — all 3 cases PASS. Also `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1` (no regression). Confirm the posix2-shape case directly: `./target/debug/huck -c 'eval "case esac in esac) ;; esac"' 2>&1` matches `bash -c 'eval "case esac in esac) ;; esac"' 2>&1` byte-for-byte (both `… eval: line 1: syntax error near unexpected token \`)'` + the echo line).
- [ ] **Step 8: fmt + commit** (`v315 task 1: eval: marker + eval error-line base`).

---

### Task 2: `$LINENO` line base inside eval

**Files:**
- Modify: `crates/huck-engine/src/executor.rs` (the three `current_lineno = …` stamps: ~3837, ~4191, ~6735)
- Modify: `tests/scripts/eval_line_diag_diff_check.sh` (add `$LINENO` cases)

**Interfaces:**
- Consumes: Task 1's `Shell::line_base()`.

- [ ] **Step 1: Add the `$LINENO` cases to the harness** (they FAIL now — huck reports 1):
```bash
# --- $LINENO inside eval reflects the outer line (single-line eval string)
check 'lineno-c'          'eval "echo $LINENO"'
check_script 'lineno-script-line3' 'echo a' 'echo b' 'eval "echo $LINENO"'
# --- control: top-level $LINENO unaffected
check_script 'lineno-toplevel' 'echo a' 'echo $LINENO'
```
Run: `bash tests/scripts/eval_line_diag_diff_check.sh` — expect `lineno-c` (bash `1`, huck `1` — may already pass) and `lineno-script-line3` (bash `3`, huck `1` — FAILS), `lineno-toplevel` PASS. Record the red state.

- [ ] **Step 2: Add the base at all three `current_lineno` stamps** (`executor.rs`). Each currently assigns a parse-time line directly; add `shell.line_base() +`:
  - ~3837: `shell.current_lineno = shell.line_base() + *line;`
  - ~4191: `shell.current_lineno = shell.line_base() + cmd.line;`
  - ~6735: `shell.current_lineno = shell.line_base() + exec.line;`
  Read ~5 lines around each to confirm the variable name (`*line` / `cmd.line` / `exec.line`) and that `shell` is in scope.

- [ ] **Step 3: Verify green.** `cargo build -p huck --bin huck && bash tests/scripts/eval_line_diag_diff_check.sh` — all cases (incl. the new `$LINENO` ones) PASS. Confirm directly: `printf 'echo a\necho b\neval "echo $LINENO"\n' | ./target/debug/huck` prints `3` (matching bash).
- [ ] **Step 4: `$LINENO` regression guard.** Run every existing `$LINENO` test: `/usr/bin/grep -rln 'LINENO' crates/huck-engine/src tests/ | /usr/bin/grep -v target` — run the covering lib tests (`cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 lineno`) and any `-p huck` integration binary that greps for LINENO, at `--test-threads 2`. Top-level and in-function `$LINENO` must be unchanged (`line_base()` is 0 there). Report the binaries + results.
- [ ] **Step 5: fmt + commit** (`v315 task 2: $LINENO line base inside eval`).

---

### Task 3: posix2 flip + verification + docs + follow-on issue

**Files:**
- Modify: `docs/bash-test-suite-baseline.md`

- [ ] **Step 1: Confirm posix2 flips.** `export BASH_SOURCE_DIR=/tmp/bash-5.2.21; HUCK_BASH_TEST_CATEGORY=posix2 timeout 120 bash tests/bash-test-suite/runner.sh 2>&1 | /usr/bin/grep -E '\| posix2 \||PASS:|FAIL:'`. Expected: **posix2 now PASS** (the #209 payoff). If it still FAILs, read `/tmp/huck-bash-tests-*/posix2.diff` and report the residual — do NOT leave it unexplained.
- [ ] **Step 2: Guard the top-level harness.** `bash tests/scripts/syntax_error_diag_diff_check.sh` — still fully green (non-eval paths unaffected).
- [ ] **Step 3: Full lib + diff-check sweep.** `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1` (green); build both binaries (`cargo build -p huck --bin huck` + `cargo build --release -p huck --bin huck`), then `ulimit -v 1500000; timeout 900 bash tests/scripts/run_diff_checks.sh 2>&1 | tail -5` (green; `coproc_diff_check.sh` is a known pre-existing timing flake — if ONLY that fails, re-run it in isolation to confirm and note it).
- [ ] **Step 4: Update `docs/bash-test-suite-baseline.md`.** Bump PASS 15 → 16, mark `posix2` PASS (was the #209 near-miss; now resolved by the `eval:` marker + line base), and refresh the provenance line for the v315 sweep.
- [ ] **Step 5: File the follow-on issue** for the deferred `command substitution:` marker: `gh issue create` (labels `divergence`, `bug`, `sev:low`), titled about the `command substitution:` marker for expansion-time comsub reparse errors (e.g. unterminated backtick body), noting it's the parse-vs-expansion-time distinction, `$(…)` already matches, and referencing #209. Record the issue number.
- [ ] **Step 6: fmt + commit** (`v315 task 3: posix2 flips to PASS + baseline + follow-on issue`).

---

## Notes for the executor

- The whole behavioral change is `line_base()` being non-zero only inside eval. If any NON-eval test changes output, something is wrong — `eval_frame` must be `None` everywhere except inside `eval_in_sink`'s inner call.
- Do NOT chase byte-exact multi-line-eval-string line numbers (bash off-by-one quirk, spec-documented as approximate). The harness uses single-line eval strings only.
- posix2 is a script file (not `-c`), so the `eval:` marker must work with a script-name prefix (`<name>: eval: line 199:`), which the `Diag::Syntax` arm handles (it suppresses `-c:` but the script name comes from `shell_argv0`, unaffected).
