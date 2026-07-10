# v280 — Run the bash-diff harness sweep in CI + fix 4 stale prog-name normalizers

**Issue:** [#110](https://github.com/jdstanhope/huck/issues/110) (enhancement).
Quarantines the one real deferred bug tracked in
[#109](https://github.com/jdstanhope/huck/issues/109) via a surgical in-harness XFAIL.

## Problem

The `tests/scripts/*_diff_check.sh` bash-diff harnesses — huck's gold-standard
bash-compat check — are **never run in CI**. `.github/workflows/ci.yml` only
does `cargo fmt --check`, `cargo build --workspace`, and `cargo test
--workspace`. A harness can go red and nothing catches it. Several have.

A full sweep, run with each harness against **its own default binary** (161
default to `target/debug/huck`, 8 to `target/release/huck`), gives **175 pass /
5 fail**:

1. `shift_range_diff_check.sh`
2. `loop_levels_diff_check.sh`
3. `trap_kill_stop_diff_check.sh`
4. `indirect_unset_positional_diff_check.sh`
5. `cmdsub_comment_diff_check.sh`

Failures 1–4 are **stale harness normalizers, not huck bugs.** Each harness
invokes bare `bash` (error prefix `bash: `) but absolute-path `$HUCK_BIN`
(error prefix `/…/target/debug/huck: `), then strips only a bare `^huck: `.
huck's prefix is **bash-faithful**: `/usr/bin/bash -c 'shift abc'` likewise
prints `/usr/bin/bash: line 1: …`. The v269 unified-error-emitter rework made
huck's prologue bash-correct, which silently broke these normalizers; CI never
noticed because it never runs the sweep.

Failure 5 is a **real deferred parser bug** (comment inside `$()`), tracked in
#109 and quarantined here.

### Not a bug: funcnest

`funcnest_diff_check.sh` fails **only** when forced onto the debug binary: 2048
deep debug stack frames overflow the 8 MB OS stack before huck's internal
`FUNCNEST_HARD_MAX = 2048` cap fires (SIGABRT). It **passes on the release
binary it targets** (5/5, backstop included). No huck change is warranted. The
durable lesson — encoded in this design — is that the sweep MUST run each
harness against its intended binary, which is exactly why the runner does not
override `HUCK_BIN`.

## Design

Five deliverables. No Rust code changes; no shell behavior change.

### 1. Committed sweep runner — `tests/scripts/run_diff_checks.sh`

One script, used by both CI and the per-iteration local check.

- Runs from the repo root (`cd "$(dirname "$0")/../.."`) so both default-binary
  forms resolve (`$(pwd)/target/...` and `$_SCRIPT_DIR/../../target/...`).
- Requires **both** binaries to already exist (`target/debug/huck` and
  `target/release/huck`); it does **not** build them and does **not** set
  `ulimit` — the caller (CI job / developer) owns that. Missing binary → clear
  error, exit 1.
- Iterates `tests/scripts/*_diff_check.sh`, **excluding**:
  - `run_diff_checks.sh` itself (defensive; it does not match `*_diff_check.sh`
    but the loop is explicit).
  - `bash_test_suite_runner_diff_check.sh` — needs a downloaded
    `BASH_SOURCE_DIR`; out of scope for this gate.
- Runs each harness with **its own default binary** (never exports/overrides
  `HUCK_BIN`), under `timeout 120`.
- Prints `PASS <name>` / `FAIL <name>` per harness and a final tally.
- Exits non-zero iff any harness is red (`timeout` rc 124 counts as red).
- **No central allowlist** — #109 is handled by surgical XFAIL inside its
  harness (deliverable 2), so the gate is simply "every harness green."

Reference skeleton (final wording is the implementer's; behavior above is
binding):

```bash
#!/usr/bin/env bash
# Run every bash-diff harness against its DEFAULT binary. Green = all pass.
# Caller MUST build both binaries first:
#   cargo build --locked --bin huck            # target/debug/huck
#   cargo build --release --locked --bin huck  # target/release/huck
set -u
cd "$(dirname "$0")/../.." || exit 1
for b in target/debug/huck target/release/huck; do
  [[ -x "$b" ]] || { echo "missing binary: $b — build it first" >&2; exit 1; }
done
pass=0; fail=0; failed=()
for h in tests/scripts/*_diff_check.sh; do
  name=$(basename "$h")
  case "$name" in
    run_diff_checks.sh|bash_test_suite_runner_diff_check.sh) continue ;;
  esac
  if timeout 120 bash "$h" >/dev/null 2>&1; then
    pass=$((pass+1)); echo "PASS $name"
  else
    fail=$((fail+1)); failed+=("$name"); echo "FAIL $name"
  fi
done
echo
echo "Diff-check sweep: $pass passed, $fail failed"
(( fail == 0 )) || { echo "Failed: ${failed[*]}" >&2; exit 1; }
```

### 2. Surgical XFAIL in `cmdsub_comment_diff_check.sh`

The one failing case is `check "comment after open"` (lines 20–21):

```
echo "[$(# c with ) paren
echo yo)]"
```

bash → `[yo]` rc 0; huck → `syntax error` rc 2 (comment-in-`$()` mis-parse).

Replace that single `check` invocation with an expected-fail assertion: verify
huck **still diverges** (so the harness notices when #109 is fixed), print
`XFAIL: comment after open (#109)`, and keep it out of the `FAIL` count. The
other 7 cases in the file remain hard `check` assertions. File exits 0.

The XFAIL must be self-checking: if huck's output for that fragment ever matches
bash's, the harness prints `FAIL: comment after open unexpectedly passes —
close #109 and restore check()` and exits non-zero, so a silent fix is caught.
Leave a comment: `# XFAIL: tracked in #109 — restore check() when fixed`.

### 3. Fix the 4 stale normalizers

huck is bash-correct; the normalizers are wrong. Apply the same canonical
path-robust transform used already by one harness — make the program-name arm
tolerate an optional leading path (`([^:]*/)?`), preserving each harness's
existing normalized target:

- `shift_range_diff_check.sh` line 15 `norm()`:
  `s/^(bash|huck): (line [0-9]+: )?//`
  → `s#^([^:]*/)?(bash|huck): (line [0-9]+: )?##`
- `indirect_unset_positional_diff_check.sh` line 17 `norm()`: same change as
  shift_range.
- `trap_kill_stop_diff_check.sh` line 14 `norm()`:
  `s/^(bash|huck): (line [0-9]+: )?//; s/\bSIG([A-Z])/\1/g`
  → `s#^([^:]*/)?(bash|huck): (line [0-9]+: )?##; s/\bSIG([A-Z])/\1/g`
- `loop_levels_diff_check.sh` line 30, the two `SHELL:`-mapping subs:
  `s/^bash: line [0-9]*: /SHELL: /g; s/^huck: /SHELL: /g`
  → `s#^([^:]*/)?bash: (line [0-9]+: )?#SHELL: #g; s#^([^:]*/)?huck: (line [0-9]+: )?#SHELL: #g`
  (also strips huck's `line N:` — which huck now emits for stdin scripts — so
  it matches bash's already-stripped `SHELL:` form).

Why `([^:]*/)?` is safe: it is anchored at `^`, `[^:]*` stops at the first `:`
(the one right after `huck`/`bash`), and the local binary paths contain no `:`.
Lines without a program prefix don't match and are untouched.

### 4. CI wiring — extend the existing `build & test` job

In `.github/workflows/ci.yml`, after the existing `Test (workspace)` step
(which leaves `target/debug/huck` built), add two steps in the same job (reuses
the cargo cache; no duplicate debug build):

```yaml
      - name: Build release huck (release-default harnesses)
        run: cargo build --release --locked --bin huck

      - name: Bash-diff harness sweep
        run: tests/scripts/run_diff_checks.sh
```

A red sweep fails CI. No separate job (would duplicate the debug build).

### 5. CLAUDE.md checklist

Add the sweep to the iteration loop's verification step so it runs every
iteration: after building, run `tests/scripts/run_diff_checks.sh` (build both
debug and release huck first) and confirm it is green before opening the PR.

## Testing

No Rust changes, so `cargo test` is unaffected. Verify by execution:

- After deliverables 2 + 3, run each of the 5 previously-failing harnesses
  standalone against its default binary — each exits 0
  (cmdsub_comment via XFAIL, the other 4 via the normalizer fix).
- Run `tests/scripts/run_diff_checks.sh` (both binaries built) — expect
  `Diff-check sweep: 180 passed, 0 failed`, exit 0. (180 = 181 harnesses minus
  the excluded bash-test-suite runner.)
- Negative check for the XFAIL: temporarily confirm the harness would flag an
  unexpected pass (reasoned/inspected, not committed).

## Out of scope

- Fixing #109 (comment-in-`$()`): tracked, quarantined here.
- Any huck source/behavior change (funcnest included — not a bug).
- The `bash_test_suite_runner_diff_check.sh` harness and the bash 5.2 test
  suite (separate `BASH_SOURCE_DIR` tooling).
