# v355 — split ERR-trap suppression from errexit suppression — design

**Issue:** [#480](https://github.com/jdstanhope/huck/issues/480) — *`set -e` exits
inside a body whose caller is exempt where bash continues.* Downstream and fixed
by the same change: [#468](https://github.com/jdstanhope/huck/issues/468) (an
exempt compound does not propagate the exemption into its body),
[#469](https://github.com/jdstanhope/huck/issues/469) (`!` over a compound
suppresses the body fire; bash still fires),
[#470](https://github.com/jdstanhope/huck/issues/470) (the same gap on the
`set -E` inherited path).

**Follows:** v354 ([#466](https://github.com/jdstanhope/huck/issues/466)), which
unified the pending-unwind signals. `err_suppressed_depth` was deliberately left
alone there; this is the promised second half.

## Problem

One counter, `Shell::err_suppressed_depth`, gates two different things:

- `maybe_errexit` (`executor.rs:96`) — whether a failing command exits the shell
  under `set -e`;
- the ERR-trap fire in `finish_command` (`executor.rs:434`).

bash does not treat those as one concept, so a single counter cannot express its
behaviour. Worse, the counter is raised in only three of the four contexts that
exempt a command, and the missing one is the common one.

**The user-visible cost is that `set -e` exits where bash continues:**

```
$ bash -c 'set -e; f() { false; echo x; }; f || echo or'      # x
$ huck -c 'set -e; f() { false; echo x; }; f || echo or'      # (nothing — shell exits)

$ bash -c 'set -e; { false; echo x; } || echo or'             # x
$ huck -c 'set -e; { false; echo x; } || echo or'             # (nothing — shell exits)
```

A script that guards a failing step with `|| handler` — the standard idiom —
dies instead of running the handler. That is the sharp end of #480 and the
reason this is sev:high rather than a trap-cosmetics issue.

## The contract

Measured against bash 5.2.21 on 2026-08-05. Rows are the **body** of a compound
sitting in an exempt position; the outer command's own status is already handled
correctly today by the `is_last` / `is_negated_pipeline` guards.

| exempt context | ERR in body, no `set -e` | ERR in body, `set -e` | errexit in body |
|---|---|---|---|
| `!` negation | **fires** | suppressed | suppressed |
| `&&` / `\|\|` non-last | suppressed | suppressed | suppressed |
| `if` / `elif` condition | suppressed | suppressed | suppressed |
| `while` / `until` condition | suppressed | suppressed | suppressed |

The negation row is not a measurement artefact. It is the inner command firing,
confirmed three ways:

```
trap 'echo E:$?' ERR; ! { (exit 5); }      # E:5   — carries the inner status
trap 'echo E' ERR;   ! { false; true; }    # E     — fires though the group SUCCEEDS
set -E; trap 'echo E' ERR; f() { false; }; ! f   # E — same through a function body
```

Whether bash reaches this by "ERR suppression depends on errexit" or by "`!`
only sets ignore-return when errexit is on" is unknowable from outside and does
not matter: the observable table is the contract.

## Design

### 1. Two counters

`Shell::err_suppressed_depth` becomes two fields:

```rust
/// Depth of nested contexts where a failing command must NOT exit the shell
/// under `set -e` — a negated pipeline, an `if`/`while` condition, or a
/// command in a non-last `&&`/`||` position, plus everything they run.
pub errexit_suppressed_depth: u32,

/// Depth of nested contexts where a failing command must NOT fire the ERR
/// trap. Usually raised together with the errexit counter, but NOT for a
/// negated pipeline while errexit is off — see the contract table.
pub err_trap_suppressed_depth: u32,
```

Sites express intent rather than poking counters:

| helper | raises |
|---|---|
| `suppress_both()` / `unsuppress_both()` | both counters |
| `suppress_errexit_only()` / `unsuppress_errexit_only()` | errexit only |
| `errexit_suppressed() -> bool` | read, at `maybe_errexit` |
| `err_trap_suppressed() -> bool` | read, at the ERR fire in `finish_command` |

`traps::clear_for_subshell` zeroes both, as it did the single counter.

### 2. Where each is raised

Three existing sites change from the old counter to `suppress_both()`: the
`while`/`until` condition (`executor.rs:1329`), the `if` condition
(`executor.rs:2107`) and the `elif` condition (`executor.rs:2124`).

The negated-pipeline site (`executor.rs:2554`) becomes conditional:

```rust
if pipeline.negate {
    // bash: `!` exempts the negated command from BOTH, but does not stop the
    // BODY of a compound from firing ERR — unless errexit is on, where the
    // exemption propagates fully. `! { false; }` prints the trap's output with
    // `set +e` and prints nothing with `set -e`. Reproducing a bash quirk, not
    // a design choice (#469).
    if shell.shell_options.errexit {
        shell.suppress_both();
    } else {
        shell.suppress_errexit_only();
    }
}
```

The reading of `shell_options.errexit` happens at execution time, so
`set -e` / `set +e` mid-script behaves as bash does.

### 3. The new site — where all four issues converge

`run_andor_group` raises `suppress_both()` around any command that is **not the
last** of its and-or list, and drops it afterwards. That single addition is the
fix for #480 (errexit in the body), #468 (ERR in the body) and #470 (the same
through a function body under `set -E`); #469 is the conditional above.

For a simple command the scope changes nothing observable — its own fire is
already skipped by the `is_last` guard, and it has no body. The scope exists for
what the command *runs*: a brace group's statements, a function's body, a loop's
iterations.

### 4. What deliberately does not change

The outer command's own exemption still comes from `is_last` and
`is_negated_pipeline` at the fire site. This iteration only changes what happens
**inside** an exempt command. The compound-does-not-fire-for-itself rule from
#445 (`body_already_fired_err`) is untouched and its harness must stay green.

## Verification

1. **A truth-table harness.** `tests/scripts/errexit_err_suppression_diff_check.sh`
   pins all sixteen cells: four exempt contexts × {ERR with `set +e`, ERR with
   `set -e`, errexit, the outer command's own status}, plus the function-body
   and `set -E` variants and the nesting cases (`! { { false; }; }`,
   `! ! { false; }`).
2. **The existing ERR/errexit harnesses must stay green unchanged** —
   `err_trap_compound` (30), `err_trap_function` (24), `trap_action_exit` (28),
   `set_e_andor` (the and-or exemption rules) and `negated_errexit` (the `!` rules) — the two most likely to move. If any needs an expected value
   edited, the change went further than the contract.
3. **Full sweep** — `tests/scripts/run_diff_checks.sh`, all green. ⚠️ Several
   job-control harnesses are load-flaky (#476); a failure there must be checked
   against `main` by run-count before it is called a regression, and re-run.
4. **Tests** — `cargo test -p huck-engine --lib -- --test-threads 4`, plus the
   set_options, pipefail, if/while/for/case, functions, subshell and trap
   integration binaries.
5. **bash suite PASS-set diff vs `main`** — `BASH_SOURCE_DIR=/tmp/bash-5.2.21`,
   diffing the whole PASS set rather than the count. `errexit` and `set-e` are
   the categories most likely to move, and movement in either direction needs
   explaining before merge. ⚠️ The runner rebuilds the RELEASE binary, so it
   leaves `target/release/huck` built from whichever branch ran last.
6. **CI green before handover** — a `vNN` iteration PR is the user's to merge.

## Non-goals

- **The outer command's exemption rules.** `is_last` and `is_negated_pipeline`
  stay as they are.
- **#478** (huck does not ignore SIGQUIT non-interactively) and the startup
  disposition model — unrelated, already filed.
- **The job-control flakiness (#476).** It will be noise during this iteration's
  sweeps and must not be silently absorbed into it.
- **A general "ignore return" flag on commands**, the way bash models this
  internally. Two counters reproduce the observable contract; a third
  representation of the same idea is what v354 spent an iteration removing.
