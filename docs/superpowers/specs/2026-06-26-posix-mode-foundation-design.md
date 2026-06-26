# v225 — POSIX-mode foundation + posix-gated special-builtin persistence

## Status

Design approved 2026-06-26. First iteration of "properly support POSIX mode":
add a real `posix` flag and use it to fix special-builtin prefix-assignment
persistence. Fixes a default-mode correctness bug AND flips the `func` bash-test
category to PASS (suite 8→9). Later iterations add the rest of POSIX mode (see
Scope).

## Background

A requirements review (2026-06-26) found that huck accepts `set -o posix` as a
**silent no-op** (builtins.rs ~5035) and nothing branches on it. bash's POSIX
mode is 64 documented behavioral changes; huck's defaults are an inconsistent
blend (some behaviors hardcoded to the POSIX variant, some to the bash default).
Notably, the POSIX-*named* bash-test categories (posix2, posixexp, ifs-posix, …)
do NOT require the flag — they exercise POSIX *features* and fail on unrelated
engine bugs — so a posix flag flips none of them directly. The flag's first
high-value use is special-builtin persistence, which gates `func`'s last hunk.

### The persistence bug (measured against bash 5.2.21)

`var=20 return` (a prefix assignment before the special builtin `return`):

| | no enclosing prefix | with `var=30 func` enclosing |
| --- | --- | --- |
| **default bash** | `var=0` (does NOT persist) | `var=0` |
| **posix bash** | `var=20` (persists) | `var=20` |
| **huck (today)** | `var=20` ✗ (wrong in default) | `var=0` ✗ (wrong in posix — func3.sub) |

So persistence is **posix-gated** in bash, and huck is wrong in BOTH directions:
v221 made special-builtin persistence unconditional (so the no-enclosing case is
the posix answer always — wrong in default), while v221's enclosing
prefix-assignment *restore* clobbers the persist in the `var=30 func` case (wrong
in posix — this is func's sole remaining hunk, func3.sub line 155, `5 0` vs `5 20`).

## Goals

1. A real `posix` flag in `ShellOptions`, settable via `set -o posix`/`set +o posix`,
   the `--posix` CLI option, invocation as `sh`, and `POSIXLY_CORRECT` in the
   startup environment. `set -o`/`set +o` listings report its true state.
2. Special-builtin prefix-assignment persistence is gated on the flag: OFF in
   default mode (`var=20 return` → unchanged), ON in posix mode.
3. In posix mode, a special-builtin persist SURVIVES an enclosing prefix-assignment
   restore (`var=30 func` where func does `var=20 return` → `var=20`), at any
   nesting depth.
4. `func` bash-test category PASSes; no currently-PASS category regresses; default
   and posix behavior match bash 5.2.21 for all four persistence cases.

## Non-goals / Out of scope (later iterations)

- **Cluster A — "non-interactive error → exit in posix mode"** (special-builtin
  error, assignment error before a special builtin, readonly `for`/`select` var,
  `.` not-found, function/special-builtin name clash): the broad-leverage POSIX
  mechanism → v226.
- **Cluster B — format/validation toggles** (drop `function` on nested `declare -f`,
  `kill -l` bare names, `export NAME=` form, `trap -p` SIG_DFL, `shift_verbose`).
- **Gating huck's currently-unconditional POSIX behaviors** (trap names without
  `SIG`, bare `set` omits functions, `inherit_errexit`) — these diverge from bash
  *default* today; making them conditional is a separate iteration.
- The default-mode regressions L-43 (readonly-assignment fatal) and v215
  (arith-error fatal).

## Design

### Section 1 — the `posix` flag

- Add `pub posix: bool` to `ShellOptions` (shell_state.rs:200), default `false`.
- `option_set`/`option_get` (builtins.rs): replace the `"posix" =>` no-op with
  `shell.shell_options.posix = value; Ok(())`, and add `"posix" =>
  Some(shell.shell_options.posix)` to `option_get`. `set -o`/`set +o` listings
  then report the real state.
- huck-cli: parse a `--posix` long option in the argv handling that builds
  `RunMode`, setting posix before the first command runs. Detect invocation as
  `sh` (basename of argv0 == `sh`) and `POSIXLY_CORRECT` present in the
  environment at startup; either turns posix on after startup files. (Engine
  exposes a setter, e.g. `engine.set_posix(true)`, mirroring `set_arg0`.)
- No behavior other than Section 2/3 reads the flag in this iteration.

### Section 2 — gate special-builtin persistence

In `executor.rs` (~4219, the v221 site):

```rust
let persistent = builtins::is_special_builtin(&resolved.program)
    && shell.shell_options.posix;
```

Default mode → `persistent` false for special builtins → their prefix
assignments restore (no persist), matching default bash (`var=20 return` →
unchanged). Posix mode → persist, as today.

### Section 3 — posix persist survives an enclosing prefix-restore

Promote the inline-assignment snapshot from a Rust-stack-local
`AssignmentSnapshot` to a **shell-managed stack** `shell.inline_scopes:
Vec<AssignmentSnapshot>` so an inner persist can reach enclosing scopes:

- `apply_inline_assignments` pushes its snapshot onto `shell.inline_scopes`
  (only when there are assignments); the command's exit paths pop it.
- On a NON-persistent command: pop the top scope and restore its (remaining)
  entries — current behavior, just sourced from the stack.
- On a PERSISTENT command (posix special builtin): pop the top scope, do NOT
  restore it, and for each variable name it held, **remove that name from every
  remaining (enclosing) scope** on `inline_scopes`. This makes the live persisted
  value survive all enclosing restores.

Trace (posix, `var=0; f(){ var=20 return 5; }; var=30 f`):
1. outer `var=30 f`: push `[(var,Some(0))]`; set var=30.
2. inner `var=20 return`: push `[(var,Some(30))]`; set var=20; persistent → pop
   without restore, remove `var` from the outer scope → outer becomes `[]`.
3. f returns; outer pop restores `[]` → var stays **20**. ✓

The same trace in default mode: inner is non-persistent → restores var to 30; f
returns; outer restores var to 0 → **0**. ✓ The no-enclosing cases fall out
identically. Multi-level nesting (`a=1 o; o→a=2 m; m→a=3 return`) works because
the persist removes `a` from BOTH enclosing scopes — a correctness requirement
that rules out a simpler "skip-list set" (which can only consume one level).

Invariant: every push has exactly one matching pop on every exit path of the
simple-command executor; `inline_scopes` is empty between top-level commands
(assert in tests).

## Testing / Verification

- **Unit tests** (shell_state.rs): the flag round-trips via `option_set`/
  `option_get`; default is false.
- **Unit/behavior tests** (executor.rs, via `exec_script`): all four persistence
  cases (posix/default × enclosing/no-enclosing) yield bash's values; a
  multi-level (`a=1/a=2/a=3 return`) posix case yields `a=3`; `inline_scopes` is
  empty after each top-level command.
- **Diff harness** `posix_mode_diff_check.sh` vs live bash 5.2.21: run each
  persistence fragment under BOTH `bash`/`huck` (default) and `bash --posix`/
  `huck --posix` (or `set -o posix` prefix), byte-identical. Include a couple of
  flag-plumbing fragments (`set -o posix; set -o | grep posix`).
- `cargo test --workspace` green (~3698).
- **func category PASS** (headline criterion); cprint + herestr stay PASS.

## Risks

- **Scope-stack push/pop balance.** The simple-command executor has many
  early-return paths that currently call `restore_inline_assignments`; each must
  become a stack pop. A missed pop leaks a scope (caught by the
  `inline_scopes`-empty assertion + the full suite). Mitigation: route all
  pop/restore through one helper; add the emptiness assertion.
- **Behavior change in default mode is intended** (`var=20 return` no longer
  persists). Any existing huck test asserting the old unconditional persist
  encoded the bug and must be updated to the bash-default value — verify against
  bash, don't just flip to green.
- **`--posix`/`sh`/`POSIXLY_CORRECT` plumbing** is additive; guard that omitting
  all of them leaves posix=false (no behavior change for existing invocations).

## Divergence-doc / bookkeeping (on merge)

- `docs/bash-divergences.md`: REMOVE L-61 (func's last blocker resolved → func
  PASSes). Note the new `posix` option is now real (update any "posix no-op"
  references). Add a `[deferred]` entry listing the remaining POSIX-mode clusters
  (A/B + the unconditional-behavior gating) as the roadmap.
- `docs/bash-test-suite-baseline.md` (func → PASS; Summary 8→9) + memory.
