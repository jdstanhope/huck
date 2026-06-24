# v215 Arith non-fatal cascade — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop huck's arith expansion errors from halting the surrounding command list in script-file mode, matching bash. Also accept `set +o posix` / `set -o posix` as a silent no-op. Together these close the cascade that produced a 200-line diff in v214's bash test-suite `arith` category sweep.

**Architecture:** Two surgical changes in `crates/huck-engine/`. (1) `builtins.rs::option_set` gains a `"posix" => Ok(())` arm before the catchall — accepted but inert. (2) The two `WordPart::Arith` arms in `expand.rs` (lines ~1119 and ~1565) drop the `shell.pending_fatal_pe_error = Some(1)` line on the Err branch and stop returning early — the error still prints; the expansion contributes empty; the script continues.

**Tech Stack:** Rust 2024, no new deps.

**Branch:** `v215-arith-non-fatal-cascade`.

**Spec:** `docs/superpowers/specs/2026-06-24-arith-non-fatal-cascade-design.md`.

## Global Constraints

- Commit trailer (every commit): `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>` exact, last line of every commit.
- No source comments referencing `v215` / task numbers / iteration version. Divergence IDs (e.g. `L-55`) MAY be referenced in code comments.
- Workspace test command: `cargo test --workspace`.
- Bash version targeted for the file-mode behavior: 5.2.21 (matches v214's baseline).
- New low-priority divergence entry: **L-55** (next available; current highest is L-54).
- Tier 4 count in `docs/bash-divergences.md`: currently 39, becomes 40 after v215.

**Key verified facts (pre-plan):**

- `crates/huck-engine/src/builtins.rs:4987` — `fn option_set` returns `Err(OptSetErr::Unimplemented)` for `posix` because the catchall arm matches `SETO_TABLE` entries that aren't behaviorally implemented (line 5000).
- `crates/huck-engine/src/builtins.rs:5061-5067` — the `set -o NAME` dispatch site translates `OptSetErr::Unimplemented` into the "huck: set: NAME: not yet supported" stderr message. Once `option_set("posix", ...)` returns Ok(()), that error path is bypassed for `posix`.
- `crates/huck-engine/src/expand.rs:1113-1125` — `WordPart::Arith` arm in `fn expand`. The Err branch (line 1119-1124) calls `with_err(...)`, sets `shell.pending_fatal_pe_error = Some(1)`, and `return result`s early.
- `crates/huck-engine/src/expand.rs:1564-1572` — `WordPart::Arith` arm in the assignment-RHS expansion path. Same shape: print, set fatal, return.
- `crates/huck-engine/src/expand.rs:1001` — `pub fn expand(word: &Word, shell: &mut Shell) -> Vec<Field>`. This is the function name to use in `expand.rs::mod tests`.
- `crates/huck-engine/src/expand.rs:2900` — `fn arith_part(text: &str) -> WordPart` is the test helper. Already in scope via `mod tests`.
- `crates/huck-engine/src/expand.rs:2917-2926` — the existing `expand_arith_part_division_by_zero_is_fatal` test that asserts `shell.pending_fatal_pe_error == Some(1)`. We rename + flip the assertion.
- `tests/builtin_vars.rs` and other integration tests use a private `fn huck(s: &str) -> String` helper that wraps `Command::new(env!("CARGO_BIN_EXE_huck"))` with `-c`. For the file-mode test we need a sibling helper that writes a temp file and runs `huck <tempfile>` instead.
- `docs/bash-divergences.md` highest L-number: L-54. Tier 4 count: 39.

---

## File structure

**Modify:**
- `crates/huck-engine/src/builtins.rs` — add `"posix" => Ok(())` arm to `option_set`; add 2 unit tests.
- `crates/huck-engine/src/expand.rs` — drop `pending_fatal_pe_error` from 2 arith Err arms; rename existing test; add 1 new unit test.
- `docs/bash-divergences.md` — add L-55 entry under Tier 4; update Tier 4 count 39 → 40.
- `docs/bash-test-suite-baseline.md` — regenerate counts + refreshed Notes after re-running v214's harness.

**Create:**
- `tests/arith_nonfatal_integration.rs` — integration test for the file-mode continuation.

No new modules. No new crates.

---

## Task 1: `set +o posix` / `-o posix` accepted as no-op

**Files:**
- Modify: `crates/huck-engine/src/builtins.rs:4987` (`fn option_set`) — add the `"posix"` arm.
- Modify: `crates/huck-engine/src/builtins.rs::mod tests` — add 2 unit tests.

**Interfaces:**
- Produces: `option_set(&mut shell, "posix", true)` and `("posix", false)` both return `Ok(())`. No state change.

- [ ] **Step 1: Create the branch**

```bash
git checkout main
git pull --ff-only
git checkout -b v215-arith-non-fatal-cascade
```

- [ ] **Step 2: Add the `"posix" =>` arm in `option_set`**

```bash
grep -n "fn option_set" crates/huck-engine/src/builtins.rs
```

Find `fn option_set` (line ~4987). The current shape ends with `other => { if SETO_TABLE.iter().any(...) Err(Unimplemented) else Err(Unknown) }`. Insert this arm IMMEDIATELY BEFORE the `other =>` catchall:

```rust
        "posix" => {
            // Accept as a silent no-op. huck is POSIX-respecting by default;
            // `set +o posix` is a no-op against that default, and `set -o
            // posix` does not unlock additional strict-POSIX semantics.
            // Scripts that toggle the option for bash compatibility pass
            // through cleanly. The "huck doesn't implement strict POSIX
            // mode" gap is a known minor divergence.
            let _ = value;
            Ok(())
        }
```

`option_get` already returns `Some(false)` via the SETO_TABLE default; no change there.

- [ ] **Step 3: Add 2 unit tests**

Find the existing `mod tests` block:

```bash
grep -n "^mod tests\|#\[cfg(test)\]" crates/huck-engine/src/builtins.rs | head -3
```

Append at the end of the block (the existing block uses `use super::*;`; verify and adapt if it uses explicit imports instead):

```rust
    #[test]
    fn set_posix_option_is_accepted_as_noop_via_option_set() {
        let mut shell = crate::shell_state::Shell::new();
        assert!(super::option_set(&mut shell, "posix", true).is_ok());
        assert!(super::option_set(&mut shell, "posix", false).is_ok());
    }

    #[test]
    fn option_get_posix_returns_table_default() {
        let shell = crate::shell_state::Shell::new();
        // SETO_TABLE default for posix is `false`.
        assert_eq!(super::option_get(&shell, "posix"), Some(false));
    }
```

Adapt `super::` prefixes based on the `mod tests` block's existing style. Both `option_set` and `option_get` are `pub(crate)`/`fn` private to the module — reachable from `mod tests` via `super::` or `*` re-import.

- [ ] **Step 4: Build + run tests**

```bash
cargo build --workspace -q
cargo test -p huck-engine --lib set_posix option_get_posix 2>&1 | tail -5
cargo test --workspace --quiet
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: 2 new tests pass; full suite green; clippy clean.

- [ ] **Step 5: Smoke against the CLI**

```bash
cargo build --release --workspace -q
./target/release/huck -c 'set +o posix; echo POST_PLUS; set -o posix; echo POST_MINUS' 2>&1
echo "exit=$?"
```

Expected: prints `POST_PLUS` and `POST_MINUS` on stdout, no stderr; exit 0.

- [ ] **Step 6: Commit**

```bash
git add crates/huck-engine/src/builtins.rs
git commit -m "$(cat <<'EOF'
v215 task 1: accept `set +o posix` / `-o posix` as a no-op

Special-case `posix` in option_set to return Ok(()) without state
change. huck is POSIX-respecting by default; the `+o` toggle is a no-op
against that default, and the `-o` toggle doesn't unlock additional
strict-POSIX semantics. Scripts using `set +o posix` for bash compat
(common in test suites) now pass through cleanly.

Two unit tests pin option_set("posix", true/false) → Ok and
option_get("posix") returning the SETO_TABLE false default.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: arith expansion errors are non-fatal

**Files:**
- Modify: `crates/huck-engine/src/expand.rs:1113-1125` — main `expand` `WordPart::Arith` arm.
- Modify: `crates/huck-engine/src/expand.rs:1564-1572` — assignment-RHS `WordPart::Arith` arm.
- Modify: `crates/huck-engine/src/expand.rs::mod tests` — rename + flip existing test; add new test.
- Create: `tests/arith_nonfatal_integration.rs` — integration test for script-file continuation.

**Interfaces:**
- Produces: after this task, an arith error in a `$((...))` expansion no longer sets `shell.pending_fatal_pe_error`; the surrounding command continues.

- [ ] **Step 1: Read the current arith Err arms**

```bash
sed -n '1113,1125p' crates/huck-engine/src/expand.rs
sed -n '1564,1572p' crates/huck-engine/src/expand.rs
```

Confirm the shapes match the spec (Err arm calls `with_err`, sets `pending_fatal_pe_error`, and `return result`s early).

- [ ] **Step 2: Update the main `expand` arith arm (line ~1119)**

Replace the Err branch in the main `expand` function's `WordPart::Arith` arm. Today:

```rust
WordPart::Arith { body, quoted: _ } => {
    match eval_arith_word(body, shell) {
        Ok(n) => {
            current.push_str(&n.to_string(), true);
            has_emitted = true;
        }
        Err(e) => {
            with_err(|err| e!(err, "huck: arithmetic: {}", e));
            shell.pending_fatal_pe_error = Some(1);
            return result;
        }
    }
}
```

Change to:

```rust
WordPart::Arith { body, quoted: _ } => {
    match eval_arith_word(body, shell) {
        Ok(n) => {
            current.push_str(&n.to_string(), true);
            has_emitted = true;
        }
        Err(e) => {
            // Print the error but DO NOT set pending_fatal_pe_error —
            // bash script-file mode prints and continues. The empty
            // contribution here matches bash's empty $((..)) value
            // on error. (-c mode divergence: L-55 in bash-divergences.md.)
            with_err(|err| e!(err, "huck: arithmetic: {}", e));
            has_emitted = true;
        }
    }
}
```

Two changes: drop `shell.pending_fatal_pe_error = Some(1)`; drop `return result`. The Err branch now flows through the match, leaves `current` unchanged (no push_str), and continues to the next WordPart in the surrounding loop.

- [ ] **Step 3: Update the assignment-RHS arith arm (line ~1565)**

Replace the Err branch in the second arith arm (the path used by `expand_assignment` for `y=$((..))`). Today:

```rust
WordPart::Arith { body, quoted: _ } => {
    match eval_arith_word(body, shell) {
        Ok(n) => result.push_str(&n.to_string()),
        Err(e) => {
            with_err(|err| e!(err, "huck: arithmetic: {}", e));
            shell.pending_fatal_pe_error = Some(1);
            return result;
        }
    }
}
```

Change to:

```rust
WordPart::Arith { body, quoted: _ } => {
    match eval_arith_word(body, shell) {
        Ok(n) => result.push_str(&n.to_string()),
        Err(e) => {
            // Print the error but DO NOT halt — bash script-file mode
            // prints and continues. Empty contribution to the assignment
            // value matches bash. (-c mode divergence: L-55.)
            with_err(|err| e!(err, "huck: arithmetic: {}", e));
        }
    }
}
```

Drops the same two lines; in this site `result` is the accumulator and we just don't push anything on the error path.

- [ ] **Step 4: Rename + flip the existing test**

Find the existing `expand_arith_part_division_by_zero_is_fatal` test:

```bash
grep -n "expand_arith_part_division_by_zero_is_fatal" crates/huck-engine/src/expand.rs
```

Replace it with:

```rust
#[test]
fn expand_arith_part_division_by_zero_is_nonfatal() {
    // An arith eval error (e.g. division by zero) in $((…)) is NO LONGER a
    // fatal expansion error — bash script-file mode prints the error and
    // continues. The error still surfaces via stderr; pending_fatal_pe_error
    // stays None so the surrounding command list runs to completion.
    // The `-c` mode divergence is tracked as L-55.
    let mut shell = Shell::new();
    let word = Word(vec![arith_part("1 / 0")]);
    let _ = expand(&word, &mut shell);
    assert_eq!(shell.pending_fatal_pe_error, None);
}
```

- [ ] **Step 5: Add a sibling test for the parse-time arith error**

Append in the same `mod tests` block:

```rust
#[test]
fn expand_arith_part_invalid_lhs_assignment_is_nonfatal() {
    // A parse-time arith error (e.g. assignment to a non-lvalue) is also
    // non-fatal. The expansion contributes empty; pending_fatal_pe_error
    // stays None.
    let mut shell = Shell::new();
    let word = Word(vec![arith_part("1 + 2 = 3")]);
    let _ = expand(&word, &mut shell);
    assert_eq!(shell.pending_fatal_pe_error, None);
}
```

If `1 + 2 = 3` doesn't trigger the "assignment requires variable on LHS" path (the arith parser might reject it differently), adapt to whatever literal does — the goal is "an arith expression that fails at parse OR eval time and triggers the Err branch in the WordPart::Arith arm". Probe via `./target/debug/huck -c 'echo $((1 + 2 = 3))'` to see the actual error.

- [ ] **Step 6: Create the integration test**

Create `tests/arith_nonfatal_integration.rs`:

```rust
//! Integration test: arith errors in a script file print to stderr but
//! don't halt subsequent statements.

use std::io::Write;
use std::process::Command;

fn run_script_file(script: &str) -> (String, String, i32) {
    let mut tmp = tempfile::NamedTempFile::new().expect("tempfile");
    tmp.write_all(script.as_bytes()).expect("write");
    tmp.flush().expect("flush");
    let out = Command::new(env!("CARGO_BIN_EXE_huck"))
        .arg(tmp.path())
        .output()
        .expect("spawn");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn arith_division_by_zero_does_not_halt_script_file() {
    let script = "y=$((1/0))\necho POST\n";
    let (stdout, stderr, _rc) = run_script_file(script);
    assert!(stdout.contains("POST"), "POST not printed; script halted. stdout={stdout:?}");
    assert!(
        stderr.contains("arithmetic") || stderr.contains("division"),
        "arith error not on stderr. stderr={stderr:?}",
    );
}

#[test]
fn arith_invalid_lhs_does_not_halt_script_file() {
    let script = "y=$((1 + 2 = 3))\necho POST\n";
    let (stdout, _stderr, _rc) = run_script_file(script);
    assert!(stdout.contains("POST"), "POST not printed; script halted. stdout={stdout:?}");
}
```

This requires the `tempfile` crate. Check whether huck's root or test dependencies already include it:

```bash
grep -n "tempfile" Cargo.toml crates/huck-engine/Cargo.toml 2>/dev/null
```

If `tempfile` is NOT already a dev-dependency in the root `huck` crate's `Cargo.toml`, add it:

```toml
[dev-dependencies]
tempfile = "3"
```

If it's already there, use the existing entry.

If adding `tempfile` is undesirable (out-of-scope for v215), simplify by using `std::env::temp_dir()` + `std::fs::write` + a unique filename. Example fallback:

```rust
fn run_script_file(script: &str) -> (String, String, i32) {
    use std::time::{SystemTime, UNIX_EPOCH};
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let path = std::env::temp_dir().join(format!("huck-v215-arith-{stamp}.sh"));
    std::fs::write(&path, script).expect("write");
    let out = Command::new(env!("CARGO_BIN_EXE_huck"))
        .arg(&path)
        .output()
        .expect("spawn");
    let _ = std::fs::remove_file(&path);
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}
```

Prefer the `tempfile` crate if already a dep; the std fallback otherwise.

- [ ] **Step 7: Build + run tests**

```bash
cargo build --workspace -q
cargo test --workspace --quiet expand_arith_part_division_by_zero_is_nonfatal expand_arith_part_invalid_lhs_assignment_is_nonfatal arith_division_by_zero_does_not_halt_script_file arith_invalid_lhs_does_not_halt_script_file
cargo test --workspace --quiet
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: 4 new/renamed tests pass; full suite green; clippy clean.

- [ ] **Step 8: Smoke against the CLI**

```bash
cargo build --release --workspace -q

# Script file mode: error prints, POST still runs.
cat > /tmp/v215-smoke.sh <<'EOF'
y=$((1/0))
echo POST
EOF
./target/release/huck /tmp/v215-smoke.sh
echo "exit=$?"
```

Expected: stderr shows `huck: arithmetic: division by zero`, stdout shows `POST`, exit 0.

```bash
# -c mode: now ALSO continues (the divergence — see L-55).
./target/release/huck -c 'y=$((1/0)); echo POST'
echo "exit=$?"
```

Expected: stderr shows the arith error, stdout shows `POST`. (Bash would NOT print POST here; this is the documented L-55 divergence.)

- [ ] **Step 9: Commit**

```bash
git add crates/huck-engine/src/expand.rs tests/arith_nonfatal_integration.rs Cargo.toml
git commit -m "$(cat <<'EOF'
v215 task 2: arith expansion errors are non-fatal in file mode

The two WordPart::Arith arms in expand.rs (main expand + assignment-RHS
expand) no longer set shell.pending_fatal_pe_error on Err. The error
still prints via with_err; the expansion contributes empty; the
surrounding command list continues. Matches bash script-file behavior
where `y=$((1/0)); echo POST` prints POST.

Renames expand_arith_part_division_by_zero_is_fatal → _is_nonfatal and
flips the assertion. Adds expand_arith_part_invalid_lhs_assignment_is_nonfatal
for the parse-time error path. New integration test
tests/arith_nonfatal_integration.rs exercises the file-mode CLI to
verify the script continues past the error.

The `-c` mode behavior is now identical to file mode — bash diverges
there (exits on arith error in -c) but huck does not. Tracked as L-55
in bash-divergences.md (added in task 3).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: L-55 entry + Tier 4 count update in bash-divergences.md

**Files:**
- Modify: `docs/bash-divergences.md` — add L-55 under Tier 4; update count 39 → 40.

- [ ] **Step 1: Find the Tier 4 section**

```bash
grep -n "## Tier 4\|Tier 4)" docs/bash-divergences.md | head -3
grep -n "^- \*\*L-54" docs/bash-divergences.md
```

Locate Tier 4 ("Low-impact / edge cases") and find the last `L-XX` entry so the new entry inserts in a sensible place. L-54 is the highest.

- [ ] **Step 2: Add L-55 entry**

Insert this new entry in Tier 4. Best location: at the END of the existing L-entries (after L-54 or wherever the bottom of Tier 4 is). Use Edit to place it at a sensible row:

```markdown
- **L-55: arithmetic expansion errors in `-c` mode continue instead of halting** — `[deferred]`, low (found during v215). `bash -c 'y=$((1/0)); echo POST'` prints the arith error to stderr and exits 1 without printing `POST` — bash treats arith expansion errors as fatal in `-c` mode. huck's same invocation prints the error AND prints `POST` (exit 0). Script-file mode (the dominant use case) matches bash in both shells: both print the error and continue. The divergence is huck under-halting in `-c` mode where bash over-halts. v215 corrected huck's previously-fatal-everywhere behavior to match bash script-file mode; the `-c` distinction is a follow-on. Detected via the v214 bash test-suite arith category sweep. Real-world impact: low — `-c` chains where an early arith error should halt later commands are rare.
```

- [ ] **Step 3: Update the Tier 4 count**

```bash
grep -n "Tier 4)\|| 39 |" docs/bash-divergences.md | head -3
```

Find the summary table row for Tier 4 (currently shows `39`). Update to `40`. The row should read:

```markdown
| Low-impact (Tier 4) | 40 | Open edge cases / cosmetic divergences (`[low]`/`[intentional]`/`[deferred]`). |
```

Spot-check the actual current text (count and surrounding cells) before editing — if 39 was already updated by another iteration, use the actual current number + 1.

- [ ] **Step 4: Verify the edits**

```bash
grep -E '^- \*\*L-55' docs/bash-divergences.md | head -1
grep -E '\| 40 \|' docs/bash-divergences.md | head -1
```

Expected: both find a match. L-55 is the only new bullet; Tier 4 count is the new total.

- [ ] **Step 5: Commit**

```bash
git add docs/bash-divergences.md
git commit -m "$(cat <<'EOF'
v215 task 3: add L-55 — arith errors in -c mode continue (was: halt)

After v215 task 2 made arith expansion errors non-fatal, the `-c` mode
behavior diverges from bash: bash exits with rc 1 on arith error in
-c, huck now prints and continues. Script-file mode matches bash in
both shells. Documenting the -c divergence as a low-priority deferred
follow-on. Tier 4 count: 39 → 40.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: regenerate bash-test-suite baseline

**Files:**
- Modify: `docs/bash-test-suite-baseline.md` — regenerated summary + per-category statuses + Notes.

**Interfaces:**
- Consumes: the fixes from Tasks 1+2 (now in the working tree).
- Produces: refreshed baseline with new counts. The `arith` row should improve (FAIL → PASS, or FAIL with much shorter diff).

- [ ] **Step 1: Ensure bash source is available**

```bash
if [ ! -d /tmp/bash-5.2.21 ]; then
    curl -sL https://ftp.gnu.org/gnu/bash/bash-5.2.21.tar.gz | tar -xzf - -C /tmp
fi
export BASH_SOURCE_DIR=/tmp/bash-5.2.21
```

- [ ] **Step 2: Run the full sweep**

```bash
bash tests/bash-test-suite/runner.sh | tee /tmp/v215-sweep.md
echo "---"
grep '^Scratch dir' /tmp/v215-sweep.md | head -1
```

Expected: runs in 5-10 minutes; ~78 categories classified (after the 4 skips). The scratch dir path is in the header — keep it for triage.

- [ ] **Step 3: Compare against the v214 baseline**

```bash
diff <(grep '^| ' docs/bash-test-suite-baseline.md | head -100) <(grep '^| ' /tmp/v215-sweep.md | head -100) | head -80
```

This shows per-category Status deltas. The `arith` row likely flipped (FAIL → PASS, or remained FAIL with a different first 3 diff lines visible in the scratch). Sibling categories (`arith-for`, `arith2`, `arith3`) may also have improved if they shared the cascade pattern.

For each category whose Status flipped:
- **FAIL → PASS**: clear the Note column (empty cell).
- **PASS → FAIL** (shouldn't happen but verify): investigate; this would be a regression.
- **FAIL → FAIL (different reason)**: refresh the Note prose to reflect the new root cause based on `$SCRATCH/<category>.diff`. Don't copy verbatim bash content.
- **TIMEOUT/ERROR changes**: similar triage.

- [ ] **Step 4: Rewrite `docs/bash-test-suite-baseline.md`**

Replace the existing baseline doc using the v215 sweep output. The structure is the same as v214's; only counts + Status + Notes change. The committed format (verbatim from v214):

```markdown
# bash 5.2.21 test-suite baseline

bash source: 5.2.21 (GNU, GPLv3+; not vendored, run from `$BASH_SOURCE_DIR`).
huck commit: <NEW_SHA from `git rev-parse --short HEAD` AFTER tasks 1-3 land>.
Sweep date: <YYYY-MM-DD UTC>.

## Summary

- Categories run: <NN>
- PASS: <NN>
- FAIL: <NN>
- TIMEOUT: <NN>
- ERROR: <NN>
- SKIP (from known-skips.txt): 4

## Per-category status

| Category | Status | Note |
|---|---|---|
| <alphabetically-sorted rows with refreshed Status + Note> | | |

## Skipped categories

(unchanged from v214 — keep the existing 4-row table verbatim)

## How to regenerate

(unchanged from v214 — keep this section verbatim)

## Licensing reminder

(unchanged from v214 — keep this section verbatim)
```

Fill in counts from the sweep output; preserve any categories whose Status is unchanged (their Notes need no refresh).

Re-verify that no Note cell contains verbatim bash content. The same heuristic from v214 Task 4 Step 6 applies: `grep -E '`|\$\{|\$\(' docs/bash-test-suite-baseline.md` should match only the boilerplate header/footer paths.

- [ ] **Step 5: Commit**

```bash
git add docs/bash-test-suite-baseline.md
git commit -m "$(cat <<'EOF'
v215 task 4: regenerate bash-test-suite baseline after the arith fix

Reruns tests/bash-test-suite/runner.sh against bash 5.2.21 after
Tasks 1-2 land. The arith cascade (200-line diff dominated by huck
halting on the first error) is closed: arith row [PASS/FAIL-with-Xline-diff]
in the v215 baseline (was FAIL with 200-line diff in v214). Sibling
arith categories ([list any that flipped]) similarly improved.

New summary: NN PASS / NN FAIL / NN TIMEOUT / NN ERROR / 4 SKIP
(was 5/73/4/0/4 in v214).

Notes refreshed for flipped rows. No verbatim bash content committed.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

The commit message reports the actual new counts.

---

## Task 5: Final sweep + stop

**Files:**
- (no further file changes; just verification)

- [ ] **Step 1: Full sweep**

```bash
cargo test --workspace --quiet
cargo test --workspace --doc --quiet
cargo clippy --workspace --all-targets -- -D warnings
cargo build --release --workspace --quiet

# 132 harnesses (131 prior + new bash-test-suite smoke from v214):
unset BASH_SOURCE_DIR
FAIL=0
for h in tests/scripts/*_diff_check.sh; do
    bash "$h" > /tmp/h.out 2>&1
    rc=$?
    if [ $rc -ne 0 ]; then
        echo "FAIL: $h (exit $rc)"
        FAIL=1
    fi
done
[ $FAIL -eq 0 ] && echo "all $(ls tests/scripts/*_diff_check.sh | wc -l) harnesses green"

# Headless CLI smoke:
./target/release/huck -c 'echo hello'
echo "exit=$?"

# v215 smoke:
./target/release/huck -c 'set +o posix; y=$((1/0)); echo POST'
echo "exit=$?"
```

Expected: all green; release builds; 132 harnesses pass; smoke prints `hello` + `exit=0`; v215 smoke prints the arith error + `POST` + exit 0.

- [ ] **Step 2: Stop — do NOT merge**

The final whole-branch code review is the controller's call.

---

## Self-review

**Spec coverage:**
- `set +o posix` no-op: Task 1.
- Arith expansion errors non-fatal: Task 2.
- Test updates (rename + new unit + integration): Task 2.
- L-55 entry: Task 3.
- Tier 4 count update: Task 3.
- Baseline regen: Task 4.
- Final verify: Task 5.

**Placeholder scan:**
- "Adapt to whatever literal does — the goal is …" in Task 2 Step 5: instruction with concrete derivation hint (probe via `./target/debug/huck -c 'echo $((..))'`). Acceptable; the implementer reads the actual error to pick.
- "<NN>" / "<NEW_SHA>" / "<list any that flipped>" in Task 4 Step 4's template are placeholders the implementer fills from the sweep output. Acceptable; explicit instruction to fill from actual data.
- "39 → 40" in Task 3 Step 3: if the Tier 4 count isn't currently 39 (because another iteration changed it), the instruction is "use the actual current number + 1". Conditional but concrete.

**Type consistency:**
- `option_set(&mut Shell, &str, bool) -> Result<(), OptSetErr>`: consistent across Task 1 references.
- `option_get(&Shell, &str) -> Option<bool>`: consistent in Task 1.
- `shell.pending_fatal_pe_error`: same field name throughout Task 2.
- `eval_arith_word` + `with_err` + `e!`: existing huck API; not redefined here.
- `arith_part(text: &str) -> WordPart`: existing test helper at expand.rs:2900; used unchanged in Task 2.

**5 tasks. ~20 LOC production + ~50 LOC tests + ~1 LOC docs + regenerated baseline.**
