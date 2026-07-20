# v319 — Restricted shell: policy abstraction + bash-fidelity entry points

Issue: [#222](https://github.com/jdstanhope/huck/issues/222)

## Problem

huck has a restricted-mode *subset* today, reachable only from the embedding
API (`ExecBuilder::restricted()`). It is implemented as ~9 hand-placed sites of
the shape:

```rust
if crate::restricted::is_restricted(shell)
    && let Err(msg) = crate::restricted::check_cd()
```

Two flag-ish reads plus a bespoke helper per operation, with the policy decision
smeared across `builtins.rs`, `executor.rs`, and `shell_state.rs`. Adding a
restriction means finding the right file and hand-writing the same conditional
again. There is no shell-level entry point at all: no `rbash`, no `-r`, no
`set -r`. `docs/bash-test-suite-baseline.md:214` asserts restricted shell "is
not implemented and is not a huck feature" — this work reverses that stance.

Probing bash 5.2.21 (see "Ground truth" below) shows the existing checks diverge
from bash in wording, in redirect semantics, and in the *mechanism* used for
variable restriction — and that the bash man page is itself wrong in several
places.

## Ground truth (bash 5.2.21, measured — not from the man page)

Every claim below was verified by probing the real shell. Where the man page
disagrees, the probe wins.

**Message shapes.** Every message huck emits today is wrong:

| op | huck today | bash 5.2.21 |
|---|---|---|
| cd | `restricted: cd` | `cd: restricted` |
| exec | `restricted: exec` | `exec: restricted` |
| command name | `restricted: /bin/echo: restricted` | ``/bin/echo: restricted: cannot specify `/' in command names`` |
| source | `restricted: source: paths with '/'` | `.: /etc/profile: restricted` |
| redirect | `restricted: /tmp/x` | `/tmp/x: restricted: cannot redirect output` |
| `set +r` | `restricted: cannot turn off restricted mode` | `set: +r: invalid option` + usage line, rc=1 |

**Redirection is target-dependent, not operator-dependent.** bash denies every
*file-target* output redirect — `>`, `>>`, `>|`, `<>`, `&>`, and `>& file` — all
with `<path>: restricted: cannot redirect output`. But fd-duplication (`>&2`,
`2>&1`) and input redirection (`<`) remain **allowed**. huck today permits
relative file paths and denies only absolute-or-`..` ones.

**Variable restriction is readonly-marking, not a check.** bash marks
`SHELL`, `PATH`, `HISTFILE`, `ENV`, `BASH_ENV` readonly when restriction
engages; every write path then reports through ordinary readonly machinery with
*that path's* normal wording, never mentioning restriction:

```
PATH=/tmp         → PATH: readonly variable
PATH+=/tmp        → PATH: readonly variable
export PATH=/tmp  → PATH: readonly variable
read PATH         → PATH: readonly variable
declare PATH=/tmp → declare: PATH: readonly variable
unset PATH        → unset: PATH: cannot unset: readonly variable
```

huck's two `check_special_assign` call sites cover only plain assignment —
`export`, `read`, `declare`, `unset`, and `+=` all escape them — and omit
`HISTFILE` entirely.

**Man-page corrections.** Three claims in `man bash` are false against 5.2.21:

- `shopt -u restricted_shell` is described as disallowed. It is a **silent
  no-op**, rc=0, and the option stays `on`. `shopt -s restricted_shell` in a
  normal shell is *also* a no-op. It is a read-only indicator, not an entry
  point, in either direction.
- There is no `restricted` long option. `set -o restricted` and
  `set +o restricted` both give `set: restricted: invalid option name`, rc=2,
  in restricted *and* normal shells.
- "turning off restricted mode with `set +r`" understates the mechanism:
  `set +r` **succeeds** (rc=0) in a normal shell. The refusal works by making
  `+r` an *invalid option* while restricted, which is why the output is a usage
  message rather than a restriction diagnostic.

**Propagation.** Restrictions apply inside functions, subshells, and command
substitutions. A prefix assignment (`PATH=/tmp cmd`) is refused, not just a
standalone one.

**Script-spawn exemption is real.** `bash -r -c 's.sh'` runs the script with
restrictions *off* — confirmed by probe. Deferred to v320.

## Design

### The abstraction

New `crates/huck-engine/src/policy.rs` replaces `restricted.rs`.
`Shell.restricted: bool` becomes `Shell.policy: Policy`.

```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Policy {
    Unrestricted,
    Rbash,    // bash-exact
    Sandbox,  // huck's embedding semantics: block escape, allow local work
}

pub enum Op<'a> {
    Cd,
    Exec,
    CommandName(&'a str),
    SourcePath(&'a str),
    RedirectFile { path: &'a str },
    DisableRestricted,
}

impl Policy {
    pub fn check(&self, op: Op<'_>) -> Result<(), String>;
}
```

Every enforcement site becomes one line:

```rust
shell.policy.check(Op::Cd)?;
```

`Policy::Unrestricted` returns `Ok(())` from the first match arm — the
"approves everything" path is a single branch with no per-op cost.

Three properties this buys, in order of how much they matter:

1. **Adding a restriction is compiler-checked.** A new `Op` variant fails to
   compile until every policy handles it. This is the whole point of the enum
   over scattered conditionals, and it is what makes v320 a mechanical
   extension rather than another archaeology exercise.
2. **`Shell` stays cheaply `Clone`.** `Policy` is `Copy`, so fork/subshell/
   capture paths inherit it exactly as the `bool` did. Given how reliably this
   codebase's exec paths need sibling fixes, avoiding new fork-path work is
   deliberate, not incidental.
3. **One call form everywhere**, so the enforcement sites read as policy
   queries rather than as bespoke logic.

**The policy owns the message, not the op.** `Rbash` and `Sandbox` deny
overlapping-but-different things, so `check` dispatches policy-first, op-second,
and each arm formats its own body. The returned string is body-only (no
invocation-name prefix), preserving the existing "callers translate" contract
shared with `shell_state::declare_err_message` — the site emits it through
`sh_error!`/`sh_error_to!`.

**Fd-duplication never reaches the gate.** Because `>&2` is allowed and
`>& file` is not, the call site passes `Op::RedirectFile` only once the redirect
has resolved to a file target. The policy does not re-parse redirect syntax.
This is a constraint on the *call sites*, and redirect machinery in this
codebase is duplicated across the fg-pipeline, bg, subshell, and capture paths —
so the implementation must audit **all** of them, not only the single site
`executor.rs:55` guards today.

### What `Op::Assign` is not

The obvious design has an `Op::Assign(&str)` variant checked at each write site.
**Rejected**, because bash does not work that way and neither should we: the
restriction is readonly-marking, and huck already has
`Shell::mark_readonly(&mut self, name: &str)` at `shell_state.rs:2350`.

On entering a restricted policy, mark `SHELL`, `PATH`, `HISTFILE`, `ENV`,
`BASH_ENV` readonly. All six write paths, their exact per-path wordings, and
their control-flow behavior (v313 aligned readonly-assignment to discard the
current command) then follow for free.

This is strictly less code than today *and* strictly more correct: it replaces
two hand-placed sites that miss four write paths. It is also the clearest case
in this design where the right abstraction turned out not to be a filter at all.

### Entry points

`repl.rs:101` already selects POSIX mode via
`shell::startup_posix(opts.posix, &argv0, POSIXLY_CORRECT)` — a flag-or-argv0
helper. Restricted mode takes the same shape: a new
`shell::startup_restricted(opts.restricted, &argv0)` called alongside it. No new
mechanism.

Entering `Policy::Rbash`:
- `argv[0]` basename is `rbash`
- `-r` at invocation (new field in `parse_cli`)
- `set -r` at runtime (new arm in `set`'s option loop; `-r` is unhandled today)

Entering `Policy::Sandbox`: `ExecBuilder::restricted()`, unchanged.

`shopt restricted_shell` becomes a **read-only indicator**, reporting `on` under
either restricted policy — a script asking "am I restricted?" wants yes. Both
`-s` and `-u` are silent no-ops returning 0, matching bash in both directions.
The shopt table entry already exists at `shell_state.rs:527`; it reports the
policy instead of a stored bit.

### The one-way property

While the policy is restricted, `+r` is rejected by `set`'s **existing
invalid-option path** (`set: +r: invalid option` + usage, rc=1) rather than by a
bespoke refusal. This is the `Op::DisableRestricted` query, and it is the only
op whose site is an option parser rather than an operation.

In `Unrestricted`, `set +r` keeps succeeding at rc=0, matching bash.

No support for `set -o restricted` / `set +o restricted` is added: bash has no
such long option, and huck's `set -o` table must keep rejecting the name with
`restricted: invalid option name`, rc=2.

### `Sandbox`

Preserves today's behavior exactly, re-worded to bash's vocabulary. Denies
`Cd`, `Exec`, `CommandName` containing `/`, `SourcePath` containing `/`, and
`DisableRestricted`. Denies `RedirectFile` **only** when the path is absolute or
contains a `..` component.

That path-conditional redirect rule is the one place the two policies differ in
logic rather than wording — and the reason a capability-bitflag design could not
have worked: `Sandbox` must *inspect* the target, not just deny the category.

One drift fixed while here: `Sandbox` marks `HISTFILE` readonly alongside the
other four. Its omission today reads as an oversight rather than a decision.

**Both policies use bash's message vocabulary**, differing only in what they
deny. The embedder-visible strings were never a documented contract; the only
affected assertions are huck's own tests in `engine.rs`.

## Verification

Three layers, in descending order of what they actually prove.

1. **`tests/scripts/rbash_diff_check.sh`** — the gold standard, and the real
   proof. Each fragment runs through `bash -r -c` and `huck -r -c`, normalized
   with the established convention
   (`norm() { sed -e 's#^bash: line [0-9]*: #SH: #' -e 's#^bash: #SH: #' ... }`),
   asserting byte-identical stdout, stderr, and exit status. Coverage:
   - every op, in both its denied and its permitted form
   - every redirect operator: `>`, `>>`, `>|`, `<>`, `&>`, `>& file`
   - the **allowed** cases that must not regress: `<` input, `>&2`, `2>&1`,
     bare command names
   - all six variable write paths, plus prefix assignment (`PATH=x cmd`)
   - `set +r` in both modes; `set -o restricted` / `set +o restricted` in both
   - `shopt restricted_shell` with `-s`, `-u`, and bare query
   - propagation into functions, subshells, and command substitutions

   This harness is what would have caught the six wrong messages that only
   surfaced by probing, so it is the centerpiece rather than an afterthought.

2. **Policy matrix unit test** in `policy.rs` — every `Op` × every `Policy` →
   expected decision, as one table. This is what makes the extensibility claim
   testable: a v320 `Op` variant fails to compile until the matrix covers it.

3. **Updated `engine.rs` `Sandbox` tests** for the new wording.

Per this repo's constraints, tests run per-crate
(`cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`), and the
`-p huck` integration binaries run **before pushing** — a change to `Shell`'s
fields has caused CI-only failures before (v289, v313). The full
`tests/scripts/run_diff_checks.sh` sweep must be green before the PR.

## Scope

**In scope (v319).** The `Policy`/`Op` abstraction; conversion of all
~9 existing sites; the message and redirect-semantics corrections; variable
restriction via readonly-marking; the `rbash` / `-r` / `set -r` entry points;
the `shopt restricted_shell` indicator; the one-way property; the diff harness.

**Deferred to v320.** New `Op` variants for `history` with a `/` filename,
`hash -p` with a `/` filename, `enable -f`/`-d`, `enable` re-enabling a disabled
builtin, and `command -p`; refusing `BASH_FUNC_x%%` function import at startup
(huck implements this at `shell_state.rs:1127`); bash's script-spawn exemption;
and the `rsh` bash-suite category, with the retirement of
`docs/bash-test-suite-baseline.md:214`.

The split is clean because v320 is *only* new `Op` variants against an interface
v319 freezes — which is the extensibility claim being tested for real rather
than asserted.

**Not applicable.** Parsing `SHELLOPTS` from the environment: huck never reads
it, so there is nothing to enforce and dead code would be worse than a note.

**Vacuous today.** "Restrictions are enforced after startup files are read":
huck reads no startup files (`repl.rs` has no bashrc/profile path). Recorded
here as a constraint on whoever adds them.

## Documentation

`ExecBuilder::restricted()`'s doc comment and `docs/architecture.md:49-52` both
describe `restricted.rs` and its rules, and must be updated to the policy model.
The `docs/bash-test-suite-baseline.md:214` line stays until v320, when the `rsh`
category is actually addressed.
