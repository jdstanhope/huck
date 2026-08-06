# v358 — the error-fatality classifier — design

**Umbrella:** [#198](https://github.com/jdstanhope/huck/issues/198) — *Architecture:
unify the error-fatality decision (abort vs continue, and exit code) across error
sites.*

**Members fixed through the classifier:**
[#116](https://github.com/jdstanhope/huck/issues/116) (`history` too-many-arguments
does not abort the list), [#25](https://github.com/jdstanhope/huck/issues/25) (a
malformed backtick substitution aborts where bash continues),
[#68](https://github.com/jdstanhope/huck/issues/68) (its `set -Q` fatality edge only;
the two message-text gaps there are out of scope), and
[#490](https://github.com/jdstanhope/huck/issues/490) (`${x[}` exits the shell where
bash aborts only the list) — filed during this brainstorm.

**Follows:** v354 ([#466](https://github.com/jdstanhope/huck/issues/466)), which
unified how an unwind *travels*, and v355/v356
([#480](https://github.com/jdstanhope/huck/issues/480),
[#483](https://github.com/jdstanhope/huck/issues/483)), which unified whether a
failure *counts*. Those are two of #198's three legs. This is the third: deciding
whether an error is fatal at all, and with what code.

## Problem

huck answers "is this error fatal, and with what rc" at each error site
independently. The result is not a bias in one direction — it is no correlation
with bash at all. Both errors occur, often in adjacent code.

### Measured: bash has three outcomes, and huck disagrees on five of 28 cells

The discriminator is a **two-line script**, because `-c` cannot distinguish
"abort the rest of this command list" from "exit the shell":

```
line 1:   <ERROR>; echo SAME
line 2:   echo NEXT

SAME NEXT -> continue        NEXT -> abort the LIST        (empty) -> exit the SHELL
```

Against bash 5.2.21, non-interactive, 2026-08-06:

| error | posix | bash | huck |
|---|---|---|---|
| arithmetic `$((1/0))` | off | abort list | abort list |
| assignment to readonly | off | abort list | abort list |
| `set -u` on unset | off | exit shell | exit shell |
| `$( )` syntax error | off | exit shell | exit shell |
| command not found / redirect failure / `cd` / `kill` | off | continue | continue |
| special-builtin usage (`set -Q`, `export -Q`, `unset -Q`) | off | continue | continue |
| **`history` too many args** | off | **abort list** | **continue** (#116) |
| **`${x[}` bad substitution** | off | **abort list** | **exit shell** (#490) |
| **backtick syntax error** | off | **continue** | **exit shell** (#25) |
| **`set -Q`** | **on** | **exit shell** | **continue** (#68) |
| arithmetic, readonly, `export -Q`, `unset -Q` | on | exit shell | exit shell |

Two further cells agree on the outcome and differ on the **exit code** — invisible
to an outcome-only probe, and the reason #198 says "abort vs continue, *and* exit
code".

### Measured: the exit code is a property of the DRIVER, not of the error

| case | bash `-c` | bash script | bash stdin | huck (all three) |
|---|---|---|---|---|
| posix arithmetic | 127 | 1 | 1 | 127 |
| posix readonly assignment | 127 | 1 | 1 | 127 |
| `set -u` unset | 127 | 1 | 1 | 1 |

Extending the probe to syntax errors pins the rule exactly, and it is NOT simply
"the driver picks the code":

| case | bash `-c` | bash script | bash stdin | huck (`-c` / script / stdin) |
|---|---|---|---|---|
| plain syntax error (`if`) | 2 | 2 | 2 | 2 / 2 / 2 — correct |
| `$( )` syntax error | 127 | 2 | 2 | **2** / 2 / 2 |
| `set -u` unset | 127 | 1 | 1 | **1** / 1 / 1 |
| posix arithmetic / readonly | 127 | 1 | 1 | 127 / **127** / **127** |
| backtick syntax error | 0 (continues) | 0 | 0 | **2 / 2 / 2** (#25) |

**The rule: each kind has its own exit code — 1 for an expansion or assignment
fatal, 2 for a syntax error — and the `-c` driver substitutes 127 for it. The one
exception is a plain top-level syntax error, which stays 2 under every driver**
(bash rejects it before execution begins, so the `-c` substitution never
applies).

huck has this half-right twice, and the code shows exactly why — two families of
sites, each hardcoding the other's answer:

```rust
// expand.rs:1331, expand.rs:2003, executor.rs:4194, executor.rs:4204
if shell.shell_options.posix && !shell.is_interactive {
    shell.posix_fatal(127);        // always 127, even from a script
} else {
    shell.raise_discard();
}

// expand.rs x11 — unbound variable / bad substitution
shell.raise_fatal(1);              // always 1, even under -c
```

`Shell::posix_fatal(status)` is a proto-classifier: it already encapsulates
"posix && !interactive" but takes the code **from the caller**, and all four
callers pass a constant.

### The structural hole

`Shell::raise_fatal` and `Shell::raise_discard` are `pub`. Any site may declare an
error fatal; 24 non-test sites do, each deciding alone. Nothing prevents the 25th
from deciding differently, which is how this cluster keeps growing.

## Design

### 1. One module owns the decision

`crates/huck-engine/src/error_fatality.rs`:

```rust
/// What KIND of error occurred. Not a message — a classification.
pub enum ErrorKind {
    /// Arithmetic error, bad substitution, and the other expansion failures.
    Expansion,
    /// An unset variable under `set -u`.
    UnsetUnderNounset,
    /// A POSIX special builtin rejected its options or operands.
    SpecialBuiltinUsage,
    /// Any other builtin error. Measured: ALWAYS continue — see the note below.
    BuiltinError,
    /// `history` with too many arguments. Its own kind because it is the ONLY
    /// builtin error in bash that aborts the list (measured across 15 cases).
    HistoryTooManyArgs,
    /// A syntax error inside a command substitution. `backtick` matters: a
    /// backtick body is parsed during EXPANSION and its error is reported
    /// without killing the shell, while `$( )` is parsed with the script and
    /// its error is a shell syntax error.
    ComsubSyntax { backtick: bool },
    /// A syntax error in the script itself.
    Syntax,
}

/// The three outcomes bash actually has.
pub enum Fatality {
    Continue,
    AbortList,
    ExitShell(i32),
}

/// The ONLY place an error's fatality is decided.
pub fn fatality(kind: ErrorKind, shell: &Shell) -> Fatality;
```

Two inputs, as measured. **Kind + posix mode** select the outcome. For
`ExitShell`, the **kind** supplies a base code (1 for an expansion or assignment
fatal, 2 for a syntax error) and the **driver** substitutes 127 under `-c` —
except for `ErrorKind::Syntax`, which keeps 2 under every driver because bash
rejects it before execution begins. That exception is a measured fact and gets a
unit test of its own, not a comment.

### 2. The decision is enforced, not merely offered

`Shell::raise_fatal` and `Shell::raise_discard` become private to the crate's
unwind internals, reachable only from `error_fatality`. The sole public entry
becomes:

```rust
impl Shell {
    /// Classify an error and raise whatever unwind it deserves.
    pub fn report_error(&mut self, kind: ErrorKind);
}
```

The privacy is load-bearing, not tidiness — it is the same mechanism that made
v356's `Suppression` fix stick: the compiler rejects a site that tries to decide
for itself, so the 25th site cannot re-diverge. The ~360 error sites that merely
emit a message need no edit; after this they are **structurally incapable** of
being anything but `Continue`, where today they are `Continue` by accident.

`Shell::posix_fatal` is deleted — it is this design, half-built, with the code
supplied by the wrong party.

### 3. How the decision travels — unchanged

v354 already built this leg. `AbortList` raises the existing discard;
`ExitShell(n)` raises the existing fatal. `pending_unwind(shell, phase)` remains
the single reporter and the `Around`/`After` asymmetry is untouched. This
iteration adds no new unwind signal and no new checkpoint.

### 4. Migration

| sites | kind | change |
|---|---|---|
| `expand.rs` x11 | `UnsetUnderNounset` / `Expansion` | code from the driver, not `1` |
| `posix_fatal` x4 | `Expansion` | code from the driver, not `127` |
| `expand.rs` x2 | `Expansion` (`ExpansionResult::Fatal` passthrough) | routed, behaviour unchanged |
| `shell.rs:557` | `Syntax` / `ComsubSyntax` | gains the backtick split — #25 |
| `param_expansion.rs` | `Expansion` | routed |
| builtin usage sites | `SpecialBuiltinUsage` / `BuiltinError` / `HistoryTooManyArgs` | new — #68, #116 |

`shell.rs:557` already reads the `top_level` flag, so the driver distinction the
classifier needs is already threaded to the site that most needs it. That is the
reader-vs-eval-vs-sourced work paying off rather than new plumbing.

### 5. What is deliberately NOT done

- **Rewriting the ~360 emit-only sites to call the classifier.** They would ask
  "is this fatal?" and always hear "no". Privacy already guarantees that answer;
  asking for it costs a large diff and buys nothing.
- **#68's message-text gaps** (`shift 99` prints no diagnostic, `unset -Q` omits
  its usage line). Real, measured, and unrelated to fatality — they stay on #68.
- **A general "builtin usage error" fatality rule.** Measured across 15 cases:
  every builtin error except `history`-too-many-args continues. Inventing a rule
  here would be fitting a line to one point.

## Verification

1. **A fatality matrix harness** — `error_fatality_diff_check.sh`, error kind x
   posix x driver, using the two-line-script discriminator. Committed RED. The
   28 cells above are the floor; the builtin sub-matrix (15 cases) goes in as the
   "must not change" set.
   - ⚠️ The harness header must state the discriminator, because `-c` alone
     silently conflates abort-list with exit-shell.
   - ⚠️ It must capture the exit code SEPARATELY. `rc=$?` after a pipeline reads
     the pipeline's last stage, not the shell — this reported false agreement
     twice while measuring for this spec.
2. **Exit codes change for cases people may assert.** `set -u` under `-c` becomes
   127; posix arithmetic and posix readonly from a script or stdin become 1
   (from 127). A `$( )` syntax error keeps 2 from a script but becomes 127 under
   `-c`. The plan MUST grep the test tree for those constants before touching the
   sites — v340's lesson that a
   semantics change needs `tests/*.rs` swept, and the kill/job-control cascade's
   lesson that tests asserting the old behaviour look like regressions.
3. **No expected-value edits** in the existing error harnesses. If one needs its
   expectation changed, the change went beyond the measured contract — except
   for the two exit-code cases named above, which are expected to move and must
   be justified row by row.
4. **Full sweep** green, and the bash-suite PASS-set diff vs `main` (the
   `errors`, `posix2`, `set-e` categories are the ones that could move).
   ⚠️ Baseline via `git checkout main` with the `huck commit:` stamp checked, and
   the row COUNT printed next to any "identical" claim — an empty-set comparison
   reported a false "IDENTICAL" during v356.
5. **CI green before handover** — a `vNN` iteration PR is the user's to merge.

## Success criteria

`fatality()` is the only function that decides whether an error is fatal, the
compiler enforces it, and #116, #25, #68's `set -Q` edge and #490 are all fixed
through it rather than at their sites. A new error site inherits the right
behaviour by default because it cannot express a wrong one.
