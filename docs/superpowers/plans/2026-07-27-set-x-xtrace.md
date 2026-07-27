# v339 — set-x xtrace fidelity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Flip the bash 5.2.21 test-suite `set-x` category from FAIL to a byte-identical PASS (runner PASS 26 → 27) by fixing three xtrace divergences.

**Architecture:** Three independent fixes in the xtrace subsystem: (1) preserve each arith-for header section's trailing whitespace in `trim_section`; (2) support `BASH_XTRACEFD` by resolving the target fd at each emit site; (3) trace standalone assignments with the real operator (`+=`/`=`) and the RHS this statement assigned (threaded out of the apply path via a transient `Shell` field, mirroring the existing `last_cmd_sub_status` pattern).

**Tech Stack:** Rust (crates `huck-syntax`, `huck-engine`), bash-vs-huck diff-check harnesses (`tests/scripts/*_diff_check.sh`), the bash test-suite runner (`tests/bash-test-suite/runner.sh`).

**Design reference:** `docs/superpowers/specs/2026-07-27-set-x-xtrace-design.md`. Issue: [#310](https://github.com/jdstanhope/huck/issues/310).

## Global Constraints

- **Branch:** all work on `v339-set-x-xtrace` (branch off `main`). Do NOT push to `main` or merge; hand the PR to the user.
- **Commit trailer:** every commit ends with `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- **Formatting:** run `cargo fmt --all` before every commit (CI enforces `cargo fmt --all --check`).
- **This box OOMs on `cargo test --workspace`.** ALWAYS test per-crate single-threaded: `cargo test -p <crate> --jobs 1 --lib -- --test-threads 1` (crates: `huck-syntax`, `huck-engine`). Build the binary with `cargo build -p huck` (debug) and `cargo build --release --locked --bin huck` (release). Guard diff-check sweeps with `ulimit -v 1500000` + `timeout`.
- **Bash source for the category runner** is at `/tmp/bash-5.2.21`; export `BASH_SOURCE_DIR=/tmp/bash-5.2.21` before invoking the runner.
- **Empirical bash rule (Root 1), verified vs bash 5.2.21:** each `for (( … ))` header section trace = `(( <section> ))` where `<section>` is the raw source text with **leading whitespace trimmed, trailing whitespace preserved**.
- **Empirical bash rule (Root 2):** a standalone assignment traces as `{name}{op}{xtrace_quote(rhs)}` where `op` is `+=` for append else `=`, and `rhs` is the RHS **value this statement assigned** (the expansion of the RHS word), NOT the full post-append variable contents.
- **Scope:** array/associative assignment-trace divergence (`a=($y $y)` printed literally by bash) is OUT of scope — file a follow-up issue in Task 4, do not fix here.

---

### Task 1: Root 1 — preserve arith-for section trailing whitespace

**Files:**
- Modify: `crates/huck-syntax/src/parser.rs` (`trim_section`, ~4534)
- Test: `crates/huck-syntax/src/generate.rs` (round-trip unit test, near existing arith-for tests ~1212/1416)
- Test: `tests/scripts/set_x_diff_check.sh` (extend)

**Interfaces:**
- Consumes: nothing from other tasks.
- Produces: nothing other tasks depend on (pure behavior fix). `trim_section` keeps its signature `fn trim_section(word: &Word) -> Option<Word>`.

**Background:** `trim_section` currently trims BOTH leading and trailing whitespace of the section's leading/trailing `Literal` parts. Bash trims leading only. The retained trailing whitespace flows through `reconstruct_word_source_inner`, fixing BOTH the xtrace (`run_arith_for_inner`) and `declare -f` (`generate.rs::arith_for_to_source`). Existing generate.rs tests use sources without a trailing space before `))` (`i++))`), so they are unaffected.

- [ ] **Step 1: Add a failing round-trip unit test in `generate.rs`**

Find the existing arith-for round-trip test block (near line 1212, `assert_rt("for ((i=0; i<3; i++)); do echo $i; done");`). Add, right after it:

```rust
// v339 (#310): a header section with trailing whitespace before `))`
// (`i++ ))`) must round-trip WITH the space — bash preserves trailing
// section whitespace (trims leading only).
assert_rt("for ((i=0; i<3; i++ )); do echo $i; done");
```

- [ ] **Step 2: Run the test; verify it FAILS**

Run: `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1 generate`
Expected: FAIL — huck currently reconstructs `for ((i=0; i<3; i++))` (trailing space dropped), so the round-trip does not match the input.

- [ ] **Step 3: Change `trim_section` to trim leading only**

In `crates/huck-syntax/src/parser.rs`, in `trim_section`, DELETE the trailing-Literal trimming block (the second `if let Some(WordPart::Literal ...) = parts.last()...` that does `text.trim_end()`), keeping only the leading-`trim_start` block. The function becomes:

```rust
fn trim_section(word: &Word) -> Option<Word> {
    let mut parts: Vec<WordPart> = word.0.clone();
    // Trim the leading Literal only. bash preserves each `for (( … ))` header
    // section's TRAILING whitespace verbatim (v339 #310) — it flows through
    // reconstruct_word_source_inner into both the xtrace and `declare -f`
    // output, matching bash `(( i++  ))` / `for ((…; i++ ))`. Arith evaluation
    // ignores trailing whitespace, so this is trace/reconstruction-only.
    if let Some(WordPart::Literal { text, quoted }) = parts.first().cloned() {
        let trimmed = text.trim_start().to_string();
        if trimmed.is_empty() {
            parts.remove(0);
        } else {
            parts[0] = WordPart::Literal {
                text: trimmed,
                quoted,
            };
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(Word(parts))
    }
}
```

- [ ] **Step 4: Run the unit test; verify it PASSES**

Run: `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1 generate`
Expected: PASS (including the new `assert_rt`), and NO other generate/parser test regresses.

- [ ] **Step 5: Extend `set_x_diff_check.sh` with arith-for spacing + declare -f cases**

In `tests/scripts/set_x_diff_check.sh`, add before the final `echo ""; echo "Total…"` line:

```bash
# v339 (#310) Root 1: arith-for section trace preserves trailing whitespace.
check "arith-for trailing sp"  'set -x; for ((i=0; i<=2; i++ )); do :; done'
check "arith-for no sp"        'set -x; for ((i=0;i<=2;i++)); do :; done'
check "arith-for all spaced"   'set -x; for ((i=0 ; i<=2 ; i++ )); do :; done'
# declare -f reconstruction shares the same section-trim path.
check "declare -f arith-for"   'f() { for ((i=0; i<=2; i++ )); do :; done; }; declare -f f'
```

- [ ] **Step 6: Build huck (debug) and run the harness; verify all cases PASS**

Run:
```bash
cargo build -p huck
bash tests/scripts/set_x_diff_check.sh
```
Expected: `Fail: 0` — all cases (including pre-existing ones) PASS.

- [ ] **Step 7: `cargo fmt --all` and commit**

```bash
cargo fmt --all
git add crates/huck-syntax/src/parser.rs crates/huck-syntax/src/generate.rs tests/scripts/set_x_diff_check.sh
git commit -m "$(cat <<'EOF'
v339: preserve arith-for section trailing whitespace in trace (#310)

trim_section trimmed both leading and trailing whitespace of each
for ((…)) header section; bash trims leading only and preserves trailing
whitespace verbatim. Trimming leading-only makes the retained trailing
space flow through reconstruct_word_source_inner into both the set -x
trace (`(( i++  ))`) and `declare -f` (`for ((…; i++ ))`), matching bash.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Root 3 — `BASH_XTRACEFD` support

**Files:**
- Modify: `crates/huck-engine/src/executor.rs` (`xtrace_emit` ~4215; new `xtrace_target_fd`; 7 call sites: 2677, 2740, 4244, 4357, 4714, 4759, 8871)
- Test: `tests/scripts/set_x_diff_check.sh` (extend)

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces: `fn xtrace_emit(fd: i32, line: &str)` and `fn xtrace_target_fd(shell: &Shell) -> i32` — Task 3 calls both from the assignment-trace block.

- [ ] **Step 1: Add a failing `BASH_XTRACEFD` case to `set_x_diff_check.sh`**

In `tests/scripts/set_x_diff_check.sh`, add before the final total line:

```bash
# v339 (#310) Root 3: BASH_XTRACEFD redirects xtrace to an fd; unset reverts.
check "BASH_XTRACEFD" 'tf=$(mktemp); exec 4>"$tf"; BASH_XTRACEFD=4; set -x; echo a; echo b; unset BASH_XTRACEFD; echo c; set +x; echo ---; cat "$tf"; rm -f "$tf"'
```

- [ ] **Step 2: Run the harness; verify the new case FAILS**

Run: `bash tests/scripts/set_x_diff_check.sh`
Expected: the `BASH_XTRACEFD` case FAILs — huck sends `+ echo a`/`+ echo b`/`+ unset BASH_XTRACEFD` to stderr inline instead of to fd 4 (so they appear before `---` and the file dump is empty).

- [ ] **Step 3: Add `xtrace_target_fd` and give `xtrace_emit` an fd parameter**

In `crates/huck-engine/src/executor.rs`, add near `xtrace_emit` (~4209):

```rust
/// Resolve the fd that `set -x` trace output goes to. bash's `BASH_XTRACEFD`,
/// when its value parses to a valid non-negative integer, is the xtrace
/// destination fd; unset / empty / non-numeric falls back to fd 2 (stderr).
/// Resolved at emit time (v339 #310) — no separate assign-time capture — which
/// naturally handles set→fd, `unset`→stderr, and invalid→stderr.
fn xtrace_target_fd(shell: &Shell) -> i32 {
    match shell.lookup_var("BASH_XTRACEFD") {
        Some(v) => v.trim().parse::<i32>().ok().filter(|&n| n >= 0).unwrap_or(2),
        None => 2,
    }
}
```

Change `xtrace_emit` to take the fd and write to it:

```rust
fn xtrace_emit(fd: i32, line: &str) {
    let mut buf = String::with_capacity(line.len() + 1);
    buf.push_str(line);
    buf.push('\n');
    let bytes = buf.as_bytes();
    unsafe {
        let _ = libc::write(fd, bytes.as_ptr() as *const libc::c_void, bytes.len());
    }
}
```

(Keep the existing doc comment about single-write atomicity; just add that `fd` is the resolved target.)

- [ ] **Step 4: Update all 7 `xtrace_emit` call sites to pass the resolved fd**

At each call site, resolve the fd from the `shell` already in scope and pass it first. The sites and their new forms:

- `~2677` (`eval_test_expr_traced`, `[[ … ]]`):
  ```rust
  xtrace_emit(xtrace_target_fd(shell), &format!("{p4}[[ {body} ]]"));
  ```
- `~2740` (`[[ ! … ]]`):
  ```rust
  xtrace_emit(xtrace_target_fd(shell), &format!("{p4}[[ ! {body} ]]"));
  ```
- `~4244` (`xtrace_compound`):
  ```rust
  xtrace_emit(xtrace_target_fd(shell), &format!("{p4}{body}"));
  ```
- `~4357` (standalone assignment trace in `run_assignment_list`):
  ```rust
  xtrace_emit(
      xtrace_target_fd(shell),
      &format!("{p4}{name}={}", crate::param_expansion::xtrace_quote(&val)),
  );
  ```
  (Task 3 rewrites the body of this block; here only add the fd arg so it compiles.)
- `~4714` (inline-assignment prefix trace):
  ```rust
  xtrace_emit(
      xtrace_target_fd(shell),
      &format!("{p4}{name}={}", crate::param_expansion::xtrace_quote(&val)),
  );
  ```
- `~4759` (simple-command trace):
  ```rust
  xtrace_emit(xtrace_target_fd(shell), &format!("{p4}{body}"));
  ```
- `~8871` (command trace):
  ```rust
  xtrace_emit(
      xtrace_target_fd(shell),
      &format!("{p4}{}", xtrace_command_line(&[], &resolved.program, &resolved.args)),
  );
  ```

Verify with `grep -n 'xtrace_emit(' crates/huck-engine/src/executor.rs` that every call now passes an fd as the first argument and there are no remaining single-argument calls.

- [ ] **Step 5: Build and run the harness; verify the `BASH_XTRACEFD` case PASSES**

Run:
```bash
cargo build -p huck
bash tests/scripts/set_x_diff_check.sh
```
Expected: `Fail: 0` — the `BASH_XTRACEFD` case now matches bash (trace lines land in the file, dumped after `---`; `+ echo c`/`+ set +x` on stderr).

- [ ] **Step 6: Per-crate compile/test check for huck-engine**

Run: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 xtrace`
Expected: PASS (compiles; existing xtrace unit tests unaffected — they use the default stderr path where `BASH_XTRACEFD` is unset → fd 2).

- [ ] **Step 7: `cargo fmt --all` and commit**

```bash
cargo fmt --all
git add crates/huck-engine/src/executor.rs tests/scripts/set_x_diff_check.sh
git commit -m "$(cat <<'EOF'
v339: support BASH_XTRACEFD for set -x output (#310)

xtrace_emit hardcoded write(2). Add xtrace_target_fd(shell) — read
BASH_XTRACEFD, use it when it parses to a valid non-negative fd, else
stderr — and thread the resolved fd through all 7 emit sites. Resolved
at emit time so set→fd / unset→stderr / invalid→stderr all work.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: Root 2 — trace standalone assignments with real operator + RHS

**Files:**
- Modify: `crates/huck-engine/src/shell_state.rs` (new transient field + setter/taker, near `last_cmd_sub_status` ~654/3123)
- Modify: `crates/huck-engine/src/executor.rs` (`apply_one_assignment` bare-scalar branch ~8122; `run_assignment_list` trace block ~4354)
- Test: `tests/scripts/set_x_diff_check.sh` (extend)

**Interfaces:**
- Consumes: `xtrace_emit(fd, line)` and `xtrace_target_fd(shell)` from Task 2.
- Produces: transient `Shell` methods `set_xtrace_assign_rhs(&mut self, v: Option<String>)` and `take_xtrace_assign_rhs(&mut self) -> Option<String>` (returns and clears).

**Background:** the current trace at `executor.rs:4354` emits `name=<lookup_var value>` — wrong operator (`=`) and wrong value (full post-append) for `+=`. The bare-scalar branch of `apply_one_assignment` (`executor.rs:8123`) already computes the exact RHS bash traces: `let s = expand_assignment(&a.value, shell)`. Thread `s` out via a transient field (mirroring `last_cmd_sub_status`), reset per-assignment before apply and read in the trace block; fall back to `lookup_var` for non-scalar targets (array/assoc — deferred).

- [ ] **Step 1: Add failing `+=` cases to `set_x_diff_check.sh`**

In `tests/scripts/set_x_diff_check.sh`, add before the final total line:

```bash
# v339 (#310) Root 2: standalone assignment trace shows the operator (+=/=) and
# the RHS this statement assigned, not the full post-append value.
check "trace plain assign"     'set -x; x=hi'
check "trace append assign"    'set -x; foo=one; foo+=two'
check "trace append expand"    'y=world; set -x; foo=hello; foo+=" $y"'
```

- [ ] **Step 2: Run the harness; verify the append cases FAIL**

Run: `bash tests/scripts/set_x_diff_check.sh`
Expected: `trace append assign` and `trace append expand` FAIL — huck emits `+ foo=onetwo` / `+ foo='hello world'` where bash emits `+ foo+=two` / `+ foo+=' world'`. (`trace plain assign` already PASSES.)

- [ ] **Step 3: Add the transient field to `Shell`**

In `crates/huck-engine/src/shell_state.rs`:

Add the field to `struct Shell` (near `last_cmd_sub_status`, ~654):
```rust
    /// The scalar RHS value the most recent bare-scalar `apply_one_assignment`
    /// assigned (the expansion of the RHS word). Set by that path, read+cleared
    /// by `run_assignment_list`'s `set -x` trace so the trace shows the RHS this
    /// statement assigned (bash `foo+=two`), not the full variable value.
    /// v339 (#310); mirrors `last_cmd_sub_status`.
    xtrace_assign_rhs: Option<String>,
```

Initialize it in the constructor(s) where `last_cmd_sub_status: None,` is set (~1103):
```rust
            xtrace_assign_rhs: None,
```

Add methods near `set_last_cmd_sub_status`/`last_cmd_sub_status` (~3123):
```rust
    pub(crate) fn set_xtrace_assign_rhs(&mut self, v: Option<String>) {
        self.xtrace_assign_rhs = v;
    }
    pub(crate) fn take_xtrace_assign_rhs(&mut self) -> Option<String> {
        self.xtrace_assign_rhs.take()
    }
```

- [ ] **Step 4: Record the RHS in the bare-scalar apply branch**

In `crates/huck-engine/src/executor.rs`, in `apply_one_assignment`, the bare-scalar branch `(AssignTarget::Bare(name), None) => {` (~8122). Immediately after `let s = expand_assignment(&a.value, shell);` (~8123), record it for the trace:

```rust
            let s = expand_assignment(&a.value, shell);
            // Record the RHS this statement assigns so the `set -x` trace in
            // run_assignment_list can show `name+=rhs` / `name=rhs` (v339 #310).
            shell.set_xtrace_assign_rhs(Some(s.clone()));
```

(Leave the rest of the branch unchanged — `s` is still consumed by the append/non-append logic below.)

- [ ] **Step 5: Rewrite the assignment trace block in `run_assignment_list`**

In `run_assignment_list`, reset the transient field before each apply. Change the apply call site (~4341) from:
```rust
        if apply_one_assignment(a, shell, &mut *err_writer(err_sink, sink)).is_err() {
```
to:
```rust
        shell.set_xtrace_assign_rhs(None);
        if apply_one_assignment(a, shell, &mut *err_writer(err_sink, sink)).is_err() {
```

Then replace the trace block (~4354–4361) with:
```rust
        if shell.shell_options.xtrace {
            let op = if a.append { "+=" } else { "=" };
            // Bare-scalar apply recorded the assigned RHS; array/assoc/indexed
            // targets don't — fall back to the full value for those (their
            // literal-source trace is a separate deferred divergence, #310).
            // `match` (not `unwrap_or_else`) so the mut borrow from `take_…`
            // fully ends before the `lookup_var` shared borrow.
            let val = match shell.take_xtrace_assign_rhs() {
                Some(rhs) => rhs,
                None => shell.lookup_var(name).unwrap_or_default(),
            };
            let p4 = ps4(shell);
            xtrace_emit(
                xtrace_target_fd(shell),
                &format!("{p4}{name}{op}{}", crate::param_expansion::xtrace_quote(&val)),
            );
        }
```

- [ ] **Step 6: Build and run the harness; verify all cases PASS**

Run:
```bash
cargo build -p huck
bash tests/scripts/set_x_diff_check.sh
```
Expected: `Fail: 0` — `trace append assign` → `+ foo+=two`, `trace append expand` → `+ foo+=' world'`, `trace plain assign` → `+ x=hi`.

- [ ] **Step 7: Per-crate test for huck-engine**

Run: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`
Expected: PASS — no assignment/xtrace unit test regresses.

- [ ] **Step 8: `cargo fmt --all` and commit**

```bash
cargo fmt --all
git add crates/huck-engine/src/shell_state.rs crates/huck-engine/src/executor.rs tests/scripts/set_x_diff_check.sh
git commit -m "$(cat <<'EOF'
v339: trace standalone assignments with real operator + RHS (#310)

The set -x trace emitted `name=<full value>`, wrong for `+=` (bash shows
`foo+=two`, the RHS this statement assigned, not `foo=onetwo`). Thread
the scalar RHS the bare-scalar apply computed out via a transient Shell
field (mirrors last_cmd_sub_status) and emit `name{+=|=}rhs`. Array/assoc
targets keep the old full-value fallback (separate deferred divergence).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: Verify the category flip, guard regressions, update docs + memory

**Files:**
- Modify: `docs/bash-test-suite-baseline.md` (baseline note + Summary + `set-x` row)
- Modify: `/home/john/.claude/projects/-home-john-projects-huck/memory/project_huck_iterations.md`
- Modify: `/home/john/.claude/projects/-home-john-projects-huck/memory/MEMORY.md`

**Interfaces:** none (verification + docs).

- [ ] **Step 1: Build the release binary**

Run: `cargo build --release --locked --bin huck`
Expected: builds clean.

- [ ] **Step 2: Run the `set-x` category runner; verify 0-diff PASS**

Run:
```bash
BASH_SOURCE_DIR=/tmp/bash-5.2.21 HUCK_BASH_TEST_CATEGORY=set-x bash tests/bash-test-suite/runner.sh
```
Expected: the summary line shows `| set-x | PASS |`. If FAIL, inspect the fresh `/tmp/huck-bash-tests-*/set-x.diff` — it must be empty; any residual line points back to Root 1/2/3.

- [ ] **Step 3: Confirm no neighbor category regressed**

Run the runner on the categories most likely to touch xtrace / arith-for / declare -f:
```bash
export BASH_SOURCE_DIR=/tmp/bash-5.2.21
for c in arith-for set-x parser posix2 dbg-support2 func cprint herestr; do
  HUCK_BASH_TEST_CATEGORY=$c bash tests/bash-test-suite/runner.sh 2>/dev/null | grep -E "^\| $c "
done
```
Expected: `set-x` now PASS; every other listed category holds its prior status (the PASS ones — `posix2`, `dbg-support2`, `func`, `cprint`, `herestr` — stay PASS; `arith-for`, `parser` stay FAIL, not worse).

- [ ] **Step 4: Full diff-check sweep (regression guard)**

Run:
```bash
cargo build -p huck
cargo build --release --locked --bin huck
( ulimit -v 1500000; timeout 600 bash tests/scripts/run_diff_checks.sh )
```
Expected: all harnesses PASS (green), including `set_x_diff_check.sh`, `setx_trace_fidelity_diff_check.sh`, `xtrace_compound_diff_check.sh`, `declare_f_diff_check.sh`, and any arith-for harness.

- [ ] **Step 5: Run the touched integration binaries single-threaded**

Run (each guarded against OOM):
```bash
for t in set_x_integration setx_trace_fidelity_integration arith_for_integration declare_f_integration; do
  ( ulimit -v 1500000; cargo test -p huck --test $t --jobs 1 -- --test-threads 1 ) || echo "FAILED: $t"
done
```
Expected: all PASS, no `FAILED:` line.

- [ ] **Step 6: Update `docs/bash-test-suite-baseline.md`**

Add a dated `**Updated by v339 (#310, 2026-07-27 UTC):**` note at the top (mirroring the v338 note's style) recording: `set-x` flipped to PASS (0-diff); the three roots (arith-for section trailing whitespace, append-assignment operator+RHS trace, `BASH_XTRACEFD`); Summary PASS 26→27, FAIL 56→55; only `set-x` flipped, no regressions. Update the `## Summary` counts (PASS 26→27, FAIL 56→55). Replace the `| set-x | FAIL | … |` row with a `PASS` row summarizing the fix. Note in the row that Root 1 also aligned `declare -f` arith-for reconstruction, and that the array/assoc assignment-trace divergence is a deferred follow-up.

- [ ] **Step 7: File the deferred follow-up issue**

Run:
```bash
gh issue create --label divergence --label bug --label sev:low \
  --title "set -x traces array/associative assignments as garbage, not literal source" \
  --body "Under \`set -x\`, bash traces an array assignment as its literal unexpanded source (\`a=(\$y \$y)\`, \`b+=(3 4)\`); huck traces the first element's value (\`a=world\`, \`b=1\`). The standalone-assignment trace in run_assignment_list only threads out the scalar RHS (v339 #310); array/assoc/indexed targets fall back to lookup_var. Fix by tracing the reconstructed literal RHS source for array-literal / indexed targets. Not exercised by the set-x category (scalar-only there)."
```
Record the new issue number for the baseline row / memory note.

- [ ] **Step 8: Update memory files**

Append a v339 entry to `project_huck_iterations.md` (newest at top) and add the one-line v339 hook to the top of `MEMORY.md`'s iteration list, in the established style: FLIPS `set-x` 26→27; the three roots; the durable lessons (bash preserves arith-for section TRAILING whitespace — one `trim_section` change fixes trace AND `declare -f`; `BASH_XTRACEFD` resolved at emit time; scalar RHS threaded out via a transient `Shell` field mirroring `last_cmd_sub_status`); follow-up issue number from Step 7.

- [ ] **Step 9: Commit docs + memory**

```bash
git add docs/bash-test-suite-baseline.md
git commit -m "$(cat <<'EOF'
v339: baseline — set-x flipped to PASS (26->27) (#310)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```
(Memory files live outside the repo — save them via the Write tool, not git.)

---

## Final review & PR (after all tasks)

- [ ] Review the whole branch diff (`git diff main...v339-set-x-xtrace`) for stray edits, leftover debug, and formatting.
- [ ] Confirm `cargo fmt --all --check` is clean and a fresh `cargo build --workspace --locked` (build only, NOT test) succeeds.
- [ ] Push `v339-set-x-xtrace` and open a PR targeting `main` with body `Closes #310`, a summary of the three roots, and the verification evidence (category runner PASS, sweep green). Hand to the user to review/merge; wait for CI to finish green before calling it ready (do NOT self-merge).
