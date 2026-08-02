# fd one-model — Stage 0 (safety-net harnesses) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add bash-diff harnesses that pin the *current* behavior of every command-substitution / capture combination that Stage 1 (fork comsub) will move, so Stage 1 either keeps them green or has a precise, pre-agreed target to change. **No engine code changes.**

**Architecture:** New `tests/scripts/*_diff_check.sh` harnesses, auto-discovered by `run_diff_checks.sh`. Each compares `(stdout | stderr | exit-code)` byte-identically between bash 5.2.21 and huck. Cases where huck already matches bash are asserted against **bash**; cases where huck already diverges are pinned to huck's **current** output with a `# STAGE-1 TARGET (#N)` comment — so the harness is green on today's tree and documents exactly what Stage 1 must flip. Models the existing `comsub_merge_stderr_diff_check.sh` (v310/#176), which already uses this two-kinds-of-check pattern and pins #195.

**Tech Stack:** POSIX-ish bash harness scripts; `target/debug/huck` (built by `cargo build -p huck`); the existing `run_diff_checks.sh` sweep and `tools/redirect_audit.sh` / `tools/soak/` differential audits.

**Issue:** [#197](https://github.com/jdstanhope/huck/issues/197). **Design:** `docs/superpowers/specs/2026-08-02-fd-one-model-design.md`.

## Global Constraints

- **No engine/library code changes in Stage 0.** Only new files under `tests/scripts/` (and, in Task 5, an appended note to the design doc). If a harness cannot be made green without an engine change, that case is a Stage-1 target — pin it to current huck behavior, do not change engine code.
- Every harness compares **stdout, stderr, AND exit code**, byte-identical, via a `check`-style helper (copy the shape from `comsub_merge_stderr_diff_check.sh`).
- Harness filename ends in `_diff_check.sh` so `run_diff_checks.sh` auto-discovers it; `chmod +x` each; reference `HUCK=target/debug/huck` and fail loudly if the binary is absent.
- Two check kinds: `check` (assert vs bash — for cases huck already matches) and `check_pin` (assert vs a hard-coded expected string = huck's CURRENT output, with a `# STAGE-1 TARGET (#N)` comment naming the member issue).
- **The whole harness set must be green on the current tree** (debug binary) AND under the full `run_diff_checks.sh` sweep (which also builds release). Any red that is not a deliberate `check_pin` is a blocker.
- Do NOT run `cargo test --workspace` (OOM). Stage 0 runs no engine tests — only the harnesses + sweep + audits.
- Commit trailer on every commit: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- Do not duplicate cases already covered by `comsub_merge_stderr_diff_check.sh` (compound `2>&1` groups/subshells inside `$()`, and the #195 `2>&1 >file` pin). Reference it; cover the gaps.

---

### Task 1: Capture / redirect matrix harness

**Files:**
- Create: `tests/scripts/comsub_capture_matrix_diff_check.sh`

**Interfaces:**
- Produces: a harness green on the current tree; the authoritative baseline for stderr/stdout routing under `$()` for the streams Stage 1 forks.

- [ ] **Step 1: Write the harness skeleton**

Copy the header + `check` helper shape from `comsub_merge_stderr_diff_check.sh` (compares `stdout|stderr|rc`), add a `check_pin` helper:

```bash
#!/usr/bin/env bash
# Stage 0 (#197): baseline for stderr/stdout routing through `$( )` capture that
# Stage 1 (fork comsub) will move. `check` = must match bash; `check_pin` = pins
# huck's CURRENT (diverging) output, a STAGE-1 TARGET, so this file is green now.
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: build huck first (cargo build -p huck)" >&2; exit 1; }
FAIL=0
check() {  # assert huck == bash on (out|err|rc)
  local label=$1 frag=$2 bo be br ho he hr
  bo=$(bash -c "$frag" 2>/tmp/s0_be); br=$?; be=$(cat /tmp/s0_be)
  ho=$("$HUCK" -c "$frag" 2>/tmp/s0_he); hr=$?; he=$(cat /tmp/s0_he)
  if [ "$bo" != "$ho" ] || [ "$be" != "$he" ] || [ "$br" != "$hr" ]; then
    echo "FAIL [$label]"; echo "  bash: out=[$bo] err=[$be] rc=$br"; echo "  huck: out=[$ho] err=[$he] rc=$hr"; FAIL=1
  else echo "PASS [$label]"; fi
}
check_pin() {  # STAGE-1 TARGET: assert huck's CURRENT output == expected (out|err|rc)
  local label=$1 frag=$2 xo=$3 xe=$4 xr=$5 ho he hr
  ho=$("$HUCK" -c "$frag" 2>/tmp/s0_he); hr=$?; he=$(cat /tmp/s0_he)
  if [ "$ho" != "$xo" ] || [ "$he" != "$xe" ] || [ "$hr" != "$xr" ]; then
    echo "FAIL [$label] (pin drifted)"; echo "  want: out=[$xo] err=[$xe] rc=$xr"; echo "  got : out=[$ho] err=[$he] rc=$hr"; FAIL=1
  else echo "PIN  [$label]"; fi
}
```

- [ ] **Step 2: Add the matrix cases**

Cover, for both a **builtin** producer (`echo`/`printf`) and an **external** producer (`/bin/echo`):
- plain capture: `x=$(echo o); printf '<%s>' "$x"`
- stderr escapes capture: `x=$(echo o; echo e >&2); printf '<%s>' "$x"` (stderr on the terminal, only `o` captured)
- `2>&1` merges into capture: `x=$(echo o; echo e >&2 2>&1)` — note the operator scope; use the group form `x=$({ echo o; echo e >&2; } 2>&1)` only if NOT already in `comsub_merge_stderr` (it is — skip duplicates, cover the *bare-simple-command* `2>&1` instead: `x=$(echo hi 2>&1)`)
- `>file 2>&1` and `2>&1 >file` ordering **inside** `$()` (the #195 shape) — `check_pin` to current huck, comment `# STAGE-1 TARGET (#195)`
- `$(<file)` file read: `printf 'abc\n' >/tmp/s0_f; x=$(</tmp/s0_f); printf '<%s>' "$x"` (must match bash; must NOT fork — behavior only, fork-freeness verified in Stage 1)
- readonly-arith error under `$(cmd 2>&1)` capture (the #353 shape): `x=$(readonly r=1; (( r++ )) 2>&1); printf '<%s>' "$x"` — `check` if it already matches, else `check_pin` with `# STAGE-1 TARGET (#353)`

- [ ] **Step 3: Build the binary and run the harness; triage each case**

```bash
cargo build -p huck
bash tests/scripts/comsub_capture_matrix_diff_check.sh
```
Expected: every line `PASS` or `PIN`, none `FAIL`. For any `check` that FAILs, convert it to `check_pin` with the observed current output and a `# STAGE-1 TARGET (#N)` comment (finding the matching member issue: #195, #353, #77, #30) — do **not** change engine code.

- [ ] **Step 4: Commit**

```bash
chmod +x tests/scripts/comsub_capture_matrix_diff_check.sh
git add tests/scripts/comsub_capture_matrix_diff_check.sh
git commit -m "test(#197): Stage-0 comsub capture/redirect matrix baseline"
```

---

### Task 2: Large-output + nesting harness (the deadlock guard)

**Files:**
- Create: `tests/scripts/comsub_large_nesting_diff_check.sh`

**Interfaces:**
- Produces: the >64 KB case that Stage 1's fork+drain MUST handle without deadlock, plus nesting shapes.

- [ ] **Step 1: Write the skeleton** (same `check`/`check_pin` helpers as Task 1; factor by copy — these are standalone scripts).

- [ ] **Step 2: Add large-output cases** (each must match bash; these are the deadlock guards Stage 1 relies on):

```bash
check 'large 70k builtin'  'x=$(printf "%0.sX" {1..70000}); echo ${#x}'         # 70000
check 'large 70k external' 'x=$(head -c 70000 /dev/zero | tr "\0" X); echo ${#x}' # 70000
check 'large with newlines' 'x=$(for i in {1..5000}; do echo line$i; done); echo "${#x} $(echo "$x" | wc -l)"'
```

- [ ] **Step 3: Add nesting cases** (match bash unless noted):

```bash
check 'nested comsub'      'echo $(echo $(echo deep))'                 # deep
check 'comsub in pipeline' 'echo $(echo hi) | cat'                     # hi
check 'comsub in subshell' '( x=$(echo a); echo "$x" )'               # a
check 'comsub in lastpipe' 'shopt -s lastpipe; echo hi | while read l; do echo "$(echo $l)"; done'  # hi
check 'comsub arg splits'  'set -- $(echo a b c); echo $#'            # 3
```

- [ ] **Step 4: Build + run + triage** (as Task 1 Step 3). Any `check` FAIL → `check_pin` + `# STAGE-1 TARGET (#N)`.

- [ ] **Step 5: Commit** (`chmod +x`, trailer): `test(#197): Stage-0 comsub large-output + nesting baseline`.

---

### Task 3: Subshell-semantics harness

**Files:**
- Create: `tests/scripts/comsub_subshell_semantics_diff_check.sh`

**Interfaces:**
- Produces: the baseline for the behaviors a real fork changes vs today's clone (state isolation, `$?`, traps, background jobs, stdin).

- [ ] **Step 1: Write the skeleton** (same helpers).

- [ ] **Step 2: Add semantics cases** — pin bash's ACTUAL behavior so Stage 1's fork is a deliberate match, not an accident:

```bash
check 'dollar-? from exit'  'x=$(exit 7); echo $?'                         # 7
check 'dollar-? success'    'x=$(true); echo $?'                          # 0
check 'state isolation var' 'v=outer; x=$(v=inner; echo x); echo "$v"'   # outer
check 'state isolation cd'  'cd /; x=$(cd /tmp; echo x); basename "$PWD"' # (root)
check 'assign exit status'  'x=$(false); echo $?'                        # 1
check 'trap reset in comsub' 'trap "echo T" USR1; x=$(trap; echo done); echo "$x"'  # comsub sees no trap => empty trap list
check 'stdin read in comsub' 'printf "hi\n" | { x=$(cat); echo "[$x]"; }' # [hi]
```

- [ ] **Step 3: Add the background-job-in-comsub case** — this is where today's `&`→`;` sanitization may differ from a real subshell. Pin bash's behavior; if huck differs, `check_pin` current huck + `# STAGE-1 TARGET (bg-in-comsub)`:

```bash
# bash: comsub returns when the write-end is closed; a backgrounded job that
# keeps stdout open can delay it. Keep the job's stdout OUT of the comsub to
# avoid a real-time-dependent hang in the harness:
check 'bg job in comsub'    'x=$( { sleep 0.05 & } 3>/dev/null; echo now ); echo "[$x]"'  # [now]
```

- [ ] **Step 4: Build + run + triage**. Record which cases are `check` (already match) vs `check_pin` (Stage-1 targets) — this list feeds Task 5.

- [ ] **Step 5: Commit** (`chmod +x`, trailer): `test(#197): Stage-0 comsub subshell-semantics baseline`.

---

### Task 4: Full sweep + differential audits green

**Files:** none (verification only).

- [ ] **Step 1: Build both binaries**

```bash
cargo build --locked -p huck && cargo build --release --locked -p huck
```

- [ ] **Step 2: Run the full sweep** (guarded per the box's limits)

```bash
ulimit -v 8000000; tests/scripts/run_diff_checks.sh
```
Expected: `... passed, 0 failed`, and the three new harnesses appear as PASS lines.

- [ ] **Step 3: Run the differential audits**

```bash
tools/redirect_audit.sh || true   # record baseline; must not regress vs current main
ls tools/soak/ >/dev/null          # confirm soak harness present (run is optional/long)
```
If `redirect_audit.sh` shows any NEW divergence vs current main, stop — Stage 0 must be behavior-neutral.

- [ ] **Step 4: No commit** (verification task; nothing changed).

---

### Task 5: Record the Stage-1 target list

**Files:**
- Modify: `docs/superpowers/specs/2026-08-02-fd-one-model-design.md` (append a short "Stage-1 targets (pinned in Stage 0)" subsection under Staging → Stage 1).

- [ ] **Step 1: Collect every `check_pin` case** from the three harnesses (grep `STAGE-1 TARGET`) and list them under Stage 1 in the design doc, each with its member issue (`#195`, `#353`, …) and the one-line current-vs-bash difference. This is the precise, pre-agreed change-set Stage 1 must flip green.

- [ ] **Step 2: Commit** (trailer): `docs(#197): record Stage-0-pinned Stage-1 targets`.

---

## Self-Review

- **Spec coverage:** the design's Stage 0 list (capture/redirect matrix incl. #195/#353 shapes; large >64 KB output; nesting incl. pipeline/subshell/lastpipe; subshell semantics incl. `$?`/exit/trap/isolation/stdin/bg-job; `$(<file)`; builtin-vs-external) maps to Tasks 1–3; audits + sweep to Task 4; the target list to Task 5. Covered.
- **No placeholders:** every task has concrete case strings and commands.
- **Behavior-neutral:** the Global Constraints forbid engine changes; already-diverging cases are pinned, not fixed. Green-on-current-tree is the acceptance bar.
- **No duplication:** cross-referenced `comsub_merge_stderr_diff_check.sh`; Task 1 covers the gaps, not its compound-group `2>&1` cases.
