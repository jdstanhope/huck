# v364 — one adjudication per failure: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A failure is adjudicated for `set -e` and the ERR trap exactly once, at
the site that produced it — closing [#676](https://github.com/jdstanhope/huck/issues/676)
and [#685](https://github.com/jdstanhope/huck/issues/685).

**Architecture:** `executor.rs::body_already_fired_err` already encodes the right
predicate and is used to stop a compound firing ERR twice. It is renamed
`status_produced_by_body`, gains `Command::Redirected`, and now gates errexit as
well as ERR. A redirect failure is the wrapper's OWN failure, so `run_redirected`
adjudicates it at the point it happens, through a reporter that has been split
into "decide" and "do". Before any of that, four duplicated loop-body matches
become one helper, so the change is threaded through one place rather than four.

**Tech Stack:** Rust (edition 2024), `crates/huck-engine/src/executor.rs`; bash-diff
harnesses under `tests/scripts/`; `tools/runsweep` for the runtime gate.

**Spec:** `docs/superpowers/specs/2026-08-19-errexit-adjudication-design.md`

## Global Constraints

- Branch `v364-errexit-adjudication`, off `main`. Do NOT merge it — a `vNN` PR is handed to the user.
- Every commit ends with `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- `cargo fmt --all` before every commit; CI enforces `--check`.
- Lint with the PINNED toolchain: `cargo +1.97.1 clippy --workspace --all-targets --locked -- -D warnings`.
- This box has 1 core / 1.9 GB. Run tests per-crate with `ulimit -v 8000000` and `--jobs 1`; run anything longer than ~8 minutes with `setsid nohup … &` and poll its log.
- The `-p huck` integration suites need `-- --test-threads 4`.
- Harness error rows must normalise the program-name prefix: `norm() { sed -E 's#^[^:]*: line #SH: line #'; }` — bash says `bash:` under `-c`, huck says its argv[0] path.
- ⚠️ An ERR-trap assertion must write the trap action's output to **stderr** (`trap "echo ERRFIRE >&2" ERR`). Counting on stdout hides a fire that happened inside the redirect under test — measured: stdout counting read `bash: 0, huck: 1` where the truth was `bash: 1, huck: 2`.

---

### Task 0: One loop-body outcome helper (behaviour-preserving)

`run_while_inner`, `run_for_inner`, `run_arith_for_inner` and `run_select_inner`
carry the same ~20-line match. Collapse them so Task 1 changes one place.

**Files:**
- Modify: `crates/huck-engine/src/executor.rs` (add the helper; rewrite four call sites at approximately lines 1444, 1587, 1831, 2048)

**Interfaces:**
- Produces: `enum LoopStep { Next(i32), Stop(i32), Propagate(ExecOutcome) }` and
  `fn loop_body_step(outcome: ExecOutcome) -> LoopStep`, used by the four loop runners.

- [ ] **Step 1: Capture the baseline that proves zero behaviour change**

```bash
cd /home/john/projects/huck
git checkout -b v364-errexit-adjudication
(ulimit -v 8000000; cargo build --locked --jobs 1 --bin huck)
cp target/debug/huck /tmp/huck-v364-base
```

- [ ] **Step 2: Add the helper**

Put this immediately above `fn run_while(` in `executor.rs`:

```rust
/// What a loop body's outcome means to the loop that ran it.
///
/// The four loop runners (`while`/`until`, `for`, arith-`for`, `select`) each
/// carried an identical ~20-line match on this. One copy means a fix lands in
/// all four — which matters now, because v364 threads status provenance through
/// exactly this path.
enum LoopStep {
    /// Keep iterating; the loop's status so far is this.
    Next(i32),
    /// Stop iterating; the loop's status is this (`break` at THIS level).
    Stop(i32),
    /// Not this loop's business — propagate unchanged. `break`/`continue`
    /// bound for an OUTER loop have already been decremented one level.
    Propagate(ExecOutcome),
}

fn loop_body_step(outcome: ExecOutcome) -> LoopStep {
    match outcome {
        ExecOutcome::LoopBreak(1, st) => LoopStep::Stop(st),
        ExecOutcome::LoopBreak(n, st) => LoopStep::Propagate(ExecOutcome::LoopBreak(n - 1, st)),
        // `continue` at this level: the loop's status resets to 0 and the
        // runner falls through to its own step/re-test, exactly as before.
        ExecOutcome::LoopContinue(1) => LoopStep::Next(0),
        ExecOutcome::LoopContinue(n) => LoopStep::Propagate(ExecOutcome::LoopContinue(n - 1)),
        ExecOutcome::Continue(c) => LoopStep::Next(c),
        // Exit / FunctionReturn / Interrupted leave the loop untouched.
        other => LoopStep::Propagate(other),
    }
}
```

- [ ] **Step 3: Rewrite the four call sites**

Each of the four `match execute_sequence_body(&clause.body, shell) { … }` blocks
becomes exactly this (keep each site's own trailing code — `select` sets
`show_menu = false` after the match, arith-`for` runs its step):

```rust
match loop_body_step(execute_sequence_body(&clause.body, shell)) {
    LoopStep::Propagate(o) => return o,
    LoopStep::Stop(st) => {
        last = ExecOutcome::Continue(st);
        break;
    }
    LoopStep::Next(c) => {
        last = ExecOutcome::Continue(c);
    }
}
```

- [ ] **Step 4: Build and run the engine tests**

```bash
cargo fmt --all
(ulimit -v 8000000; cargo test --locked --jobs 1 -p huck-engine --lib)
```
Expected: PASS, same count as before (2034 at time of writing).

- [ ] **Step 5: Prove ZERO behaviour change — the gate for this task**

Compare the new binary against the baseline over the loop-heavy harnesses AND a
direct differential. Any difference at all means the refactor is not
behaviour-preserving and must be fixed, not accepted (v343's rule).

```bash
(ulimit -v 8000000; cargo build --locked --jobs 1 --bin huck)
for h in for_list_line loop_control while_until_status select_builtin arith_for; do
  f=tests/scripts/${h}_diff_check.sh
  [ -x "$f" ] && { HUCK_BIN=$(pwd)/target/debug/huck "$f" >/tmp/new.$h 2>&1
                   HUCK_BIN=/tmp/huck-v364-base "$f" >/tmp/old.$h 2>&1
                   diff /tmp/old.$h /tmp/new.$h && echo "ZERO-DIFF $h"; }
done
for frag in 'for i in 1 2 3; do echo $i; done' \
            'i=0; while [ $i -lt 3 ]; do i=$((i+1)); echo $i; done' \
            'for i in 1 2 3; do [ $i = 2 ] && continue; echo $i; done' \
            'for i in 1 2; do for j in a b; do [ $j = b ] && break 2; echo $i$j; done; done' \
            'for i in 1 2; do for j in a b; do continue 2; done; echo no; done; echo done' \
            'f(){ for i in 1 2; do return 7; done; }; f; echo $?' \
            'for ((i=0;i<3;i++)); do [ $i = 1 ] && continue; echo $i; done'; do
  a=$(/tmp/huck-v364-base -c "$frag" 2>&1; echo "rc=$?")
  b=$(./target/debug/huck -c "$frag" 2>&1; echo "rc=$?")
  [ "$a" = "$b" ] && echo "ZERO-DIFF: $frag" || { echo "DIFF: $frag"; diff <(echo "$a") <(echo "$b"); }
done
```
Expected: every line `ZERO-DIFF`.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor(#676): one loop-body outcome helper for four loops

Preparation, with no behaviour change: while/until, for, arith-for and
select each carried the same ~20-line match translating a body outcome
into break/continue/propagate. v364 threads status provenance through
exactly this path, and four copies is four places to get it wrong.

Zero-diff gate: every loop harness and a break/continue/return matrix
compared byte-for-byte against the pre-refactor binary.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 1: Gate errexit on the same predicate that gates ERR

**Files:**
- Modify: `crates/huck-engine/src/executor.rs` (`body_already_fired_err` → `status_produced_by_body`; `finish_command`'s epilogue)
- Create: `tests/scripts/errexit_adjudication_diff_check.sh`

**Interfaces:**
- Consumes: nothing from Task 0 beyond it having landed.
- Produces: `fn status_produced_by_body(cmd: &Command) -> bool` (renamed from
  `body_already_fired_err`, same list plus `Command::Redirected` in Task 2).

- [ ] **Step 1: Write the failing harness**

Create `tests/scripts/errexit_adjudication_diff_check.sh`. Structure it as a
matrix: every construct × every failure shape × both consumers.

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for WHERE a failure is adjudicated (#676,
# #685). POSIX and bash exempt "any command executed in a `&&` or `||` list
# except the command following the final `&&` or `||`" from `set -e`. huck
# applied that at top level and lost it whenever the list was the last command
# of a compound body — the compound's inherited status was judged a second time.
#
# ⚠️ The ERR rows write the trap action to STDERR. Counting fires on stdout hid
# a fire that happened INSIDE the redirect under test and read `bash: 0, huck: 1`
# where the truth was `bash: 1, huck: 2`.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib/harness.sh"

norm() { sed -E 's#^[^:]*: line #SH: line #'; }
check() {
    local label="$1" frag="$2" b h out rc
    out=$(bash --norc --noprofile -c "$frag" 2>&1); rc=$?
    b=$(printf '%s\n' "$out" | norm; echo "EXIT:$rc")
    out=$("$HUCK_BIN" --norc -c "$frag" 2>&1); rc=$?
    h=$(printf '%s\n' "$out" | norm; echo "EXIT:$rc")
    compare "$label" "$b" "$h"
}

# ── errexit: an EXEMPT failure inside an in-place compound must not exit ──────
check 'if body'         'set -e; if true; then false && true; fi; echo R'
check 'elif body'       'set -e; if false; then :; elif true; then false && true; fi; echo R'
check 'else body'       'set -e; if false; then :; else false && true; fi; echo R'
check 'for body'        'set -e; for i in x; do false && true; done; echo R'
check 'while body'      'set -e; i=0; while [ $i = 0 ]; do i=1; false && true; done; echo R'
check 'until body'      'set -e; i=0; until [ $i = 1 ]; do i=1; false && true; done; echo R'
check 'arith-for body'  'set -e; for ((i=0;i<1;i++)); do false && true; done; echo R'
check 'case body'       'set -e; case x in x) false && true;; esac; echo R'
check 'brace group'     'set -e; { false && true; }; echo R'
check 'nested compound' 'set -e; if true; then { false && true; }; fi; echo R'
check 'deeply nested'   'set -e; for i in x; do if true; then { false && true; }; fi; done; echo R'
check 'or-list exempt'  'set -e; if true; then false || true; fi; echo R'
check 'bang in body'    'set -e; if true; then ! true; fi; echo R'
check 'dbracket in body' 'set -e; if true; then [[ 1 = 2 ]] && true; fi; echo R'
check 'redirected group' 'set -e; { false && true; } > /dev/null; echo R'
check 'redirected if'    'set -e; if true; then false && true; fi > /dev/null; echo R'

# ── errexit: a status the compound OWNS must still exit ───────────────────────
check 'plain inner false'   'set -e; { false; }; echo R'
check 'plain inner in if'   'set -e; if true; then false; fi; echo R'
check 'plain inner in for'  'set -e; for i in x; do false; done; echo R'
check 'subshell exempt'     'set -e; ( false && true ); echo R'
check 'function exempt'     'set -e; f(){ false && true; }; f; echo R'
check 'redirect FAILS'      'set -e; { :; } > /nonexistent/x; echo R'
check 'redirect FAILS on if' 'set -e; if true; then :; fi > /nonexistent/x; echo R'
check 'dbracket alone'      'set -e; [[ 1 = 2 ]]; echo R'
check 'arith alone'         'set -e; (( 0 )); echo R'
check 'pipeline'            'set -e; false | cat; echo R'
check 'last of and-or'      'set -e; true && false; echo R'
check 'not last of and-or'  'set -e; false && true; echo R'

# ── errexit off: the status itself must not move ─────────────────────────────
check 'status, exempt body'  'if true; then false && true; fi; echo "st=$?"'
check 'status, plain body'   'if true; then false; fi; echo "st=$?"'
check 'status, empty for'    'for i in; do false; done; echo "st=$?"'
check 'status, no case match' 'case x in y) false;; esac; echo "st=$?"'
check 'status, if no branch' 'if false; then false; fi; echo "st=$?"'

# ── the ERR trap: fire COUNT, action written to stderr ───────────────────────
check 'ERR in if body'      'trap "echo ERRFIRE >&2" ERR; if true; then false; fi'
check 'ERR in for body'     'trap "echo ERRFIRE >&2" ERR; for i in x; do false; done'
check 'ERR in brace'        'trap "echo ERRFIRE >&2" ERR; { false; }'
check 'ERR nested braces'   'trap "echo ERRFIRE >&2" ERR; { { false; }; }'
check 'ERR exempt body'     'trap "echo ERRFIRE >&2" ERR; if true; then false && true; fi'
check 'ERR subshell'        'trap "echo ERRFIRE >&2" ERR; ( false )'
check 'ERR function'        'trap "echo ERRFIRE >&2" ERR; f(){ false; }; f'
check 'ERR redirected grp'  'trap "echo ERRFIRE >&2" ERR; { false; } > /dev/null'
check 'ERR redirected if'   'trap "echo ERRFIRE >&2" ERR; if true; then false; fi > /dev/null'
check 'ERR redirect FAILS'  'trap "echo ERRFIRE >&2" ERR; { :; } > /nonexistent/x'
check 'ERR bang bang'       'trap "echo ERRFIRE >&2" ERR; ! ! { false; }'
check 'ERR with errexit'    'set -e; trap "echo ERRFIRE >&2" ERR; if true; then false; fi'
check 'ERR action exits'    'set -e; trap "exit 9" ERR; if true; then false; fi; echo R'

harness_summary
```

- [ ] **Step 2: Run it RED and record the count**

```bash
chmod +x tests/scripts/errexit_adjudication_diff_check.sh
HUCK_BIN=$(pwd)/target/debug/huck tests/scripts/errexit_adjudication_diff_check.sh | tail -3
```
Expected: FAIL on roughly the 16 exempt-body rows plus the 2 redirected ERR rows.
**Write the exact `Total: N, Pass: P, Fail: F` line into the commit message** — a
harness that was never red proves nothing.

- [ ] **Step 3: Rename the predicate and correct its doc**

In `executor.rs`, rename `body_already_fired_err` to `status_produced_by_body`.
Keep the list and the `! !` unwrap exactly as they are. Replace the paragraph
that begins "errexit is deliberately NOT gated on this" with:

```rust
/// ⚠️ errexit IS gated on this, and the comment that used to say otherwise was
/// the bug (#676). It reasoned that `set -e; { false; }` must still exit — true,
/// but it exits through the INNER `false`'s own adjudication, not the group's.
/// The compound's second look at the same status is what wrongly exited when the
/// inner failure was EXEMPT, as in `{ false && true; }`. The rule is one
/// adjudication per failure, at the site that produced it.
```

- [ ] **Step 4: Gate errexit on it**

In `finish_command`, the epilogue becomes:

```rust
    if c != 0 && !shell.err_trap_suppressed() && !is_negated_pipeline(cmd) {
        let inherited = status_produced_by_body(cmd);
        if err_armed && !inherited {
            crate::traps::fire_err_trap(shell);
            // #442: the ERR action itself ran `exit N`. Checked here, AFTER the
            // fire and BEFORE errexit, so a trap's exit beats the errexit
            // status: bash's `set -e; trap "exit 9" ERR; false` exits 9, not 1.
            if let Some(o) = pending_unwind(shell, UnwindPhase::After) {
                return Some(o);
            }
        }
        if !inherited
            && let Some(out) = maybe_errexit(shell, c)
        {
            return Some(out);
        }
    }
```

- [ ] **Step 5: Run the harness GREEN except the redirected rows**

```bash
cargo fmt --all
(ulimit -v 8000000; cargo build --locked --jobs 1 --bin huck)
HUCK_BIN=$(pwd)/target/debug/huck tests/scripts/errexit_adjudication_diff_check.sh | tail -4
```
Expected: every row passes EXCEPT `redirected group`, `redirected if`,
`ERR redirected grp` and `ERR redirected if` — those are #685 and are Task 2.

- [ ] **Step 6: Engine and syntax tests**

```bash
(ulimit -v 8000000; cargo test --locked --jobs 1 -p huck-engine --lib)
(ulimit -v 8000000; cargo test --locked --jobs 1 -p huck-syntax)
```
Expected: PASS. If a unit test fails, read it before changing anything: a test
asserting that a compound exits under `set -e` with an EXEMPT inner failure was
asserting the bug, and must be corrected to bash's behaviour with a comment
saying so. A test asserting a PLAIN inner failure exits is correct and a failure
there means the fix is wrong.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "fix(#676): adjudicate a failure once, where it happened

<the RED count from Step 2, and what is still red for Task 2>

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: A redirected compound (#685)

`{ false; } > /dev/null` fires ERR twice — once at the inner `false` inside the
redirect, once at the `Redirected` wrapper. But `{ :; } > /nonexistent/x` is the
wrapper's OWN failure and must still fire and still exit. So the wrapper is
pass-through when the inner ran, and a producer when the redirect failed.

**Files:**
- Modify: `crates/huck-engine/src/executor.rs` (`status_produced_by_body`; `finish_command` split; `run_redirected`; `run_command`'s `Redirected` arm)

**Interfaces:**
- Consumes: `status_produced_by_body` from Task 1.
- Produces: `fn finish_command_own(cmd: &Command, c: i32, err_armed: bool, shell: &mut Shell) -> Option<ExecOutcome>`
  — adjudicates a status the caller KNOWS is its own, bypassing the kind check.

- [ ] **Step 1: Split the reporter**

Rename the existing body to `finish_command_inner`, taking an explicit
`inherited: bool` in place of the internal `status_produced_by_body(cmd)` call,
and add two entry points:

```rust
/// Adjudicate a finished command: the kind decides whether its status is its
/// own or inherited from a body that was already adjudicated.
fn finish_command(
    cmd: &Command,
    c: i32,
    err_armed: bool,
    shell: &mut Shell,
) -> Option<ExecOutcome> {
    finish_command_inner(cmd, c, err_armed, status_produced_by_body(cmd), shell)
}

/// Adjudicate a status the CALLER knows is its own, where the command kind
/// alone would say otherwise — a redirect that failed on a compound (#685).
/// The inner command never ran, so nothing inside adjudicated anything.
fn finish_command_own(
    cmd: &Command,
    c: i32,
    err_armed: bool,
    shell: &mut Shell,
) -> Option<ExecOutcome> {
    finish_command_inner(cmd, c, err_armed, false, shell)
}
```

- [ ] **Step 2: Add `Redirected` to the pass-through list**

In `status_produced_by_body`'s `matches!`, add `| Command::Redirected { .. }`,
with:

```rust
            // #685: a redirected compound's status is the INNER command's
            // whenever the inner ran — `{ false; } > /dev/null` fired ERR twice,
            // once inside the redirect and once at the wrapper. When the
            // REDIRECT is what failed, the inner never ran and the status is the
            // wrapper's own; `run_redirected` adjudicates that case itself.
```

- [ ] **Step 3: Adjudicate the redirect failure where it happens**

Change `run_command`'s arm to pass the whole command:

```rust
        Command::Redirected { inner, redirects } => run_redirected(cmd, inner, redirects, shell),
```

and `run_redirected` to:

```rust
fn run_redirected(
    whole: &Command,
    inner: &Command,
    redirects: &[crate::command::Redirection],
    shell: &mut Shell,
) -> ExecOutcome {
    // #170: stamp `current_lineno` from the compound command's first sub-command
    // BEFORE applying the trailing redirects, so a redirect-open error carries
    // the `line N:` prologue.
    if let Some(l) = command_line(inner).filter(|&l| l != 0) {
        shell.current_lineno = shell.line_base() + l;
    }
    // #444: snapshot BEFORE anything runs, so a command that INSTALLS the ERR
    // trap is not itself caught by it.
    let err_armed = crate::traps::err_trap_armed(shell);
    let inner_ran = std::cell::Cell::new(false);
    let out = with_redirect_scope(redirects, shell, |shell| {
        inner_ran.set(true);
        run_command(inner, shell)
    });
    // #685: the inner never ran, so this status is the redirect's failure and
    // nothing has adjudicated it. Do it here — the site that produced it.
    if !inner_ran.get()
        && let ExecOutcome::Continue(c) = out
        && c != 0
        && let Some(o) = finish_command_own(whole, c, err_armed, shell)
    {
        return o;
    }
    out
}
```

- [ ] **Step 4: Run the harness fully GREEN**

```bash
cargo fmt --all
(ulimit -v 8000000; cargo build --locked --jobs 1 --bin huck)
HUCK_BIN=$(pwd)/target/debug/huck tests/scripts/errexit_adjudication_diff_check.sh | tail -3
```
Expected: `Fail: 0`.

- [ ] **Step 5: Prove the ERR fire LANDS in the same place as bash's**

Count is not enough — bash's fire happens inside the redirect, huck's used to
happen outside it.

```bash
for sh in bash ./target/debug/huck; do
  printf '%s: ' "$sh"
  $sh -c 'trap "echo ERRFIRE" ERR; { false; } > /tmp/v364-err.txt'
  tr -d '\n' < /tmp/v364-err.txt; echo " <- inside the redirect"
done
```
Expected: both print exactly one `ERRFIRE` inside the redirect file.

- [ ] **Step 6: Engine tests, then commit**

```bash
(ulimit -v 8000000; cargo test --locked --jobs 1 -p huck-engine --lib)
git add -A
git commit -m "fix(#685): a redirected compound adjudicates only its own failure

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Full gates, docs, blog, PR

**Files:**
- Modify: `docs/architecture.md` (the errexit/ERR paragraph in cross-cutting conventions)
- Create: `site/content/blog/<slug>.mdx`

- [ ] **Step 1: Clippy on the pinned toolchain**

```bash
(ulimit -v 8000000; cargo +1.97.1 clippy --workspace --all-targets --locked --jobs 1 -- -D warnings)
```
Expected: no output.

- [ ] **Step 2: Full test tree and the harness sweep, backgrounded**

```bash
SP=/tmp/claude-1000/-home-john-projects-huck/*/scratchpad
setsid nohup bash -c 'ulimit -v 8000000
  cargo test --locked --jobs 1 --no-fail-fast -p huck -- --test-threads 4 > /tmp/v364-huck.log 2>&1
  cargo build --release --locked --jobs 1 --bin huck >/dev/null 2>&1 &&
    tests/scripts/run_diff_checks.sh > /tmp/v364-sweep.log 2>&1
  echo DONE > /tmp/v364-done' >/dev/null 2>&1 &
```
Poll `/tmp/v364-done`. Expected: 0 failed; sweep `313 passed, 0 failed`.
⚠️ If a pty test fails, re-run it IDLE before believing it — this box is
one core and those tests are load-sensitive.

- [ ] **Step 3: The runtime sweep — the gate that matters most**

`set -e` is in nearly every system script, and a wrongly-KEPT exemption is
silent: a script that should have exited and does not cannot be seen in a status
check, only in its output.

```bash
awk -F'\t' '{print $3}' tools/scripts.tsv | while read -r p; do
  [ -f "$p" ] && [ -r "$p" ] && echo "$p"; done > tools/runsweep/paths.txt
sg docker -c 'bash tools/runsweep/build.sh'
sg docker -c 'bash tools/runsweep/run.sh'
```

Then compare PER SCRIPT against `tools/run_results.aug19.tsv` — bucket totals are
not comparable if the corpus changed:

```bash
python3 - <<'PY'
def load(p):
    return {l.split('\t')[0]: l.split('\t')[1] for l in open(p) if '\t' in l}
a, b = load('tools/run_results.aug19.tsv'), load('tools/run_results.tsv')
common = set(a) & set(b)
bad = {'RUN_HUCK_DIFF', 'RUN_HUCK_ERROR'}
print('regressions:', sorted(k for k in common if a[k] == 'RUN_AGREE' and b[k] in bad))
print('fixed:', sorted(k for k in common if a[k] in bad and b[k] == 'RUN_AGREE'))
PY
```
Expected: `regressions: []`. Record the bucket counts in the iteration log.

- [ ] **Step 4: Update `docs/architecture.md`**

In the cross-cutting section that covers errexit/ERR, state the model: one
adjudication per failure, at the site that produced it;
`status_produced_by_body` names the compound kinds whose status is inherited;
`run_redirected` adjudicates a failed redirect itself because the inner never
ran. Note that the four loop runners share `loop_body_step`.

- [ ] **Step 5: Blog entry**

`site/content/blog/<slug>.mdx`, frontmatter `title`/`date`/`summary`/`tags`/
`version: "v364"`. Lead with the symptom: a script with `[ -f x ] && do_it`
inside an `if` exited early under `set -e` and said nothing. Get the "before"
by building the branch point in a throwaway worktree — never from memory:

```bash
git worktree add /tmp/v364-before $(git merge-base main HEAD)
(cd /tmp/v364-before && cargo build --locked --jobs 1 --bin huck)
```

Validate before committing:

```bash
cd site && export NVM_DIR="$HOME/.nvm" && . "$NVM_DIR/nvm.sh" && nvm use node >/dev/null \
  && ( ulimit -v 12000000; node_modules/.bin/velite --strict )
```

- [ ] **Step 6: Open the PR — do NOT merge**

```bash
git push -u origin v364-errexit-adjudication
gh pr create --base main --title "v364: one adjudication per failure" \
  --body "Closes #676. Closes #685. …"
```
Wait for the GitHub run to FINISH and pass, then hand it to the user. A `vNN`
iteration PR is theirs to merge.

---

## Self-Review

**Spec coverage.** Model (one adjudication at the producing site) → Tasks 1 and 2.
Pass-through kind list → Task 1 Step 3/4. `Redirected` both ways → Task 2.
Consolidation with a zero-diff gate → Task 0. Matrix harness over constructs ×
failure shapes × both consumers → Task 1 Step 1. Red-count gate → Task 1 Step 2.
Zero-diff gate → Task 0 Step 5. Diff sweep → Task 3 Step 2. Runtime sweep
per-script → Task 3 Step 3. Out-of-scope items (#683, #679, #680, `case`/
`BraceGroup` bodies) are not touched by any task.

**Placeholder scan.** None: every code step carries the code, and the PR body is
the only "…", which is prose the author writes from the completed work.

**Type consistency.** `LoopStep`/`loop_body_step` (Task 0) are used only by the
four loop runners. `status_produced_by_body` is introduced in Task 1 and extended
in Task 2. `finish_command_inner`/`finish_command_own` are introduced together in
Task 2 Step 1 and used in Task 2 Step 3. No task references a name another task
did not define.

**Ordering.** Task 0 is inert and provable, so Task 1 threads one place instead
of four. Task 1 leaves four harness rows deliberately red, and Task 2 is exactly
those rows — so each task's gate is unambiguous.
