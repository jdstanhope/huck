# `set -e` / ERR-trap and-or-list exemption

**Status:** design (brainstormed 2026-07-09)
**Topic:** fix `set -e` (errexit) and the ERR trap firing on a command that is
NOT the last in an `&&`/`||` (and-or) list. A real correctness bug that breaks
the most common conditional idioms under `set -e`.

---

## 1. Problem

Under `set -e`, huck exits when a command that is *not the last* in an and-or
list fails. bash exempts every command in an and-or list except the
syntactically last one. Verified against bash 5.2 (file mode):

| fragment (prefixed `set -e;`) | bash | huck (before) |
|---|---|---|
| `false && echo x; echo after`            | `after`     | **EXITS rc1** ✗ |
| `false && true; echo after`              | `after`     | **EXITS rc1** ✗ |
| `false && false; echo after`             | `after`     | **EXITS rc1** ✗ |
| `... \| grep -q zz && echo Y; echo after`| `after`     | **EXITS rc1** ✗ |
| `command -v x >/dev/null && a \|\| b; echo after` | `b`,`after` | **EXITS rc1** ✗ |
| `true && false && echo x; echo after`    | `after`     | **EXITS rc1** ✗ |
| `false \|\| echo x; echo after`          | `x`,`after` | `x`,`after` ✓ |
| `echo a && false; echo after`            | EXITS rc1   | EXITS rc1 ✓ |
| `true && false; echo after`              | EXITS rc1   | EXITS rc1 ✓ |
| `false; echo after`                      | EXITS rc1   | EXITS rc1 ✓ |

The bug also fires the ERR trap wrongly (bash couples ERR and errexit under the
same rule — verified: `set -E; trap 'echo T' ERR; false && echo x` prints no
`T` in bash, huck prints `T`).

**Impact.** `cmd && action`, `check || fallback`, `… | grep -q x && …`,
`command -v x && … || …` — the canonical conditional idioms — all wrongly abort
a `set -e` script in huck. High-frequency in real scripts.

## 2. Root cause

`run_andor_group` (`crates/huck-engine/src/executor.rs`) gates the errexit +
ERR-trap fire on `!next_is_or` — i.e. it fires unless the *next connector is
`||`*. Two sites:

- first command (~line 413): `let next_is_or = matches!(rest.first(), Some((Connector::Or, _)));`
- loop command at index `i` (~line 454): `let next_is_or = matches!(rest.get(i + 1), Some((Connector::Or, _)));`

Both then fire when `c != 0 && err_suppressed_depth == 0 && !next_is_or &&
!is_negated_pipeline(cmd)`.

`!next_is_or` is true both when the next connector is `&&` and when there is no
next command. bash's rule is "fire only when there is *no* next command" (the
command is the syntactically last in the group). The two differ *only* in the
"next connector is `&&`" case — exactly the bug.

## 3. Design — the exemption rule

Replace `next_is_or` with `is_last_in_group`:

- first command: `is_last = rest.is_empty()`
- loop command at index `i`: `is_last = i + 1 == rest.len()`

Fire errexit + ERR trap only when:
`c != 0 && shell.err_suppressed_depth == 0 && is_last && !is_negated_pipeline(cmd)`.

This is a strict generalization of the current code — the `||`-next and
no-next cases are unchanged; only the `&&`-next case flips from "fire" to
"exempt". Both the errexit and ERR-trap fires move together (they already
share the gate; bash shares the rule).

### Why "last in group" and not "last that ran"

The exemption is *syntactic*, not execution-order: `true && false && echo x`
does NOT exit even though the failing `false` is the last command that actually
*runs* (`echo x` is short-circuited away). `false` is exempt because it is not
the syntactically last command of the list. Confirmed against bash. `is_last`
(rest position) is exactly the syntactic-last test.

The `should_run` short-circuit logic is untouched — a command that doesn't run
never reaches the fire site, so an exempt-but-skipped last command (e.g. the
`echo x` in `false && echo x`) simply never fires, which is correct.

## 4. What does NOT change

- Group partitioning (`partition_into_groups`) — `Semi`/`Amp` boundaries and
  the flat-`Sequence` and-or model (M-96) are unchanged.
- `$?` propagation, pending-trap dispatch, interrupt handling.
- errexit suppression inside `if`/`while`/`until` conditions, `!` negation, and
  command-substitution/subshell contexts (`err_suppressed_depth`,
  `is_negated_pipeline`) — orthogonal, already correct.
- The last-command and single-command cases (`echo a && false`, `true && false`,
  bare `false`, `true | false`) — already correct, must stay correct.

## 5. Testing

1. **`tests/scripts/set_e_andor_diff_check.sh`** — a bash-diff harness asserting
   byte-identical stdout+rc for the full matrix: first/middle/last failure ×
   `&&`/`||`/multi-link chains × pipelines/subshells/brace-groups, plus ERR-trap
   (`set -E; trap … ERR`) variants and the real-world idioms (`… | grep -q x &&
   …`, `command -v x && … || …`). Runs each fragment in FILE mode (the real
   non-interactive script path). Guarded with `ulimit -v` + `timeout` per the
   memory note.
2. **executor.rs unit tests** — assert `ExecOutcome::Exit` vs `Continue` for the
   representative cases (`false && echo x` → Continue; `echo a && false` →
   Exit; `true && false && echo x` → Continue).
3. **Full `cargo test -p huck-engine`** (per-crate, `--jobs 1 --test-threads 1`).
4. **bash-suite spot check** — run the `errors`/`posix`-ish categories that
   exercise `set -e` for movement/regression (measure-first: this fixes
   behavior, may or may not flip a category — not the goal).

## 6. Success criteria

- Every row in the §1 table matches bash byte-for-byte (stdout + rc).
- No regression in `cargo test -p huck-engine` / `-p huck-syntax`.
- No regression across the official bash-suite runner (no PASS → not-PASS).
- `set_e_andor_diff_check.sh` green.

## 7. Risks

- **errexit is load-bearing.** A wrong exemption could either mask a real
  failure (script continues when bash would exit) or over-exit. The matrix in §5
  covers both directions (last-command cases must still exit). Gate: the full
  diff harness + bash-suite runner.
- **ERR-trap coupling.** The fix moves ERR-trap fires too. §5 includes ERR-trap
  rows to confirm bash parity.
