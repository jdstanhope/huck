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

**Measured bash truth (5.2.21).** Two distinct persistence mechanisms hide
behind huck's current "all special builtins always persist":

1. **Assignment-builtin absorption — `export`/`readonly` only, NOT posix-gated.**
   `FOO=val export FOO` keeps `FOO=val` in BOTH default and posix mode (the
   builtin absorbs its named variable). `declare`/`typeset`/`local` do NOT
   absorb the prefix (`FOO=val declare FOO` → unset) — and they aren't special
   builtins anyway, so huck already restores them correctly.
2. **Generic special-builtin persistence — posix-gated.** `var=20 return`,
   `var=20 :`, `var=20 eval …`, `var=20 set --`, etc. persist ONLY in posix
   mode; default mode restores them.

huck today wrongly persists category 2 in default mode (`var=20 return` → `20`,
should be `0`). The fix gates category 2 on posix while leaving `export`/
`readonly` (category 1) persistent in both modes. In `executor.rs` (~4239, the
v221 site):

```rust
let persistent = matches!(resolved.program.as_str(), "export" | "readonly")
    || (builtins::is_special_builtin(&resolved.program) && shell.shell_options.posix);
```

- Default mode: `export`/`readonly` persist their prefix (existing test
  `run_exec_single_special_builtin_inline_assignment_persists` stays green);
  every other special builtin restores (fixes `var=20 return` → `0`).
- Posix mode: all special builtins persist, as huck does today.

`exec` is handled by its own early return (~4248) before this value is used, so
it is unaffected. `export`/`readonly` keep huck's pre-existing whole-prefix
persistence (e.g. `FOO=val export BAR` also persists `FOO`, a per-variable
divergence from bash that predates this work and is left as-is — see
bash-divergences follow-on).

### Section 3 — posix persist survives an enclosing prefix-restore

**Why only one executor path needs to change.** `apply_inline_assignments` /
`restore_inline_assignments` (executor.rs ~6396/6455) are called from four
functions: `run_exec_single` (foreground simple command), `run_double_bracket`,
`run_background_sequence`, and `run_multi_stage`. Only `run_exec_single` can be a
foreground *enclosing* scope whose restore an inner persist must suppress:
`[[ … ]]` cannot nest a function call, and the background/pipeline paths run the
inner command in a subshell, so a persist there never propagates back to the
parent's variable state. Both halves of the func3.sub case — the enclosing
`var=30 f` and the inner `var=20 return` inside f's body — flow through
`run_exec_single` (nested on the Rust call stack, sharing one `&mut Shell`).
**So the scope stack is wired into `run_exec_single` only; the other three
apply/restore sites are left exactly as they are.**

Add a **names-only** shell-managed stack — values stay in the existing local
`snap`, only the snapshotted names go on the shell stack:

```rust
// Shell:
pub inline_scopes: Vec<std::collections::HashSet<String>>,  // default empty
```

In `run_exec_single`:
- After `apply_inline_assignments` succeeds, push the snap's names:
  `shell.inline_scopes.push(snap.iter().map(|(n,_)| n.clone()).collect());`
  (The apply-*error* path runs BEFORE this push and keeps its plain
  `restore_inline_assignments(s, shell)` — no pop.)
- Replace the three post-push exit points — currently
  `if !persistent { restore_inline_assignments(snap, shell) }` at the
  restricted-name return, the child-redir-plan-error return, and the main
  post-dispatch return — with one helper `finalize_inline_scope(snap,
  persistent, shell)` that ALWAYS pops, so push/pop stay balanced on every path:

```rust
fn finalize_inline_scope(snap: AssignmentSnapshot, persistent: bool, shell: &mut Shell) {
    let kept = shell.inline_scopes.pop().unwrap_or_default();
    if persistent {
        // posix special builtin keeps its prefix: make it survive every
        // enclosing restore by deleting these names from all enclosing scopes.
        for (name, _) in &snap {
            for scope in shell.inline_scopes.iter_mut() { scope.remove(name); }
        }
    } else {
        // temporary scope: restore LIFO, but skip any name an inner persist
        // removed from this scope (it must keep the persisted live value).
        for (name, prior) in snap.into_iter().rev() {
            if kept.contains(&name) { shell.restore_var(&name, prior); }
        }
    }
}
```

Trace (posix, `var=0; f(){ var=20 return 5; }; var=30 f`):
1. outer `var=30 f`: snap `[(var,Some(0))]`, set var=30, push `{var}` →
   `inline_scopes=[{var}]`.
2. inner `var=20 return`: snap `[(var,Some(30))]`, set var=20, push `{var}` →
   `[{var},{var}]`; persistent → pop top, remove `var` from the remaining
   (outer) scope → `[{}]`; do not restore. var stays 20.
3. f returns; outer finalize (function → not persistent): pop `{}`, `kept` lacks
   `var` → skip restore. var stays **20**. ✓

Default mode: inner not persistent → restores var=30; outer restores var=0 →
**0**. ✓ No-enclosing cases (no outer prefix → no outer push) fall out
identically. Multi-level (`a=1 o; o→a=2 m; m→a=3 return`, posix) → **3**, because
the persist deletes `a` from BOTH enclosing scopes — a correctness requirement
that rules out a simpler one-shot "skip set."

Invariant: in `run_exec_single`, every push has exactly one matching pop on every
exit path; `inline_scopes` is empty between top-level commands (assert in a test).

## Testing / Verification

- **Unit tests** (shell_state.rs): the flag round-trips via `option_set`/
  `option_get`; default is false.
- **Unit/behavior tests** (executor.rs, via `exec_script`): the four generic
  persistence cases (posix/default × enclosing/no-enclosing, using `return`)
  yield bash's values; the multi-level (`a=1/a=2/a=3 return`) posix case yields
  `a=3`; `export`/`readonly` named-var persist in DEFAULT mode (regression
  guard); `inline_scopes` is empty after each top-level command.
- **Diff harness** `posix_mode_diff_check.sh` vs live bash 5.2.21: run each
  fragment under BOTH `bash`/`huck` (default) and `bash --posix`/`huck --posix`,
  byte-identical. Fragments cover: `var=20 return`/`:` (gated), `FOO=val export
  FOO` (default-persist), the enclosing `var=30 f` case, and flag-plumbing
  (`set -o posix; set -o | grep posix`).
- `cargo test --workspace` green (~3698).
- **func category PASS** (headline criterion); cprint + herestr stay PASS.

## Risks

- **Scope-stack push/pop balance.** Only `run_exec_single` participates, and it
  has exactly three post-push exit points (restricted-name return,
  child-redir-error return, main post-dispatch return); all three route through
  `finalize_inline_scope`, which always pops. The pre-push apply-error path keeps
  plain `restore_inline_assignments` (no pop). A missed pop leaks a scope — caught
  by the `inline_scopes`-empty assertion + the full suite. The other three
  apply/restore call sites (`run_double_bracket`, `run_background_sequence`,
  `run_multi_stage`) are deliberately NOT touched and must keep using the plain
  `restore_inline_assignments` (they never push, so they must never pop).
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
- Add a `[deferred]` (low) entry for the pre-existing `export`/`readonly`
  whole-prefix over-persistence (`FOO=val export BAR` persists `FOO`; bash
  restores it — bash absorbs only the *named* variable). Out of scope here.
