# `set -e` / ERR-trap and-or-list exemption — Implementation Plan

> Inline execution (small, localized fix). Spec:
> `docs/superpowers/specs/2026-07-09-set-e-andor-exemption-design.md`.

**Goal:** errexit + ERR trap fire for a failing command in an `&&`/`||` list
only when it is the syntactically last command in that list (bash parity).

**Architecture:** two-site gate change in `run_andor_group`
(`crates/huck-engine/src/executor.rs`): `next_is_or` → `is_last`.

## Global Constraints

- Per-crate tests only: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`.
  NEVER `--workspace` (OOM on this box).
- Build binary: `cargo build -p huck` (release for suite: `cargo build --release --bin huck`).
- Diff harnesses: guard with `ulimit -v 1500000` + `timeout`.
- Commit trailer: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- Do NOT push to main without confirmation.

---

## Task 1: the two-site fix in `run_andor_group`

**Files:** Modify `crates/huck-engine/src/executor.rs` (~line 383–469).

- First-command site (~413): replace
  `let next_is_or = matches!(rest.first(), Some((Connector::Or, _)));`
  with `let is_last = rest.is_empty();`
  and change the fire condition `!next_is_or` → `is_last`.
- Loop site (~454): replace
  `let next_is_or = matches!(rest.get(i + 1), Some((Connector::Or, _)));`
  with `let is_last = i + 1 == rest.len();`
  and change `!next_is_or` → `is_last`.
- Update the two nearby comments to state the bash rule ("errexit/ERR fire only
  when this failing command is the LAST in the and-or list; a command followed
  by `&&` or `||` is exempt").

Both sites keep the rest of the condition unchanged:
`c != 0 && shell.err_suppressed_depth == 0 && <is_last> && !is_negated_pipeline(cmd)`.

## Task 2: executor unit tests

**Files:** Modify `crates/huck-engine/src/executor.rs` test module.

Add tests driving a `set -e` shell through `execute_with_sink` (follow the
existing executor test helpers for building a `Shell` with `errexit` on and
parsing a fragment) that assert:

- `false && echo x` → does NOT exit (outcome `Continue`, `after` runs).
- `true && false && echo x` → does NOT exit (middle-fail exempt).
- `false && false` → does NOT exit.
- `echo a && false` → EXITS (last-command fail).
- `true && false` → EXITS.
- `false; ` (bare) → EXITS.
- `false || echo x` → does NOT exit (unchanged behavior; regression guard).

Prefer testing at the fragment level via the existing script/`process_line`
harness if the executor unit level is awkward; the diff harness (Task 3) is the
primary gate — keep unit tests minimal but present.

Run: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1` → PASS.

## Task 3: `set_e_andor_diff_check.sh` bash-diff harness

**Files:** Create `tests/scripts/set_e_andor_diff_check.sh`.

Follow the structure of an existing harness (e.g.
`tests/scripts/star_at_modifier_diff_check.sh`): a `checkf` helper that writes a
fragment to a temp file, runs it through both `bash` and the huck release binary
in FILE mode, and asserts byte-identical stdout AND exit code. Cover:

- and-or matrix: `false && echo x`, `false && true`, `false && false`,
  `false && echo a || echo b`, `true && false && echo x`, `false && echo x && echo y`,
  `echo a && false`, `true && false`, `false || echo x`, `false || false`,
  `true || false`, bare `false`.
- pipelines/groups: `true | false`, `false | true`, `{ false; }`, `false && (false)`.
- ERR-trap variants: `set -E; trap 'echo T' ERR; <fragment>` for
  `false && echo x` (no T), `echo a && false` (T), `true && false` (T),
  `false || echo x` (no T).
- real idioms: `printf 'hi\n' | grep -q zz && echo Y` , `command -v nope >/dev/null && echo has || echo missing`.

Each fragment is prefixed with `set -e;` (except where it sets its own options)
and followed by `; echo after` where meaningful. Guard the run with
`ulimit -v 1500000` and a `timeout`.

Run: `bash tests/scripts/set_e_andor_diff_check.sh` → all cases pass.

## Task 4: regression + bash-suite spot check

- `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1` and
  `cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1` → green.
- Build release, run the official runner over `set -e`-heavy categories
  (`errors`, and a couple of others) — confirm no PASS→not-PASS. Record the
  before/after diff-line counts. Not expected to flip a category (measure-first).

## Task 5: whole-branch review + docs

- Self-review the branch diff for the four §4 (spec) invariants: last-command
  cases still exit; `||`-next unchanged; `&&`-next now exempt; ERR-trap parity.
- This is a Tier-1 correctness fix → add a resolved note to memory
  (iterations log + value map) at merge; no `bash-divergences.md` L-entry to
  delete (it was uncatalogued). Add a new `[deferred]` entry only if a follow-on
  gap is found.
