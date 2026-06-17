# v180: for-loop variable name — parse-permissive, runtime-validated — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make huck accept any single unquoted word as a `for`-loop variable at parse time (matching bash — including reserved words like `if`/`in`) and enforce the identifier rule at runtime as a non-fatal error, clearing the parse-sweep's "invalid variable name in 'for' loop" cluster.

**Architecture:** Two edits. (1) Parser: `for_variable_name` (`src/command.rs:1382`) stops delegating to the strict `valid_identifier_text` and instead accepts any single, unquoted, non-empty `Literal` word — the same shape `valid_function_name_text` already uses for function names (v175). (2) Executor: `run_for` (`src/executor.rs:1218`) validates the name with `is_valid_name` (`src/builtins.rs:591`, made `pub(crate)`) before iterating; a bad name prints `huck: \`NAME': not a valid identifier` and returns `ExecOutcome::Continue(1)` (non-fatal, so the surrounding `;`-list continues — exactly like bash).

**Tech Stack:** Rust (huck shell). Verification via `cargo test`, the `tests/scripts/*_diff_check.sh` bash-diff harnesses, and `tools/parse_sweep.sh`.

**Spec:** `docs/superpowers/specs/2026-06-17-for-var-runtime-validation-design.md`

**Branch:** `v180-for-var-runtime-validation` (create from `main`; do NOT implement on `main`).

---

## Confirmed bash behavior (the contract these tasks must match)

Verified against bash 5.x on this box:

| Fragment | `bash -n` | `bash` run (stdout / rc) |
|---|---|---|
| `for if in 1 2; do echo $if; done` | rc 0 | `1`⏎`2`, rc 0 (keyword is a valid identifier) |
| `for in in a; do echo $in; done` | rc 0 | runs, rc 0 |
| `for a-b in 1; do echo body; done; echo after` | rc 0 | `bash: \`a-b': not a valid identifier`⏎`after`, rc 0 (loop status 1 but `echo after` is last) |
| `for a-b in 1; do :; done` (no trailing cmd) | rc 0 | `bash: \`a-b': not a valid identifier`, rc 1 (loop status is final) |
| `for 1x in 1; do echo body; done; echo after` | rc 0 | `bash: \`1x': not a valid identifier`⏎`after`, rc 0 |

bash accepts ANY word as the loop variable at parse; the identifier check is at runtime and is **non-fatal** (status 1, body not run, the list continues). `is_valid_name`'s charset (`[A-Za-z_][A-Za-z0-9_]*`, no keyword exclusion) is exactly the runtime rule — `if`/`in` pass it, `a-b`/`1x` fail it.

---

### Task 1: Parse-permissive loop var + runtime identifier validation

**Files:**
- Modify: `src/command.rs:1379-1385` (`for_variable_name` + its doc comment)
- Modify: `src/command.rs:645-648` (`ForClause.var` doc comment)
- Modify: `src/builtins.rs:591` (`is_valid_name` visibility)
- Modify: `src/executor.rs:1225` (top of `run_for_inner`)

**Context:** `for_variable_name` is called from two parser sites (`src/command.rs:1509` and `:1563` — the `for NAME in …` and `for NAME; do …` forms); both call `for_variable_name(&tok).ok_or(ParseError::ForVariable)?`. The function signature (`&Token -> Option<String>`) is unchanged, so both sites benefit without edits. `valid_identifier_text` (`src/command.rs:1326`) stays as-is — it is still used for coproc names; only `for_variable_name` stops calling it. The model for the new body is `valid_function_name_text` (`src/command.rs:1359`), minus the keyword guard (bash allows keyword loop vars).

- [ ] **Step 1: Create the branch**

```bash
cd /home/john/projects/shuck
git checkout main && git checkout -b v180-for-var-runtime-validation
```

- [ ] **Step 2: Make `is_valid_name` crate-visible**

In `src/builtins.rs`, change line 591 from:

```rust
fn is_valid_name(s: &str) -> bool {
```

to:

```rust
pub(crate) fn is_valid_name(s: &str) -> bool {
```

(The body is unchanged: it accepts `[A-Za-z_][A-Za-z0-9_]*` with no keyword exclusion — so `if`/`in` pass, `a-b`/`1x` fail. This is exactly bash's runtime rule.)

- [ ] **Step 3: Rewrite `for_variable_name` to be parse-permissive**

In `src/command.rs`, replace the function and its doc comment at lines 1379-1385:

```rust
/// Returns the loop-variable name if `token` is a single, unquoted
/// `Literal` `Word` whose text is a valid identifier and not a reserved
/// keyword. Otherwise `None`.
fn for_variable_name(token: &Token) -> Option<String> {
    let Token::Word(w) = token else { return None };
    valid_identifier_text(w)
}
```

with:

```rust
/// Returns the raw loop-variable name if `token` is a single, unquoted,
/// non-empty `Literal` `Word`. bash accepts ANY word as the `for` variable at
/// parse time (including reserved words like `if`, and non-identifiers like
/// `a-b`); the identifier rule is enforced at RUNTIME (`run_for`). So this does
/// NOT apply the keyword / charset checks of `valid_identifier_text`.
fn for_variable_name(token: &Token) -> Option<String> {
    let Token::Word(w) = token else { return None };
    if w.0.len() != 1 {
        return None;
    }
    let WordPart::Literal { text, quoted: false } = &w.0[0] else {
        return None;
    };
    if text.is_empty() {
        return None;
    }
    Some(text.clone())
}
```

- [ ] **Step 4: Update the `ForClause.var` doc comment**

In `src/command.rs`, change line 647 from:

```rust
    /// The loop variable name — a validated identifier.
    pub var: String,
```

to:

```rust
    /// The raw loop variable name; identifier-validated at runtime (`run_for`).
    pub var: String,
```

- [ ] **Step 5: Add the runtime validation at the top of `run_for_inner`**

In `src/executor.rs`, the loop body is `run_for_inner` (starts at line 1225). Insert the validation as the very first statement, before the word-list expansion (bash reports the bad name before running the body). Change:

```rust
fn run_for_inner(clause: &ForClause, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {

    // Expand the word list once — the same path command arguments take.
```

to:

```rust
fn run_for_inner(clause: &ForClause, shell: &mut Shell, sink: &mut StdoutSink) -> ExecOutcome {
    // bash accepts any word as the loop variable at parse time but requires a
    // valid identifier at runtime; a bad name is a NON-FATAL error (status 1,
    // body not run, the surrounding list continues). Reserved words like `if`
    // are valid identifiers and fall through to run normally.
    if !crate::builtins::is_valid_name(&clause.var) {
        eprintln!("huck: `{}': not a valid identifier", clause.var);
        return ExecOutcome::Continue(1);
    }

    // Expand the word list once — the same path command arguments take.
```

- [ ] **Step 6: Build**

Run: `cargo build 2>&1 | tail -5`
Expected: compiles clean (no error about `is_valid_name` being private — `pub(crate)` makes `crate::builtins::is_valid_name` reachable from `executor.rs`).

- [ ] **Step 7: Manual behavior check against bash**

Run:

```bash
B=target/debug/huck
echo "--- keyword loop var (should run, rc 0) ---"
printf 'for if in 1 2; do echo $if; done\n' | $B; echo "rc=$?"
echo "--- 'in' as loop var (should run, rc 0) ---"
printf 'for in in a; do echo $in; done\n' | $B; echo "rc=$?"
echo "--- non-identifier, non-fatal, list continues (after present, rc 0) ---"
printf 'for a-b in 1; do echo body; done; echo after\n' | $B; echo "rc=$?"
echo "--- non-identifier bare (rc 1, no body) ---"
printf 'for a-b in 1; do echo body; done\n' | $B; echo "rc=$?"
echo "--- leading-digit name ---"
printf 'for 1x in 1; do echo body; done; echo after\n' | $B; echo "rc=$?"
echo "--- valid loop unchanged ---"
printf 'for x in a b; do echo v-$x; done\n' | $B; echo "rc=$?"
```

Expected (matches the bash table above):
- `for if in 1 2` → `1`⏎`2`, rc 0
- `for in in a` → `a`, rc 0
- `for a-b in … ; echo after` → ``huck: `a-b': not a valid identifier`` (stderr), `after` on stdout, rc 0 (no `body`)
- `for a-b in …` bare → ``huck: `a-b': not a valid identifier``, rc 1, no `body`
- `for 1x in … ; echo after` → ``huck: `1x': not a valid identifier``, `after`, rc 0
- `for x in a b` → `v-a`⏎`v-b`, rc 0

- [ ] **Step 8: clippy**

Run: `cargo clippy 2>&1 | tail -5`
Expected: no new warnings.

- [ ] **Step 9: Commit**

```bash
git add src/command.rs src/executor.rs src/builtins.rs
git commit -m "$(cat <<'EOF'
v180: for-loop var parse-permissive + runtime identifier validation (M-?)

for_variable_name now accepts any single unquoted non-empty Literal word at
parse time (bash accepts reserved words like `if` and non-identifiers like
`a-b` as loop vars); run_for validates the identifier at runtime via the now
pub(crate) is_valid_name, emitting a non-fatal "not a valid identifier" error
(status 1, body skipped, surrounding list continues) for bad names.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Bash-diff harness `for_var_name_diff_check.sh`

**Files:**
- Create: `tests/scripts/for_var_name_diff_check.sh`

**Context:** The harness suite under `tests/scripts/*_diff_check.sh` runs fragments through bash and huck and asserts byte-identical output. Because the `huck:` vs `bash: line N:` error PREFIX differs by intentional convention, the non-identifier cases compare **stdout + exit code only** (stderr discarded), while the positive/valid cases compare full stdout + exit (no stderr expected). Follow the structure of `tests/scripts/function_name_diff_check.sh` (a `check`-style helper, `HUCK_BIN` discovery, PASS/FAIL tally, `exit (FAIL>0)`).

- [ ] **Step 1: Write the harness**

Create `tests/scripts/for_var_name_diff_check.sh`:

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v180: a `for`-loop variable name is
# accepted as any single word at PARSE time and identifier-validated at RUNTIME.
# Reserved words (`if`, `in`) are valid identifiers and run; non-identifiers
# (`a-b`, `1x`) produce a NON-FATAL "not a valid identifier" error (status 1,
# body not run, the surrounding list continues). The error WORDING differs by
# the intentional prefix convention (`huck:` vs `bash: line N:`), so the
# non-identifier cases compare stdout+exit only (stderr discarded).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0

# Compare stdout + exit code (stderr discarded) — for cases whose only stderr is
# the intentionally-differing error prefix.
check_out() {  # label ; fragment
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash --norc --noprofile 2>/dev/null; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>/dev/null; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# Keyword loop vars run (valid identifiers).
check_out "keyword 'if' as loop var"  'for if in 1 2; do echo $if; done'
check_out "keyword 'in' as loop var"  'for in in a b; do echo $in; done'
check_out "keyword 'do'... actually keep simple" 'for if in x; do echo got-$if; done'

# Non-identifier names: non-fatal, body skipped, surrounding list continues.
check_out "hyphen name, list continues" 'for a-b in 1; do echo body; done; echo after'
check_out "hyphen name bare (rc 1)"     'for a-b in 1; do echo body; done'
check_out "leading-digit name"          'for 1x in 1; do echo body; done; echo after'
check_out "dotted name"                 'for a.b in 1 2; do echo body; done; echo after'

# Valid loops unchanged.
check_out "valid in-list"               'for x in a b c; do echo v-$x; done'
check_out "valid no-in (positionals)"   'set -- p q; for x; do echo arg-$x; done'
check_out "valid empty in-list"         'for x in; do echo never; done; echo done-empty'

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 2: Make it executable and run it**

Run:

```bash
chmod +x tests/scripts/for_var_name_diff_check.sh
cargo build 2>&1 | tail -2
bash tests/scripts/for_var_name_diff_check.sh
```

Expected: `Total: 10, Pass: 10, Fail: 0`, exit 0.

- [ ] **Step 3: Prove the harness is non-tautological (v176 lesson)**

Build the pre-fix binary in a throwaway worktree and confirm the harness FAILS against it (the non-identifier cases were parse-rejected before, so stdout/exit differ):

```bash
git worktree add /tmp/huck-prefix HEAD~2 2>/dev/null || git worktree add /tmp/huck-prefix main
( cd /tmp/huck-prefix && cargo build 2>&1 | tail -1 )
HUCK_BIN=/tmp/huck-prefix/target/debug/huck bash tests/scripts/for_var_name_diff_check.sh; echo "prefix-exit=$?"
git worktree remove --force /tmp/huck-prefix
```

Expected: against the pre-fix binary the harness FAILS (several FAIL lines, exit 1) — e.g. `for a-b in …` pre-fix prints a parse error to stdout-via-2>/dev/null? No: pre-fix the error goes to stderr (discarded) but the exit code is 2 (parse) vs bash 0/1, and `after`/`body` differ, so the cases mismatch. If it PASSES against pre-fix, the harness is tautological — STOP and strengthen it.

- [ ] **Step 4: Commit**

```bash
git add tests/scripts/for_var_name_diff_check.sh
git commit -m "$(cat <<'EOF'
v180: bash-diff harness for for-loop var parse/runtime validation

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: Update old-behavior tests, full regression, parse-sweep payoff

**Files:**
- Modify: `tests/for_integration.rs:134-139` (`for_invalid_variable_is_nonfatal_syntax_error`)
- Verify (likely no change): `tests/scripts/function_name_diff_check.sh:38`
- Run: full `cargo test`, all `tests/scripts/*_diff_check.sh`, `tools/parse_sweep.sh`

**Context — UP-FRONT grep (v178 lesson):** Before changing anything, grep ALL of `tests/` and `src/` for tests that encode the OLD parse-rejection of a for-loop var. The known sites:
1. `tests/for_integration.rs:135` `for_invalid_variable_is_nonfatal_syntax_error` — asserts stderr contains `"syntax error"`. Post-fix huck emits `"not a valid identifier"` instead (and still non-fatal). **Must update.**
2. `tests/scripts/function_name_diff_check.sh:38` `both_reject "hyphen for-var rejected" 'for a-b in 1; do :; done'` — `both_reject` RUNS the fragment and asserts both shells exit nonzero. Pre-fix huck rc 2 (parse); post-fix huck rc 1 (runtime non-fatal, loop is the final command). bash rc 1. Both still nonzero → **still PASSES, no edit needed** (the label's "rejected" now means runtime-rejected). Confirm it passes; do not weaken it.
3. `src/command.rs:5972` — a parser unit-test comment about `"123"` not being a valid identifier producing no named clause. Inspect: if it asserts `for`-var parse rejection of a non-identifier, update to the new parse-accepts behavior; if it is about a different construct (e.g. assignment/coproc), leave it.

- [ ] **Step 1: Re-run the up-front grep and inspect each hit**

Run:

```bash
grep -rn "ForVariable\|invalid variable name in 'for'\|for_variable_name\|syntax error.*for\|for 2x\|for a-b\|for 1x" tests/ src/
```

Expected: surfaces the sites above. Read each match and decide: update (encodes old parse-rejection) or leave (unrelated). Record the decision for each in the commit message.

- [ ] **Step 2: Update `for_invalid_variable_is_nonfatal_syntax_error`**

In `tests/for_integration.rs`, replace lines 134-139:

```rust
#[test]
fn for_invalid_variable_is_nonfatal_syntax_error() {
    let (out, err) = run("for 2x in a; do echo hi; done\necho still-alive\nexit\n");
    assert!(err.to_lowercase().contains("syntax error"), "stderr: {err}");
    assert!(out.lines().any(|l| l == "still-alive"), "stdout: {out}");
}
```

with:

```rust
#[test]
fn for_invalid_variable_name_is_nonfatal_runtime_error() {
    // bash parses any word as the loop var and validates the identifier at
    // runtime: a bad name (`2x`) is a NON-FATAL "not a valid identifier" error
    // (body not run, the surrounding list continues — `still-alive` prints).
    let (out, err) = run("for 2x in a; do echo hi; done\necho still-alive\nexit\n");
    assert!(err.contains("not a valid identifier"), "stderr: {err}");
    assert!(!out.lines().any(|l| l == "hi"), "loop body must not run: {out}");
    assert!(out.lines().any(|l| l == "still-alive"), "stdout: {out}");
}
```

- [ ] **Step 3: Run the updated test and the for-integration suite**

Run: `cargo test --test for_integration 2>&1 | tail -20`
Expected: all pass, including `for_invalid_variable_name_is_nonfatal_runtime_error`.

- [ ] **Step 4: Confirm the function-name harness still passes**

Run:

```bash
cargo build 2>&1 | tail -1
bash tests/scripts/function_name_diff_check.sh | tail -3
```

Expected: `Fail: 0`, exit 0 — the `for a-b in` `both_reject` case still passes (bash rc 1, huck now rc 1, both nonzero). If it FAILS, re-read the case before touching it.

- [ ] **Step 5: Full test suite**

Run: `cargo test 2>&1 | tail -30`
Expected: 0 failures across all unit + integration binaries. If any binary fails on a for-var assertion not caught in Step 1, update it the same way (parse-rejection → runtime non-fatal) and re-run. Do NOT weaken unrelated tests.

- [ ] **Step 6: Run the whole bash-diff harness suite**

Run:

```bash
for s in tests/scripts/*_diff_check.sh; do
  out=$(bash "$s" 2>&1); rc=$?
  printf '%s rc=%s %s\n' "$s" "$rc" "$(echo "$out" | tail -1)"
done | grep -v 'Fail: 0' || echo "ALL HARNESSES GREEN"
```

Expected: `ALL HARNESSES GREEN` (every harness ends `Fail: 0`).

- [ ] **Step 7: Parse-sweep payoff**

Run the parse sweep and confirm the cluster scripts now parse (report any that still fail on a *different* construct — a derail beyond this fix), and that LENIENT/CRASH stay 0:

```bash
tools/parse_sweep.sh tools/scripts.tsv 2>&1 | tail -15
# spot-check the named cluster scripts (substitute their paths from scripts.tsv):
for name in ethtool_rmon fcnal-test interop_test; do
  p=$(grep -m1 "$name" tools/scripts.tsv | cut -f2)
  [[ -n "$p" ]] && { echo "=== $name ($p) ==="; huck -n "$p"; echo "huck-n rc=$?"; bash -n "$p"; echo "bash-n rc=$?"; }
done
```

Expected: `HUCK_GAP` drops (report before/after vs the 29 baseline in `/tmp/v179_sweep_out.txt`); `HUCK_LENIENT` and `HUCK_CRASH` remain 0; the three named scripts now have `huck -n` rc matching `bash -n` rc 0 — OR fail on a clearly different construct, which you note as a follow-on (not a regression). Note: the gap metric is script-level, so a single cluster fix may move it only a little if scripts have stacked gaps — judge by the named scripts parsing, not the headline number.

- [ ] **Step 8: Commit**

```bash
git add tests/for_integration.rs
git commit -m "$(cat <<'EOF'
v180: update for-var test to runtime-validation behavior; regression pass

for_invalid_variable_name_is_nonfatal_runtime_error now asserts the runtime
"not a valid identifier" error (was the pre-fix parse "syntax error"), body
skipped, list continues. function_name_diff_check.sh `for a-b in` both_reject
still passes (huck rc 2->1, still nonzero). Full cargo test + all bash-diff
harnesses green; parse-sweep cluster (ethtool_rmon/fcnal-test/interop_test)
now parses.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Final review (whole-branch, before merge)

- [ ] **Step 1: Review the full diff**

Run: `git diff main...HEAD`
Confirm: only `for_variable_name`, `ForClause.var` doc, `is_valid_name` visibility, `run_for_inner` validation, the new harness, and the one updated integration test changed. No edits to `valid_identifier_text`, coproc-name handling, the arith-for `for ((;;))` path, or unrelated tests.

- [ ] **Step 2: Confirm the scope boundary held**

`valid_identifier_text` unchanged (still keyword+charset strict, still used by coproc); error-message wording stays the intentional `huck:`-prefix family; no `bash-divergences.md` change (this cluster was sweep-found, not a tracked `M-*`/`L-*` divergence).

- [ ] **Step 3: Hand off to finishing the branch**

Use the project merge ritual (AskUserQuestion before merging to main): merge `--no-ff`, push, delete the local branch, then record the iteration in `project_huck_iterations.md` + `MEMORY.md` (no `bash-divergences.md` entry to delete — sweep-found).

---

## Self-Review (plan vs spec)

**1. Spec coverage:**
- Parser parse-permissive (`command.rs:1382`) → Task 1 Step 3. ✓
- `ForClause.var` doc comment → Task 1 Step 4. ✓
- `is_valid_name` `pub(crate)` → Task 1 Step 2. ✓
- Runtime validation at top of `run_for` → Task 1 Step 5 (note: the real insertion point is `run_for_inner`, the inner fn that `run_for` wraps — verified in code). ✓
- `valid_identifier_text` / coproc untouched → Final review Step 2. ✓
- New harness `for_var_name_diff_check.sh` (stdout+exit, stderr discarded; keyword names run; non-identifier non-fatal; valid controls incl. `for x;` positionals and empty `for x in;`) → Task 2. ✓
- Parse-sweep re-run + named-script spot-check + GAP/LENIENT/CRASH report → Task 3 Step 7. ✓
- Full `cargo test` + UP-FRONT grep of tests/+src/ for old parse-rejection, notably `for_integration.rs` → Task 3 Steps 1-5. ✓
- All `*_diff_check.sh` green + clippy → Task 3 Step 6 + Task 1 Step 8. ✓
- No `bash-divergences.md` change; record in iterations + MEMORY → Final review Step 3. ✓

**2. Placeholder scan:** No TBD/TODO/"handle edge cases"/"similar to". Every code step shows complete code; every run step shows the command + expected output.

**3. Type consistency:** `for_variable_name(&Token) -> Option<String>` signature unchanged (both call sites at `command.rs:1509`/`:1563` unaffected). `is_valid_name(&str) -> bool` referenced as `crate::builtins::is_valid_name` from `executor.rs` — matches the `pub(crate)` made in Step 2. `ExecOutcome::Continue(1)` is the established non-fatal variant. Test renamed `for_invalid_variable_is_nonfatal_syntax_error` → `for_invalid_variable_name_is_nonfatal_runtime_error` (consistent within Task 3).

**Resolved during planning:** Two call sites (not one) route through `for_variable_name` — both covered by the unchanged signature. The `function_name_diff_check.sh:38` `both_reject` case does NOT break (verified: bash rc 1, huck rc 2→1, both nonzero) — Task 3 Step 4 confirms rather than edits it. The `for_integration.rs` test DOES break (asserts `"syntax error"`) and is updated in Task 3 Step 2.
