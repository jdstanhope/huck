# Redirect diagnostics & fd fidelity (cluster A) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make huck's redirect error diagnostics and `{var}` fd numbering byte-identical to bash 5.2.21 for the cases the fd-management audit flagged — closing #152, #140, #141.

**Architecture:** Two independent change-sets in the redirect-lowering + error-emit path (`crates/huck-engine/src/executor.rs`, `expand.rs`): (1) **emit-only** message wording (thread the un-expanded source word via `reconstruct_word_source` into the ambiguous-redirect + dup-failure diagnostics, plus bash's extra `{var}>&badfd` line); (2) **fd-topology** — a virtual `{var}` allocator in the child plan so the number matches bash when a file redirect precedes it, which also makes the `{var}` assignment visible to later sibling redirects during batch lowering (external source-order `$v`-visibility).

**Tech Stack:** Rust; bash-diff harnesses (`tests/scripts/*_diff_check.sh`, `tools/redirect_audit.sh`).

## Global Constraints

- **Reference spec:** `docs/superpowers/specs/2026-07-15-redirect-diagnostics-fd-fidelity-design.md`. If code contradicts it, STOP and report.
- **Byte-identical to bash 5.2.21** for every targeted case (message text, ordering, `line N:` prefixing, and `{var}` fd number). "Close enough" on message text is a failure.
- **Primary gate:** `tools/redirect_audit.sh` (157 cases, **16 DIVERGE** baseline) must strictly DECREASE; report the exact before/after DIVERGE set. Any of the 16 that are NOT cluster-A (tracked by other issues) stay divergent and MUST be listed so the count is honest — do not claim more than cluster-A resolves.
- **Behavior-preserving elsewhere:** message changes must not alter rc or which command runs; the fd-numbering change must not alter redirect success/failure — only the number. No currently-passing sweep case may regress.
- **Test discipline (this box OOMs on `cargo test --workspace` — NEVER run it):** build `cargo build -p huck`; engine lib `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1` (~1806); integration `( ulimit -v 6000000; cargo test -p huck --test <name> --jobs 1 -- --test-threads 1 )`; sweep `( ulimit -v 1500000; timeout 1100 tests/scripts/run_diff_checks.sh )` after `cargo build --release --locked -p huck` (release ~2.5 min — allow it).
- **`cargo fmt --all`** before each commit. Commit trailer VERBATIM: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- **Line numbers below are approximate anchors — VERIFY by reading before editing.** Stale LSP diagnostics are false; trust only `cargo build -p huck`.
- **Scope guard:** keep these targeted. Do NOT fold in the deferred error-model divergences ([[huck-param-expansion-debt]]) or the `>&<non-numeric>` file-redirect semantics (bash treats `>&foo` as a file redirect — out of scope). If a case needs the error-model rework, STOP and split it to a follow-up issue.

## Current-code landmarks (verify by reading)

- `reconstruct_word_source(word: &Word) -> String` — `expand.rs:1474` (renders a Word to source text).
- `expand_single(word, shell, err)` — `executor.rs:3023`; ambiguous-redirect emit at `3034`; second bare site `~4858`; already-named site `~4716` (`{name}: ambiguous redirect`).
- `resolve_fd_target(source, shell)` — `executor.rs:3042` (expands+parses a dup source; errors `bad fd: {expanded}`).
- `resolve_dup_source(source, shell, sink, err_sink)` — `executor.rs:3056` (emits the dup-source error).
- `validate_fd_open` / `validate_plan_source` / `validate_source` — `executor.rs:3075/3096/3119` (emit `"{src}: Bad file descriptor"` with the NUMERIC fd).
- `lower_one_redirect` — `executor.rs:4698`; the `RedirFd::Var` (`{var}`) block `4708-4850`; the `{var}` dup-fail emit `4821-4830`; `dup_to_high_fd(raw_src, 10, false)` `4817`; `PlanOp::NamedFd { high, name }` push `4836`.
- `apply_one` `PlanOp::NamedFd` (in-process replay) — `executor.rs:1047` (`shell.set(name, high_fd)`).
- `lower_redirects` (batch) → `redir_plan_to_child` — `build_child_redir_plan` `executor.rs:5040`; `redir_plan_to_child` `5055`; its `NamedFd` arm `5076-5083` (emits `ChildRedirOp::Dup { target: raw, source: raw }` + holds `high` — **this is #141's real-number bug**).
- `ChildRedirOp` enum + `replay_redir_ops` — `executor.rs:4568/4584`.
- Differential audits: `tools/redirect_audit.sh` (16 DIVERGE), `pipeline_redirect_audit.sh` (15/15), `fd_torture_diff_check.sh` (44).

---

### Task 1: RED gates — message diff-check + fd-numbering harness

**Files:**
- Create: `tests/scripts/redirect_diag_diff_check.sh` (message cases; byte-identical `check`).
- Create: `tests/scripts/named_fd_number_diff_check.sh` (child `{var}` fd number via `/dev/fd`; Linux-gated).

**Interfaces:** Produces two harnesses auto-discovered by `run_diff_checks.sh` (glob `*_diff_check.sh`). RED on current code.

- [ ] **Step 1: Write the message harness.** `tests/scripts/redirect_diag_diff_check.sh` — mirror the house style of `tests/scripts/pipeline_stage_redirect_fail_diff_check.sh` (a `check()` that compares `bash -c FRAG 2>&1` vs `huck -c FRAG 2>&1` byte-identically; `HUCK=target/debug/huck`; exit non-zero on any FAIL). **The `norm` MUST strip BOTH the `line N:` form AND the bare program-name prefix** (bash's `{var}>&badfd` first line has no `line N:`), for both shells:
```bash
norm() { sed -e 's#^bash: line [0-9]*: #SH: #' -e 's#^bash: #SH: #' \
             -e "s#^$HUCK: line [0-9]*: #SH: #" -e "s#^$HUCK: #SH: #"; }
```
Cases (each a separate `check`):
```
# #152 — name the offending word in ambiguous redirect
amb-out       cat >$(echo a b)
amb-in        cat <$(echo a b)
# #140a — {var}>&badfd double message
var-dup-bad   {v}>&9
# #140b — >&$v echoes the literal word, not the resolved number
var-echo-word exec {v}>f; exec {v}>&-; echo x >&$v
# #140c — dup source $v unset -> "$v: ambiguous redirect"
amb-unset     2>&$v {v}>f
amb-unset-pl  2>&$v {v}>f | cat
# #140d — external/pipeline source-order $v-visibility: assign-then-use succeeds
ext-vis-true  true {v}>f 2>&$v | cat
ext-vis-echo  echo hi {v}>f 2>&$v | cat
```
Add `rm -f f 2>/dev/null` cleanup after cases that create `f`. NOTE the ordering cases: `2>&$v {v}>f` (use-before-assign → error) vs `true {v}>f 2>&$v` (assign-then-use → success) MUST both be present — they pin the source-order rule.

- [ ] **Step 2: Write the fd-numbering harness.** `tests/scripts/named_fd_number_diff_check.sh`: gate on Linux (`[ "$(uname)" = Linux ] || { echo "SKIP (needs /proc/self/fd)"; exit 0; }`). For each fragment, capture the `{var}` child fd number from BOTH shells and compare. Use a helper that lists the child's own fds and extracts the `{var}` fd (the one pointing at the created file `x`):
```bash
child_named_fd() {  # $1 = shell binary, $2 = leading redirects before {v}>x
  "$1" -c "ls -l /proc/self/fd $2 {v}>x 2>/dev/null" 2>/dev/null \
    | grep -oE '/proc/self/fd/[0-9]+ -> .*/x$' | grep -oE '/fd/[0-9]+' | grep -oE '[0-9]+'
}
```
Cases: `""` (bare `{v}>x` → 10), `"3>a"` (→ bash 10), `"3>a 4>b"` (→ bash 10). Compare `child_named_fd bash "$c"` vs `child_named_fd target/debug/huck "$c"`; FAIL on mismatch. `rm -f a b x` cleanup.

- [ ] **Step 3: Build + confirm both harnesses RED.**
```bash
cargo build -p huck
bash tests/scripts/redirect_diag_diff_check.sh   # expect FAILs on amb-*, var-*, ext-vis-*
bash tests/scripts/named_fd_number_diff_check.sh # expect FAIL on 3>a / 3>a 4>b (huck 11/12 vs bash 10)
```
Eyeball one FAIL body from each and confirm it captures the real divergence (huck's message missing the word / huck fd 11 vs bash 10). If a case unexpectedly PASSES, investigate before proceeding.

- [ ] **Step 4: Confirm the primary audit baseline.**
```bash
HUCK="$(pwd)/target/debug/huck" bash tools/redirect_audit.sh | grep '^AUDIT'   # 16 DIVERGE
```

- [ ] **Step 5: Commit (RED gate).**
```bash
chmod +x tests/scripts/redirect_diag_diff_check.sh tests/scripts/named_fd_number_diff_check.sh
git add tests/scripts/redirect_diag_diff_check.sh tests/scripts/named_fd_number_diff_check.sh
git commit -m "$(cat <<'EOF'
v297 T1: RED gates for redirect diagnostics + {var} fd numbering (#152/#140/#141)

New redirect_diag_diff_check.sh (ambiguous-redirect word, {var}>&badfd double
message, >&$v word-echo, $v-unset ambiguous, external source-order $v-visibility)
and named_fd_number_diff_check.sh (child {var} fd number via /dev/fd, Linux-gated)
— both RED. Tasks 2-3 turn them green.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Message wording — #152, #140a, #140b, #140c (emit-only)

**Files:** Modify `crates/huck-engine/src/executor.rs`.

**Interfaces:**
- Consumes: `reconstruct_word_source` (expand.rs:1474) — import/qualify as `crate::expand::reconstruct_word_source`.
- Produces: the `redirect_diag_diff_check.sh` `amb-*` and `var-*` cases GREEN (the `ext-vis-*` cases stay RED until Task 3).

- [ ] **Step 1: #152 — name the word in `ambiguous redirect`.** In `expand_single` (`executor.rs:3034`) change the emit to carry the source word:
```rust
        crate::sh_error_to!(
            shell,
            err,
            None,
            "{}: ambiguous redirect",
            crate::expand::reconstruct_word_source(word)
        );
```
Apply the identical change at the second bare site (`~4858`). Leave the already-named `{name}: ambiguous redirect` site (`~4716`) as-is.

- [ ] **Step 2: #140c — dup source that expands empty / ≠1 field → `"{word}: ambiguous redirect"`.** `resolve_fd_target` (`3042`) currently returns `bad fd: {expanded}` for any parse failure. Split its contract so the *dup-source* path can distinguish ambiguous (0 or >1 fields) from a bad numeric. Change `resolve_dup_source` (`3056`) to expand the word to fields itself and branch (do NOT change the single-command file path):
```rust
fn resolve_dup_source(
    source: &crate::lexer::Word,
    shell: &mut Shell,
    sink: &mut StdoutSink,
    err_sink: &mut StderrSink,
) -> Result<RawFd, ()> {
    // bash: a dup source that word-splits to 0 or >1 fields (e.g. `$v` unset,
    // `>&` empty) is an *ambiguous redirect* naming the raw word; a single
    // non-numeric field is `bad fd`.
    let fields = expand(source, shell);
    let word_src = crate::expand::reconstruct_word_source(source);
    if fields.len() != 1 {
        let mut err = err_writer(err_sink, sink);
        crate::sh_error_to!(shell, &mut *err, None, "{word_src}: ambiguous redirect");
        return Err(());
    }
    match fields.into_iter().next().unwrap().chars.parse::<i32>() {
        Ok(fd) => Ok(fd),
        Err(_) => {
            let mut err = err_writer(err_sink, sink);
            crate::sh_error_to!(shell, &mut *err, None, "bad fd: {word_src}");
            Err(())
        }
    }
}
```
Keep `resolve_fd_target` for any non-dup callers (grep — if `resolve_dup_source` is the only caller of `resolve_fd_target`, inline/remove it; otherwise leave it). Verify the `amb-unset` / `amb-unset-pl` cases now match bash (`$v: ambiguous redirect`).

- [ ] **Step 3: #140b — `>&$v` (resolves to a number, fd closed) echoes the raw word.** The not-open error is emitted by `validate_source`/`validate_fd_open`/`validate_plan_source` (`3075/3096/3119`) with the numeric `src`. For a *dup source that came from a word*, bash echoes the word. Thread the source word's rendered text to the dup-validation error. Add an optional word-label parameter to the validate chain (or, narrower: at the `lower_one_redirect` dup/move call to `validate_source` at `4810`, on `Err` re-emit with the word). Cleanest: give `validate_source` an extra `label: &str` used in place of `{src}`:
```rust
// call site (lower_one_redirect ~4810), dup/move source only:
if let Some(s) = dup_src {
    validate_source(s, fd_state.as_deref(), shell, sink, err_sink,
        &crate::expand::reconstruct_word_source(dup_source_word))?;
}
```
where `dup_source_word` is the `source` Word from the `RedirOp::Dup { source, .. } | RedirOp::Move { source, .. }` arm (thread it out of the match). In `validate_fd_open`/`validate_plan_source`, replace `"{src}: Bad file descriptor"` with `"{label}: Bad file descriptor"`. **Check the other `validate_source` callers** (in-process apply path) — they must pass a label too; for a numeric literal source (`>&9`) the label IS the number, so pass `s.to_string()` there, preserving today's output. Confirm `var-echo-word` now prints `$v: Bad file descriptor`.

- [ ] **Step 4: #140a — `{var}>&<badfd>` double message.** For a `{var}` redirect whose dup source is a bad fd, bash emits TWO lines: `redirection error: cannot duplicate fd: <strerror>` (NO `line N:` prefix) then `line N: <fd>: Bad file descriptor`. In `lower_one_redirect`'s `RedirFd::Var` block, the dup-source validation happens at `4809-4811` (`validate_source`). When the source is a `Dup`/`Move` under a `{var}` fd and validation fails, first emit the extra line, then let the standard `<fd>: Bad file descriptor` follow. Implement by wrapping the `{var}` dup-validation:
```rust
if let Some(s) = dup_src {
    if validate_source_is_open(s, fd_state.as_deref()) == false {
        // bash's extra leading line for a {var} dup of a bad fd (no line prefix).
        {
            let mut err = err_writer(err_sink, sink);
            crate::sh_error_to!(
                shell, &mut *err, None,
                "redirection error: cannot duplicate fd: {}",
                std::io::Error::from_raw_os_error(libc::EBADF)
            );
        }
        // then the standard "<fd>: Bad file descriptor"
        validate_source(s, fd_state.as_deref(), shell, sink, err_sink, &s.to_string())?;
        // (validate_source re-checks and emits the second line, then Err)
    }
}
```
(Add a small `validate_source_is_open(src, fd_state) -> bool` predicate that mirrors `validate_plan_source`/`validate_fd_open`'s open-check without emitting, to avoid double-checking races.) Confirm the `sh_error_to!` for the FIRST line does NOT add a `line N:` prefix (check how `sh_error_to!` formats — if it always prefixes, use `emit_error_to`/the raw writer form that matches bash's un-prefixed `redirection error: …`). Verify `var-dup-bad` matches bash's two lines exactly.

- [ ] **Step 5: Build + drive the message cases green.**
```bash
cargo build -p huck 2>&1 | tail -5   # warning-clean
bash tests/scripts/redirect_diag_diff_check.sh   # amb-* and var-* PASS; ext-vis-* still FAIL (Task 3)
```
The two `ext-vis-*` cases and `named_fd_number` stay RED — expected. If any `amb-*`/`var-*` case still fails, STOP and report the bash-vs-huck diff; do not weaken the harness.

- [ ] **Step 6: No-regression check.**
```bash
HUCK="$(pwd)/target/debug/huck" bash tools/redirect_audit.sh | grep '^AUDIT'   # DIVERGE < 16
HUCK="$(pwd)/target/debug/huck" bash tools/pipeline_redirect_audit.sh | grep '^AUDIT'  # 15/15
tests/scripts/fd_torture_diff_check.sh | tail -1   # 44
cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 2>&1 | tail -2   # ~1806
```

- [ ] **Step 7: Commit.**
```bash
cargo fmt --all
git add crates/huck-engine/src/executor.rs
git commit -m "$(cat <<'EOF'
v297 T2: redirect diagnostic wording matches bash (#152, #140a/b/c)

Name the un-expanded word in `ambiguous redirect` (#152); a dup source that
word-splits to 0/>1 fields is `<word>: ambiguous redirect` not `bad fd` (#140c);
`>&$v` bad-fd error echoes the raw word not the resolved number (#140b); a
`{var}>&<badfd>` emits bash's leading `redirection error: cannot duplicate fd`
line before `<fd>: Bad file descriptor` (#140a). Emit-only; fd topology
unchanged. redirect_audit DIVERGE drops; message diff-check cases green.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: `{var}` virtual fd allocation + external source-order `$v`-visibility — #141, #140d

**Files:** Modify `crates/huck-engine/src/executor.rs`.

**Interfaces:**
- Produces: `named_fd_number_diff_check.sh` GREEN + the `ext-vis-*` cases in `redirect_diag_diff_check.sh` GREEN.
- Consumes: the Task 2 `resolve_dup_source`.

**Design (read `lower_redirects`/`lower_one_redirect`/`redir_plan_to_child` first).** bash processes redirects left-to-right in the child: `{v}>f` opens `f`, dup2s it to the lowest free fd ≥10 (→ 10), and sets `$v=10`; a later `2>&$v` then dup2s fd 10 → fd 2. huck's child path today parks the file at the real high fd (11/12 when earlier file redirects hold 10) and leaves the `{var}` there, and never makes `$v` visible to a later sibling during batch lowering. Fix both with a **virtual `{var}` number assigned during batch lowering**:

- [ ] **Step 1: Assign a virtual `{var}` number during batch lowering and make it visible to siblings.** In the batch path only (`lower_one_redirect` called with `fd_state = Some(...)`, i.e. the child plan — the in-process `None` path already interleaves correctly and must NOT change), when lowering a `{var}` op:
  - Compute `virtual_fd` = the lowest number ≥10 that is (a) not a `target` of any earlier `PlanOp` in this plan, and (b) not an earlier `{var}` virtual number in this plan. Track allocated virtual numbers in a small set threaded alongside `fd_state` (add a field to the plan-building state, or a `&mut Vec<RawFd>`/`&mut HashMap` parameter). The parked `owned_src` high fd (`high` from `dup_to_high_fd`) stays the *source*; `virtual_fd` is the *destination*.
  - Emit the plan so child replay does `dup2(high → virtual_fd)` then `close(high)` (unless `virtual_fd == high`). Represent this by changing `PlanOp::NamedFd` to carry the virtual target, e.g. `PlanOp::NamedFd { high: OwnedFd, name: String, virtual_fd: RawFd }`, and update BOTH replays:
    - **child** (`redir_plan_to_child` `5076`): push `ChildRedirOp::Dup { target: virtual_fd, source: high.as_raw_fd() }`, then `ChildRedirOp::Close { target: high.as_raw_fd() }` if `virtual_fd != high_raw`; `held.push(high)` (keep the source alive until fork). Record `virtual_fd` as a used target for later virtual/target computation.
    - **in-process** (`apply_one` `1047`): UNCHANGED behavior — the in-process path assigns `$v = high` (its interleaved real-fd number, which already matches bash there). Only the child path uses `virtual_fd`. (If threading a `virtual_fd` field forces a value here, set it to `high` for the in-process build so nothing changes.)
  - **Make `$v` visible to later siblings on the child path:** after computing `virtual_fd`, set `$v` in the shell to `virtual_fd.to_string()` DURING batch lowering so a later sibling `2>&$v`'s `resolve_dup_source` resolves to it. Because bash does NOT persist `{var}` to the parent for an external command, SNAPSHOT `$v`'s prior value and RESTORE it after the plan is built (i.e. in `build_child_redir_plan` / the batch caller, save the pre-existing values of every `{var}` name and restore them after `lower_redirects` returns — mirror the existing inline-assignment snapshot/restore pattern). Verify: `true {v}>f 2>&$v | cat` → child dup2(10→2); `$v` NOT set in the parent afterward (`echo ${v-unset}` after the command prints `unset` in both shells).

- [ ] **Step 2: Build.**
```bash
cargo build -p huck 2>&1 | tail -5   # warning-clean
```

- [ ] **Step 3: Drive the fd-numbering + external-visibility cases green.**
```bash
bash tests/scripts/named_fd_number_diff_check.sh   # all PASS (bash 10 == huck 10 for 3>a / 3>a 4>b)
bash tests/scripts/redirect_diag_diff_check.sh      # ALL cases PASS now (incl. ext-vis-*)
```
Direct confirmations vs bash:
```bash
for s in bash target/debug/huck; do echo "-- $s"; $s -c 'ls /proc/self/fd 3>a {v}>x' 2>/dev/null | tr '\n' ' '; echo; done  # {var} fd = 10 both
target/debug/huck -c 'true {v}>f 2>&$v | cat; echo rc=$?'   # silent, rc=0, matches bash
target/debug/huck -c '{v}>f 2>&$v true; echo ${v-unset}'    # {var} does not persist externally where bash doesn't
rm -f a b x f
```
If a case still diverges, STOP and report — do not weaken the harness.

- [ ] **Step 4: Full verification.**
```bash
HUCK="$(pwd)/target/debug/huck" bash tools/redirect_audit.sh | grep '^AUDIT'          # DIVERGE reduced further
HUCK="$(pwd)/target/debug/huck" bash tools/pipeline_redirect_audit.sh | grep '^AUDIT'  # 15/15 unchanged
tests/scripts/fd_torture_diff_check.sh | tail -1                                       # 44 unchanged
cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 2>&1 | tail -2            # ~1806
( ulimit -v 6000000; cargo test -p huck --test named_fd_integration --jobs 1 -- --test-threads 1 2>&1 | tail -2 )  # 7/7
cargo build --release --locked -p huck 2>&1 | tail -1
( ulimit -v 1500000; timeout 1100 tests/scripts/run_diff_checks.sh 2>&1 | tail -3 )    # NN passed, 0 failed (both binaries)
```
The sweep count rises by 2 (the two new harnesses) over v296's 189 → **191 passed, 0 failed**.

- [ ] **Step 5: Commit.**
```bash
cargo fmt --all
git add crates/huck-engine/src/executor.rs
git commit -m "$(cat <<'EOF'
v297 T3: virtual {var} fd allocation + external source-order $v-visibility (#141, #140d)

Child-plan {var} redirects now allocate a virtual fd number (lowest >=10 not an
earlier target / earlier {var}) so `cmd 3>a {v}>x` gets fd 10 like bash, not 11
(#141); the virtual number is set on $v during batch lowering (snapshot/restore,
not persisted for external cmds) so a later sibling `2>&$v` resolves to it,
fixing external source-order $v-visibility (#140d). In-process path unchanged.
named_fd_number + ext-vis diff-checks green; sweep 191/0 both binaries.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Notes for the whole-branch review

- **Acceptance:** `redirect_diag_diff_check.sh` + `named_fd_number_diff_check.sh` fully green; `redirect_audit.sh` DIVERGE strictly reduced with an honest accounting of which remaining divergences are non-cluster-A (other issues); `pipeline_redirect_audit.sh` 15/15, `fd_torture` 44, engine lib ~1806, `named_fd_integration` 7/7 all unchanged; sweep 191/0 both binaries.
- **Correctness crux — Task 3 fd lifetime:** the virtual `{var}` dup2 + close of the parked high fd must be leak- and hang-free across `{var}` with/without preceding file redirects, `{var}` + sibling `>&$v`, moves, and multiple `{var}`s in one command. A wrong virtual number or a missing close shows as a divergent `/proc/self/fd` listing.
- **Scope containment:** in-process `{var}` numbering + `$v` persistence UNCHANGED (only the child/external batch path moves); no `>&<non-numeric>` semantics change; message rc unchanged; the deferred error-model divergences untouched.
- **Snapshot/restore:** confirm `$v` does not leak into the parent for external `{var}` redirects (bash doesn't persist it) and that the snapshot/restore doesn't clobber a genuinely pre-existing `$v`.
