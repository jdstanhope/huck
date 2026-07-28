# v340 — nquote2+nquote3 positional `${@<op>}` transforms Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Flip BOTH bash-suite categories `nquote2` and `nquote3` to 0-diff PASS (runner PASS 27 → 29) by making positional `${@<op>}`/`${*<op>}` per-element transforms map over each positional parameter, mirroring the working array `${arr[@]<op>}` path.

**Architecture:** One new dispatch branch + one small helper in `crates/huck-engine/src/expand.rs`. The per-element machinery (`is_per_element_modifier`, `scalar_apply_per_element`, the `WordList`/`Value` result arms) already exists and is used by the array path; this wires `$@`/`$*` into it.

**Tech Stack:** Rust (`huck-engine`), bash-vs-huck diff-check harnesses, the bash test-suite runner.

**Design reference:** `docs/superpowers/specs/2026-07-28-nquote-positional-transform-design.md`. Issue: [#314](https://github.com/jdstanhope/huck/issues/314).

## Global Constraints

- **Branch:** all work on `v340-nquote-positional-transform` (off `main`). Do NOT push to `main` or merge; hand the PR to the user.
- **Commit trailer:** every commit ends with `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- **Formatting:** run `cargo fmt --all` before every commit (CI enforces `cargo fmt --all --check`).
- **This box OOMs on `cargo test --workspace`.** Test per-crate single-threaded ONLY: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`. Build the binary with `cargo build -p huck` (debug) / `cargo build --release --locked --bin huck` (release). Guard sweeps with `ulimit -v 1500000` + `timeout`.
- **Bash source** at `/tmp/bash-5.2.21`; `export BASH_SOURCE_DIR=/tmp/bash-5.2.21` for the runner.
- **Empirical bash rule:** `${@<op>}`/`${*<op>}` with a per-element transform op (`#`/`##`/`%`/`%%`/`/`/`//`/case/`@Q`-family) applies `<op>` to EACH positional parameter. Result shape mirrors the array arm exactly: quoted `@` → separate words; quoted `*` → one IFS[0]-joined field; unquoted `@`/`*` → IFS-split each.
- **Regression discipline (v334 lesson):** word-splitting-adjacent change — prove no-regression against a `origin/main` worktree baseline on the big expansion categories before the PR.

---

### Task 1: Route positional `${@<op>}`/`${*<op>}` transforms through the per-element path

**Files:**
- Modify: `crates/huck-engine/src/expand.rs` (dispatch at ~1258; new `expand_positional_transform` helper near `scalar_apply_per_element` ~350)
- Test: `tests/scripts/param_transform_diff_check.sh` (extend)

**Interfaces:**
- Consumes: existing `is_per_element_modifier(&ParamModifier) -> bool` (expand.rs:291), `scalar_apply_per_element(name, modifier, element, quoted, shell) -> String` (expand.rs:331), `ifs_join_sep(&str) -> String`, `ExpansionResult::{WordList, Value}`.
- Produces: `fn expand_positional_transform(name: &str, modifier: &ParamModifier, quoted: bool, shell: &mut Shell) -> ExpansionResult`.

- [ ] **Step 1: Add failing harness cases**

In `tests/scripts/param_transform_diff_check.sh`, using the file's existing `check "<label>" '<fragment>'` helper (byte-identical bash↔huck over stdin), add before the final total line. Each fragment prints each resulting word wrapped in `<…>` so word boundaries are visible:

```bash
# v340 (#314): positional ${@<op>}/${*<op>} per-element transforms.
check "at unq subst"   'set aXa bXb cXc; for w in ${@/X/-};   do printf "<%s>" "$w"; done; echo'
check "at q   subst"   'set aXa bXb cXc; for w in "${@/X/-}"; do printf "<%s>" "$w"; done; echo'
check "star unq subst" 'set aXa bXb cXc; for w in ${*/X/-};   do printf "<%s>" "$w"; done; echo'
check "star q   subst" 'set aXa bXb cXc; for w in "${*/X/-}"; do printf "<%s>" "$w"; done; echo'
check "at q   rmpre"   'set aXa bXb cXc; for w in "${@#?}";   do printf "<%s>" "$w"; done; echo'
check "at q   rmsuf"   'set aXa bXb cXc; for w in "${@%?}";   do printf "<%s>" "$w"; done; echo'
check "at q   case"    'set foo bar baz; for w in "${@^^}";   do printf "<%s>" "$w"; done; echo'
check "at q   quoteQ"  'set "a b" c;     for w in "${@@Q}";   do printf "<%s>" "$w"; done; echo'
check "at q   ctrlA"   'e=$'"'"'uv\001\001wx'"'"'; set "$e" "$e"; for w in "${@/$'"'"'\001'"'"'/A}"; do printf "<%s>" "$w"; done; echo'
check "at empty args"  'set --; for w in "${@/X/-}"; do printf "<%s>" "$w"; done; echo DONE'
check "star q custom-IFS" 'IFS=-; set aXa bXb cXc; printf "<%s>" "${*/X/_}"; echo'
```

- [ ] **Step 2: Build and run the harness; confirm the new cases FAIL**

Run:
```bash
cargo build -p huck
bash tests/scripts/param_transform_diff_check.sh
```
Expected: the `at`/`star` transform cases FAIL — huck applies the op to only the first param (unquoted) and joins into one word (quoted). (Bare pre-existing cases still PASS.)

- [ ] **Step 3: Add the `expand_positional_transform` helper**

In `crates/huck-engine/src/expand.rs`, near `scalar_apply_per_element` (after ~line 350), add:

```rust
/// `${@<op>}` / `${*<op>}` with a per-element transform op: apply `op` to EACH
/// positional parameter (like `${arr[@]<op>}`). Result-shape mirrors the array
/// per-element arm verbatim (expand_array_param): quoted `@` → WordList
/// (separate words); every other case → Value(IFS[0]-join) — quoted `*` → one
/// field, unquoted `@`/`*` → caller IFS-splits. v340 (#314).
fn expand_positional_transform(
    name: &str,
    modifier: &crate::lexer::ParamModifier,
    quoted: bool,
    shell: &mut crate::shell_state::Shell,
) -> crate::param_expansion::ExpansionResult {
    use crate::param_expansion::ExpansionResult;
    // Clone the args so the per-element closure can borrow `shell` mutably.
    let args = shell.positional_args.clone();
    let transformed: Vec<String> = args
        .iter()
        .map(|a| scalar_apply_per_element(name, modifier, a, quoted, shell))
        .collect();
    if name == "@" && quoted {
        ExpansionResult::WordList(transformed)
    } else {
        let sep = ifs_join_sep(&shell.ifs());
        ExpansionResult::Value(transformed.join(&sep))
    }
}
```

- [ ] **Step 4: Wire it into the dispatch**

In `expand.rs`, in the `WordPart::ParamExpansion` arm's `result_pe` chain (~line 1258), add a branch **before** the final scalar `else`:

```rust
            } else if matches!(
                (name.as_str(), modifier),
                ("@" | "*", crate::lexer::ParamModifier::Substring { .. })
            ) {
                expand_positional_substring(name, modifier, *quoted, shell)
            } else if matches!(name.as_str(), "@" | "*") && is_per_element_modifier(modifier) {
                expand_positional_transform(name, modifier, *quoted, shell)
            } else {
                crate::param_expansion::expand_modifier_quoted(name, modifier, *quoted, shell)
            };
```

- [ ] **Step 5: Build and run the harness; confirm all cases PASS**

Run:
```bash
cargo build -p huck
bash tests/scripts/param_transform_diff_check.sh
```
Expected: `Fail: 0` — every case (new + pre-existing) byte-identical to bash.

- [ ] **Step 6: Per-crate lib tests**

Run: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`
Expected: PASS — no expansion/param unit test regresses.

- [ ] **Step 7: `cargo fmt --all` and commit**

```bash
cargo fmt --all
git add crates/huck-engine/src/expand.rs tests/scripts/param_transform_diff_check.sh
git commit -m "$(cat <<'EOF'
v340: map positional ${@<op>}/${*<op>} transforms per-element (#314)

$@/$* with a per-element transform op (pattern removal / substitution /
case / @Q) routed through the scalar is_star_at path — IFS-joining to one
string and applying the op once (first param only; quoted form joined to
one word). Route them through expand_positional_transform, which mirrors
the array ${arr[@]<op>} per-element arm (scalar_apply_per_element over
positional_args → WordList for quoted @, Value(join) otherwise).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Verify the double flip, prove no-regression, update docs + memory

**Files:**
- Modify: `docs/bash-test-suite-baseline.md`
- Modify: `/home/john/.claude/projects/-home-john-projects-huck/memory/project_huck_iterations.md`
- Modify: `/home/john/.claude/projects/-home-john-projects-huck/memory/MEMORY.md`

**Interfaces:** none (verification + docs).

- [ ] **Step 1: Build release**

Run: `cargo build --release --locked --bin huck`
Expected: clean.

- [ ] **Step 2: Both category runners — 0-diff PASS**

Run:
```bash
export BASH_SOURCE_DIR=/tmp/bash-5.2.21
for c in nquote2 nquote3; do
  HUCK_BASH_TEST_CATEGORY=$c bash tests/bash-test-suite/runner.sh 2>/dev/null | grep -E "^\| $c "
done
```
Expected: both `| nquote2 | PASS |` and `| nquote3 | PASS |`. If FAIL, inspect the fresh `/tmp/huck-bash-tests-*/nquote{2,3}.diff` (must be empty) and map any residual back to `${@<op>}`.

- [ ] **Step 3: No-regression baseline (v334 discipline)**

Build `origin/main` in a worktree and diff the big expansion categories' output old-vs-new:
```bash
git worktree add -q /tmp/huck-v340-base origin/main
( cd /tmp/huck-v340-base && ulimit -v 3000000 && cargo build --release --locked --bin huck )
export BASH_SOURCE_DIR=/tmp/bash-5.2.21
for c in more-exp new-exp exp-tests array array2 dollars posixexp nquote nquote1; do
  HUCK_BASH_TEST_CATEGORY=$c bash /tmp/huck-v340-base/tests/bash-test-suite/runner.sh 2>/dev/null | grep -E "^\| $c " | sed 's/$/  [BASE]/'
  HUCK_BASH_TEST_CATEGORY=$c bash tests/bash-test-suite/runner.sh 2>/dev/null | grep -E "^\| $c " | sed 's/$/  [v340]/'
done
git worktree remove --force /tmp/huck-v340-base
```
Expected: every listed category has the SAME status BASE vs v340 (none regresses; any that were PASS stay PASS; FAIL ones don't get worse — spot-check a couple of diffs if a line-count shifts).

- [ ] **Step 4: Full diff-check sweep**

Run:
```bash
cargo build -p huck
( ulimit -v 1500000; timeout 600 bash tests/scripts/run_diff_checks.sh )
```
Expected: all harnesses PASS (green), incl. `param_transform`, `param_substitution`, `array_transforms`, `array_at_star`, `ifs_*`.

- [ ] **Step 5: Touched integration bins (single-threaded)**

Run:
```bash
for t in param_transform_integration param_substitution_integration special_params_integration arrays_integration ifs_integration; do
  ( ulimit -v 1500000; cargo test -p huck --test $t --jobs 1 -- --test-threads 1 ) 2>&1 | grep -E 'test result|error\[' || echo "MISSING/FAILED: $t"
done
```
Expected: each `test result: ok` (skip any bin that doesn't exist — note it, don't fail).

- [ ] **Step 6: Confirm only nquote2+nquote3 flipped (full runner)**

Run:
```bash
export BASH_SOURCE_DIR=/tmp/bash-5.2.21
bash tests/bash-test-suite/runner.sh 2>/dev/null | grep -E 'PASS:|FAIL:'
```
Expected: `PASS: 29`, `FAIL: 53`. Cross-check the PASS category list gained exactly `nquote2` and `nquote3` versus the prior 27 (array2 cprint dbg-support2 dynvar extglob2 extglob3 func getopts herestr ifs input-test invert iquote lastpipe nquote nquote1 nquote5 parser posix2 posixpat precedence procsub rhs-exp set-x strip tilde tilde2).

- [ ] **Step 7: Update `docs/bash-test-suite-baseline.md`**

Add a dated `**Updated by v340 (#314, 2026-07-28 UTC):**` note at the top (mirroring the v339 note's style): `nquote2` + `nquote3` flipped to PASS (double flip); root = positional `${@<op>}`/`${*<op>}` per-element transforms applied to only the first param (routed through the array per-element path); Summary PASS 27→29, FAIL 55→53; only these two flipped, no regressions. Update the `## Summary` count block (PASS 27→29, FAIL 55→53) and refresh its PASS-category list. Replace the `| nquote2 | FAIL | … |` and `| nquote3 | FAIL | … |` rows with PASS rows. Note the `${@:-word}` default-op field-splitting (#26) remains deferred.

- [ ] **Step 8: Update memory files**

Append a v340 entry to `project_huck_iterations.md` (newest at top) and add a one-line v340 hook to the top of `MEMORY.md`'s iteration list: FLIPS `nquote2`+`nquote3` 27→29 (double flip); root = positional `${@<op>}`/`${*<op>}` per-element transforms (L-88 subset #314) applied first-param-only + quoted-join, routed through the array `scalar_apply_per_element`→WordList/Value path; durable lessons — (a) the Ctrl-A test data was a red herring, the bug reproduces on plain data; (b) the ORIGINAL v340 target `arith-for` was abandoned mid-plan on the `$0` prog-name artifact ([[huck-bashsuite-prog-name-artifact]]); (c) proved no-regression with a main-worktree baseline on the big expansion categories. Note #26 (default-op half of L-88) still open.

- [ ] **Step 9: Commit docs**

```bash
git add docs/bash-test-suite-baseline.md
git commit -m "$(cat <<'EOF'
v340: baseline — nquote2+nquote3 flipped to PASS (27->29) (#314)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```
(Memory files are outside the repo — save them via the Write tool, not git.)

---

## Final review & PR (after all tasks)

- [ ] Review the whole branch diff (`git diff main...v340-nquote-positional-transform`) for stray edits and formatting.
- [ ] Confirm `cargo fmt --all --check` clean and a fresh `cargo build --workspace --locked` (build only) succeeds.
- [ ] Push `v340-nquote-positional-transform`, open a PR targeting `main` with body `Closes #314`, a summary of the root + fix + double flip, and the verification evidence (both runners PASS, no-regression baseline, sweep green). Hand to the user; wait for CI green before calling it ready (do NOT self-merge).
