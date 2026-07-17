# v310 — Compound `2>&1` inside `$()` merges stderr into the capture — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `$( { cmd; cmd >&2; } 2>&1 )` (and `( … ) 2>&1`) captures both streams in program order, matching bash — for builtin, external, and mixed group bodies.

**Architecture:** Give the compound-redirect path (`with_redirect_scope`) the same `2>&1`-into-capture detection the simple-command path already has, via a shared predicate `redirs_merge_err_into_out`. When a captured-stdout group's redirects merge fd 2 into fd 1, hand the inner body a `StderrSink::Merged` so each inner command routes its stderr into the same capture by existing machinery.

**Tech Stack:** Rust, the `StdoutSink`/`StderrSink`/`RedirectDest` model in `executor.rs`, a bash-diff harness.

**Spec:** `docs/superpowers/specs/2026-07-17-comsub-compound-merge-stderr-design.md` — read it first.

**Issues:** [#176](https://github.com/jdstanhope/huck/issues/176) (this fix); [#195](https://github.com/jdstanhope/huck/issues/195) (the `2>&1 >file` ordering case, explicitly out of scope).

## Global Constraints

- **Branch:** `v310-comsub-merge-stderr`. Never commit to `main`; never merge.
- **Commit trailer**, exactly: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- **`cargo fmt --all`** before committing — CI enforces `cargo fmt --all --check`.
- **⚠️ NEVER run `cargo test --workspace` or a bare `cargo test`** — 1 core / 1.9 GB box; it OOM-kills the session. Per-crate: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`. Build the binary with `cargo build -p huck`. Integration binaries at `--test-threads 1` here.
- **The merge fires ONLY when** `matches!(sink, StdoutSink::Capture(_))` AND `redirs_merge_err_into_out(redirs, shell)` is true. The terminal case (`err_sink` unchanged) must be untouched — no regression.
- **Keep the real `dup2`** (`scope.apply_redirects`) exactly as today — it is inert in both consumers, and removing it is not part of this fix.
- **`2>&1 >file` / `>file 2>&1` are OUT OF SCOPE** (#195). The shared predicate must return `false` for them; the harness pins their current behavior so they are not silently changed.
- **`⚠️ NOTE: the `grep` shim in this environment is broken** — use `/usr/bin/grep` for content searches.

---

### Task 1: shared predicate + compound-path Merged routing + harness

**Files:**
- Create: `tests/scripts/comsub_merge_stderr_diff_check.sh`
- Modify: `crates/huck-engine/src/executor.rs` (add `redirs_merge_err_into_out`; rewrite `route_err_to_out` at ~1373; add the Merged-routing block in `with_redirect_scope` at ~1284)

**Interfaces:**
- Consumes: existing `final_dests_for_1_2(redirs: &[Redirection], shell: &mut Shell) -> (RedirectDest, RedirectDest)`, `RedirectDest::{Sink, Follows}`, `StdoutSink::Capture`, `StderrSink::Merged`.
- Produces: `fn redirs_merge_err_into_out(redirs: &[Redirection], shell: &mut Shell) -> bool` (crate-private, used by both `run_builtin_with_redirects` and `with_redirect_scope`).

- [ ] **Step 1: Write the failing harness**

Create `tests/scripts/comsub_merge_stderr_diff_check.sh`. It byte-diffs huck vs bash on stdout+stderr+rc for each fragment — this catches both the reversal (bytes land on fd 1 out of order) and the leak (bytes land on a different fd than bash).

```bash
#!/usr/bin/env bash
# v310 (#176): a compound group/subshell's stderr under `2>&1` INSIDE a command
# substitution must merge into the captured stdout in program order, matching
# bash. huck applied the group's `2>&1` only as a real dup2, but the comsub
# capture is an in-memory Vec (builtins) / per-command pipe (externals) with no
# single real fd — so stderr leaked out of the capture. Fixed by routing the
# inner body's stderr through a software Merged sink (see with_redirect_scope).
#
# Each case compares (stdout, stderr, exit_code) byte-identically. `printf
# "<%s>"` prints the capture so its ordering vs a leaked stream is visible.
#
# OUT OF SCOPE (#195): the `2>&1 >file` ordering case — pinned below to huck's
# CURRENT behavior so it is not silently changed.
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: huck binary not found at $HUCK (build with: cargo build -p huck)" >&2; exit 1; }

FAIL=0
# Compare huck vs bash on (stdout | stderr | rc), byte-identical.
check() {
  local label=$1 frag=$2
  local bo be br ho he hr
  bo=$(bash -c "$frag" 2>/tmp/v310_be); br=$?; be=$(cat /tmp/v310_be)
  ho=$("$HUCK" -c "$frag" 2>/tmp/v310_he); hr=$?; he=$(cat /tmp/v310_he)
  if [ "$bo" != "$ho" ] || [ "$be" != "$he" ] || [ "$br" != "$hr" ]; then
    echo "FAIL [$label]"
    echo "  bash: out=[$bo] err=[$be] rc=$br"
    echo "  huck: out=[$ho] err=[$he] rc=$hr"
    FAIL=1
  else
    echo "PASS [$label]"
  fi
}

# --- The fix: compound 2>&1 inside $() must capture both streams, in order.
check 'builtin-group'   'x=$( { echo out; echo er >&2; } 2>&1 ); printf "<%s>" "$x"'
check 'external-group'  'x=$( { /bin/echo out; /bin/echo er >&2; } 2>&1 ); printf "<%s>" "$x"'
check 'subshell'        'x=$( ( echo out; echo er >&2 ) 2>&1 ); printf "<%s>" "$x"'
check 'mixed-group'     'x=$( { echo out; /bin/echo er >&2; } 2>&1 ); printf "<%s>" "$x"'
check 'group-in-func'   'f(){ got=$( { echo out; echo er >&2; } 2>&1 ); printf "<%s>" "$got"; }; f'
check 'nested-comsub'   'x=$( echo "[$( { echo a; echo b >&2; } 2>&1 )]" ); printf "<%s>" "$x"'

# --- Controls: must stay correct (no over-fire, no regression).
check 'no-merge'        'x=$( echo out; echo er >&2 ); printf "<%s>" "$x"'      # er -> terminal
check 'simple-2>&1'     'x=$( echo hi 2>&1 ); printf "<%s>" "$x"'
check 'terminal-group'  '{ echo out; echo er >&2; } 2>&1'                        # no comsub, both -> term
check 'stdout-to-file'  'x=$( { echo out; echo er >&2; } >/tmp/v310_f 2>&1 ); printf "<%s>[%s]" "$x" "$(cat /tmp/v310_f)"'  # both -> file

# --- OUT OF SCOPE (#195): 2>&1 >file. bash captures er, huck currently leaks it.
# Pin huck's CURRENT behavior so this fix neither fixes nor further breaks it.
# (Deliberately compares huck-to-itself: a change here should be a conscious #195
# decision, surfaced by this line flipping.)
ho=$($HUCK -c 'x=$( { echo out; echo er >&2; } 2>&1 >/tmp/v310_g ); printf "cap=<%s>" "$x"' 2>/dev/null)
if [ "$ho" = "cap=<>" ]; then echo "PASS [oos-2>&1>file-pinned (#195)]"; else echo "FAIL [oos-2>&1>file changed: [$ho] — reconcile with #195]"; FAIL=1; fi

rm -f /tmp/v310_be /tmp/v310_he /tmp/v310_f /tmp/v310_g
if [ $FAIL -ne 0 ]; then echo "comsub_merge_stderr_diff_check FAILED" >&2; exit 1; fi
echo "comsub_merge_stderr_diff_check OK"
```

- [ ] **Step 2: Run the harness — verify the 6 fix cases FAIL, controls PASS**

Run: `cargo build -q -p huck && bash tests/scripts/comsub_merge_stderr_diff_check.sh`
Expected: `FAIL [builtin-group]`, `FAIL [external-group]`, `FAIL [subshell]`, `FAIL [mixed-group]`, `FAIL [group-in-func]`, `FAIL [nested-comsub]`; the control cases (`no-merge`, `simple-2>&1`, `terminal-group`, `stdout-to-file`) and `oos-2>&1>file-pinned` PASS; overall `FAILED`. This is the RED gate. If a control FAILS, stop — the harness itself is wrong.

- [ ] **Step 3: Extract the shared predicate**

In `crates/huck-engine/src/executor.rs`, immediately AFTER `final_dests_for_1_2` (ends ~line 1224), add:

```rust
/// True when `redirs` makes fd 2 follow fd 1 (`2>&1`) with fd 1's FINAL
/// destination still the software sink — i.e. stderr should be merged into the
/// (captured) stdout IN MEMORY, not sent to a real fd. Shared by the
/// simple-command path (`run_builtin_with_redirects`) and the compound-redirect
/// path (`with_redirect_scope`). Deliberately returns false for `2>&1 >file`
/// (fd 1's final dest is the file, not the sink) — that ordering case is #195.
fn redirs_merge_err_into_out(redirs: &[Redirection], shell: &mut Shell) -> bool {
    let (final_1, final_2) = final_dests_for_1_2(redirs, shell);
    matches!(final_1, RedirectDest::Sink) && matches!(final_2, RedirectDest::Follows(1))
}
```

- [ ] **Step 4: Rewrite the simple-path `route_err_to_out` to use it (pure refactor)**

In `run_builtin_with_redirects`, replace the current `route_err_to_out` (lines ~1373-1375):

```rust
    let route_err_to_out = matches!(sink, StdoutSink::Capture(_))
        && matches!(final_1, RedirectDest::Sink)
        && matches!(final_2, RedirectDest::Follows(1));
```

with:

```rust
    let route_err_to_out =
        matches!(sink, StdoutSink::Capture(_)) && redirs_merge_err_into_out(redirs, shell);
```

Leave `route_out_to_err` (the `>&2` direction) and the `let (final_1, final_2) = …` line above it exactly as-is — `final_1`/`final_2` are still used by `route_out_to_err`. This is behavior-identical (the predicate is the same two `matches!` clauses).

- [ ] **Step 5: Add the compound-path Merged routing**

In `with_redirect_scope`, find the `inner_sink` construction (~lines 1284-1290):

```rust
    let mut terminal_sink = StdoutSink::Terminal;
    let inner_sink: &mut StdoutSink = if force_terminal {
        &mut terminal_sink
    } else {
        sink
    };
    let outcome = run_inner(shell, inner_sink, err_sink);
```

Replace it with (adds the `inner_err_sink` selection; note the `sink` reborrow ordering so both borrows are valid):

```rust
    // v310 (#176): a captured group with `2>&1` (fd 2 follows fd 1, fd 1 still
    // the sink) must route the inner body's stderr INTO the capture, in program
    // order — the same software Merged routing the simple-command path does.
    // The comsub capture has no single real fd, so the real dup2 above points
    // stderr at the terminal; Merged sends builtins to the capture buf and
    // externals to the capture pipe (executor.rs:672) instead. Terminal /
    // non-`2>&1` cases keep the passed-in err_sink unchanged.
    let merge_err =
        matches!(*sink, StdoutSink::Capture(_)) && redirs_merge_err_into_out(redirs, shell);
    let mut merged_err = StderrSink::Merged;
    let mut terminal_sink = StdoutSink::Terminal;
    let inner_sink: &mut StdoutSink = if force_terminal {
        &mut terminal_sink
    } else {
        sink
    };
    let inner_err_sink: &mut StderrSink = if merge_err { &mut merged_err } else { err_sink };
    let outcome = run_inner(shell, inner_sink, inner_err_sink);
```

**Borrow-checker note:** compute `merge_err` (which borrows `*sink` and `shell` immutably) BEFORE the `inner_sink` mutable reborrow of `sink`, exactly as written above. If the borrow checker still objects because `redirs_merge_err_into_out` takes `&mut shell` while `sink` is later reborrowed, compute `merge_err` at the very top of the block (it does not depend on `inner_sink`), which is what the ordering above does.

- [ ] **Step 6: Run the harness — verify GREEN**

Run: `cargo build -q -p huck && bash tests/scripts/comsub_merge_stderr_diff_check.sh`
Expected: all `PASS`, `comsub_merge_stderr_diff_check OK`. The 6 fix cases now match bash; controls still pass; `oos-2>&1>file-pinned` still PASS (unchanged).

- [ ] **Step 7: Verify no double-routing and no simple-path regression**

Confirm the real `dup2` staying in place caused no double-emission (the spec's empirical check). Run:

```bash
H=target/debug/huck
# builtin group: exactly "out\ner" once in the capture, nothing on real fd 2
$H -c 'x=$( { echo out; echo er >&2; } 2>&1 ); printf "cap=[%s]" "$x"' 2>/tmp/v310_fd2; echo " fd2=[$(cat /tmp/v310_fd2)]"; rm -f /tmp/v310_fd2
```
Expected: `cap=[out
er] fd2=[]` — er appears once, in the capture, and real fd 2 is empty (no leak, no duplicate).

Then the simple-command path must be byte-identical after the refactor:
Run: `bash tests/scripts/engine_capture_diff_check.sh`
Expected: OK (this harness covers simple-command `2>&1` capture; the `route_err_to_out` refactor must not change it).

- [ ] **Step 8: Engine lib tests + fmt**

Run:
```bash
cargo fmt --all
cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1
```
Expected: fmt clean; all lib tests pass (no regression from the executor edits).

- [ ] **Step 9: Commit**

```bash
git add crates/huck-engine/src/executor.rs tests/scripts/comsub_merge_stderr_diff_check.sh
git commit -m "$(cat <<'EOF'
fix: compound 2>&1 inside $() merges stderr into the capture (#176)

A compound group/subshell's `2>&1` inside a command substitution was applied
only as a real dup2, but the comsub capture is an in-memory Vec (builtins) /
per-command pipe (externals) with no single real fd — so the dup pointed stderr
at the terminal and it leaked out of the capture (reversed at top level, and
appearing before the later use of the captured value).

Extracts redirs_merge_err_into_out (the `2>&1`-into-capture predicate the
simple-command path already used) and, in with_redirect_scope, routes the inner
body's stderr through a Merged sink when stdout is captured and the redirects
merge fd 2 into fd 1. Builtins then write to the capture buf, externals to the
capture pipe — both in program order. Covers external and subshell bodies too.

The ordering-subtle `2>&1 >file` case stays out of scope (#195); the shared
predicate correctly declines it (fd 1's final dest is the file).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Verification (controller, before the PR)

- [ ] `cargo fmt --all --check` — clean.
- [ ] `cargo build -p huck --locked` and `cargo build --release -p huck --locked` (the sweep needs both).
- [ ] `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1` — pass.
- [ ] The redirect/capture-adjacent `-p huck` integration binaries, each single-threaded with a `ulimit -v` guard: `captured_pipeline_drain_integration`, `subshell_integration`, `subshell_pipeline_integration`, `io_error_integration`, `compound_redirects_integration`.
- [ ] `tools/redirect_audit.sh` — 0 DIVERGE (fd-routing change).
- [ ] `tests/scripts/run_diff_checks.sh` on both binaries — green (the lone known flake is `pipeline_stage_redirect_fail_diff_check.sh` case `amb-stdin-mid`, [#180](https://github.com/jdstanhope/huck/issues/180)); the new `comsub_merge_stderr_diff_check.sh` is picked up automatically.
- [ ] PR with `Closes #176`; **the user merges, not you.** Wait for CI to finish and pass before saying it is ready.
