# Error-Fatality Classifier (v358) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make one function the only thing in huck that decides whether an error is fatal and with what exit code, and make the compiler enforce it.

**Architecture:** A new `error_fatality` module owns an `ErrorKind` → `Fatality` decision. `Shell::raise_fatal` / `raise_discard` become private to it, so no other site can declare an error fatal. The existing v354 unwind machinery carries the result unchanged — this adds no new unwind signal and no new checkpoint.

**Tech Stack:** Rust (workspace crates `huck-engine`, `huck-cli`), bash-differential harnesses under `tests/scripts/*_diff_check.sh`.

**Spec:** `docs/superpowers/specs/2026-08-06-error-fatality-classifier-design.md`

**Issues:** umbrella [#198](https://github.com/jdstanhope/huck/issues/198); members [#116](https://github.com/jdstanhope/huck/issues/116), [#25](https://github.com/jdstanhope/huck/issues/25), [#68](https://github.com/jdstanhope/huck/issues/68) (its `set -Q` edge only), [#490](https://github.com/jdstanhope/huck/issues/490).

## Global Constraints

- **Every rule in this plan is MEASURED against bash 5.2.21.** Do not "improve" a value because it looks wrong. If a measurement seems absurd (a plain syntax error exits 2 under `-c` while a `$( )` syntax error exits 127), it is still the contract.
- **Commit trailer** on every commit: `Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>`.
- **`cargo fmt --all` before every commit** — CI enforces `cargo fmt --all --check`.
- **Run tests per-crate, never `--workspace`** — this box has 1 core / 1.9 GB and `cargo test --workspace` OOM-kills the session. Use `cargo test -p huck-engine --lib -- --test-threads 4` and `cargo test -p huck --test <name> --jobs 1 -- --test-threads 1`.
- **Guard every bash/huck probe** with `ulimit -v 800000`, `timeout`, and `head -c`. An unbounded probe has OOM-killed this box twice.
- **The full sweep exceeds the 10-minute Bash-tool cap.** Run `tests/scripts/run_diff_checks.sh` with `run_in_background: true`.
- **Do not edit an existing test's expected value** except the two cases named in Task 7. If any other test needs its expectation changed, the change went beyond the measured contract — stop and report.

## File Structure

| file | responsibility |
|---|---|
| `crates/huck-engine/src/error_fatality.rs` | **new.** `ErrorKind`, `Fatality`, `fatality()`. Pure decision + unit tests. The only module allowed to raise fatality. |
| `crates/huck-engine/src/shell_state.rs` | `raise_fatal`/`raise_discard` lose `pub`; `posix_fatal` is deleted; gains `report_error`. Reads the EXISTING `is_command_string`. |
| `crates/huck-engine/src/shell.rs` | `shell.rs:557` routes its syntax error through the classifier. |
| `crates/huck-engine/src/expand.rs` | 13 raise sites route through `report_error`. |
| `crates/huck-engine/src/param_expansion.rs` | 1 raise site routes through `report_error`. |
| `crates/huck-engine/src/executor.rs` | 2 `posix_fatal` sites route through `report_error`. |
| `crates/huck-engine/src/builtins.rs` | `history` too-many-args and special-builtin usage errors route through `report_error`. |
| `tests/scripts/error_fatality_diff_check.sh` | **new.** The kind x posix x driver matrix. |

---

### Task 1: ~~`Invocation`~~ — WITHDRAWN, the field already exists

**Do not implement this task.** It was written on a false premise and the
correction is the point: `Shell` ALREADY records the driver, as
`pub is_command_string: bool` (`shell_state.rs:840`) — *"True when the shell
was invoked as `huck -c '<command>'`"*. It defaults to `false` and the CLI sets
it at `repl.rs:166`; `Engine::set_is_command_string` is the public setter.

The brainstorm searched for `invocation` / `InvocationMode` / `dash_c` and
concluded no such field existed. It exists under a name none of those matched.

It is sufficient. The classifier needs to distinguish `-c` from everything
else, and nothing more: script and stdin were MEASURED to behave identically
(both keep the kind's base code; only `-c` substitutes 127). A three-variant
`Invocation` enum would encode a distinction that carries no behaviour.

Proof it is wired, since the classifier depends on it:

```
$ huck -c 'if'          ->  huck: -c: line 2: syntax error ...
$ huck script.sh        ->  script.sh: line 2: syntax error ...
$ printf 'if\n' | huck  ->  huck: line 2: syntax error ...
```

The `-c:` segment appears under `-c` alone.

**Consequence for Task 2:** `driver_code` reads `shell.is_command_string`
rather than a new field:

```rust
fn driver_code(base: i32, shell: &Shell) -> i32 {
    if shell.is_command_string { 127 } else { base }
}
```

and every `true` / `false` / `false` in
Task 2's tests becomes `is_command_string = true` / `false`.

**Not** widened to make `Engine::run` (which has `bash -c` semantics) set it:
`is_command_string` also drives the `-c:` error-prologue segment, so flipping
its default would change error text for every embedder. Out of scope, and the
CLI path the harness exercises is already correct.

---

### Task 2: The classifier

**Files:**
- Create: `crates/huck-engine/src/error_fatality.rs`
- Modify: `crates/huck-engine/src/lib.rs` (add `mod error_fatality;`)

**Interfaces:**
- Consumes: `Shell::is_command_string` (already exists; Task 1 withdrawn).
- Produces: `ErrorKind`, `Fatality`, `fatality(kind, shell) -> Fatality`. Tasks 4-6 consume these.

- [ ] **Step 1: Write the failing tests**

```rust
use super::*;
use crate::shell_state::Shell;

/// `dash_c` is `Shell::is_command_string` — the EXISTING field that records
/// `huck -c`. Script and stdin both spell it `false`, which is correct: they
/// were measured to behave identically.
fn shell_with(posix: bool, dash_c: bool) -> Shell {
    let mut s = Shell::new();
    s.shell_options.posix = posix;
    s.is_interactive = false;
    s.is_command_string = dash_c;
    s
}

#[test]
fn expansion_error_aborts_the_list_outside_posix() {
    let s = shell_with(false, false);
    assert_eq!(fatality(ErrorKind::Expansion, &s), Fatality::AbortList);
}

#[test]
fn expansion_error_exits_in_posix_with_the_drivers_code() {
    // script and stdin are the same spelling (`is_command_string == false`),
    // which is the measured truth rather than a simplification.
    assert_eq!(
        fatality(ErrorKind::Expansion, &shell_with(true, false)),
        Fatality::ExitShell(1)
    );
    assert_eq!(
        fatality(ErrorKind::Expansion, &shell_with(true, true)),
        Fatality::ExitShell(127)
    );
}

#[test]
fn nounset_always_exits_with_the_drivers_code() {
    assert_eq!(
        fatality(ErrorKind::UnsetUnderNounset, &shell_with(false, false)),
        Fatality::ExitShell(1)
    );
    assert_eq!(
        fatality(ErrorKind::UnsetUnderNounset, &shell_with(false, true)),
        Fatality::ExitShell(127)
    );
}

#[test]
fn special_builtin_usage_is_fatal_only_in_posix() {
    assert_eq!(
        fatality(ErrorKind::SpecialBuiltinUsage, &shell_with(false, false)),
        Fatality::Continue
    );
    assert_eq!(
        fatality(ErrorKind::SpecialBuiltinUsage, &shell_with(true, false)),
        Fatality::ExitShell(2)
    );
}

#[test]
fn ordinary_builtin_errors_always_continue() {
    for posix in [false, true] {
        assert_eq!(
            fatality(ErrorKind::BuiltinError, &shell_with(posix, false)),
            Fatality::Continue
        );
    }
}

#[test]
fn history_too_many_args_aborts_the_list_in_both_modes() {
    // The ONLY builtin error in bash that aborts. Measured across 15 cases:
    // `cd -Q`, `kill -Q`, `read -Q`, `getopts`, `umask a b`, `shift a b`,
    // `break 1 2` and the rest all continue.
    for posix in [false, true] {
        assert_eq!(
            fatality(ErrorKind::HistoryTooManyArgs, &shell_with(posix, false)),
            Fatality::AbortList
        );
    }
}

#[test]
fn backtick_comsub_syntax_error_continues_but_dollar_paren_exits() {
    let s = shell_with(false, false);
    assert_eq!(
        fatality(ErrorKind::ComsubSyntax { backtick: true }, &s),
        Fatality::Continue
    );
    assert_eq!(
        fatality(ErrorKind::ComsubSyntax { backtick: false }, &s),
        Fatality::ExitShell(2)
    );
}

#[test]
fn dollar_paren_syntax_error_takes_127_under_dash_c() {
    assert_eq!(
        fatality(
            ErrorKind::ComsubSyntax { backtick: false },
            &shell_with(false, true)
        ),
        Fatality::ExitShell(127)
    );
}

#[test]
fn plain_syntax_error_keeps_2_under_every_driver() {
    // THE EXCEPTION, and the reason this is a test rather than a comment:
    // bash rejects a top-level syntax error before execution begins, so the
    // `-c` substitution never applies. `bash -c 'if'` exits 2, not 127.
    for dash_c in [true, false] {
        assert_eq!(
            fatality(ErrorKind::Syntax, &shell_with(false, dash_c)),
            Fatality::ExitShell(2)
        );
    }
}

#[test]
fn an_interactive_shell_is_never_killed_by_an_error() {
    let mut s = shell_with(true, false);
    s.is_interactive = true;
    assert_ne!(
        fatality(ErrorKind::SpecialBuiltinUsage, &s),
        Fatality::ExitShell(2)
    );
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p huck-engine --lib error_fatality -- --test-threads 4`
Expected: FAIL — module does not exist.

- [ ] **Step 3: Implement**

```rust
//! The ONE place huck decides whether an error is fatal, and with what code.
//!
//! Before v358 this was answered ad hoc at 24 sites and the results were not
//! merely inconsistent — they were uncorrelated with bash, wrong in BOTH
//! directions (huck exited where bash continued, #25, and continued where bash
//! aborted, #116). `Shell::raise_fatal` / `raise_discard` are private to this
//! module so a site cannot decide for itself.

use crate::shell_state::Shell;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// Arithmetic error, bad substitution, and other expansion failures.
    Expansion,
    /// An unset variable under `set -u`.
    UnsetUnderNounset,
    /// A POSIX special builtin rejected its options or operands.
    SpecialBuiltinUsage,
    /// Any other builtin error. Measured: ALWAYS continues.
    BuiltinError,
    /// `history` with too many arguments — the only builtin error in bash that
    /// aborts the list. Its own kind because a general rule fitted to one data
    /// point would be a fabrication.
    HistoryTooManyArgs,
    /// A syntax error inside a command substitution. `backtick` matters: a
    /// backtick body is parsed during EXPANSION and its error is reported
    /// without killing the shell, while `$( )` is parsed with the script.
    ComsubSyntax { backtick: bool },
    /// A syntax error in the script itself.
    Syntax,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fatality {
    Continue,
    AbortList,
    ExitShell(i32),
}

/// `-c` substitutes 127 for the kind's own code; a script or stdin keeps it.
fn driver_code(base: i32, shell: &Shell) -> i32 {
    match shell.invocation {
        true => 127,
        false | false => base,
    }
}

pub fn fatality(kind: ErrorKind, shell: &Shell) -> Fatality {
    // An interactive shell is never killed by one of these; it returns to the
    // prompt. Checked first so no rule below has to repeat it.
    let can_exit = !shell.is_interactive;
    let posix = shell.shell_options.posix;

    match kind {
        ErrorKind::Expansion => {
            if posix && can_exit {
                Fatality::ExitShell(driver_code(1, shell))
            } else {
                Fatality::AbortList
            }
        }
        ErrorKind::UnsetUnderNounset => {
            if can_exit {
                Fatality::ExitShell(driver_code(1, shell))
            } else {
                Fatality::AbortList
            }
        }
        ErrorKind::SpecialBuiltinUsage => {
            if posix && can_exit {
                Fatality::ExitShell(driver_code(2, shell))
            } else {
                Fatality::Continue
            }
        }
        ErrorKind::BuiltinError => Fatality::Continue,
        ErrorKind::HistoryTooManyArgs => Fatality::AbortList,
        ErrorKind::ComsubSyntax { backtick } => {
            if backtick || !can_exit {
                Fatality::Continue
            } else {
                Fatality::ExitShell(driver_code(2, shell))
            }
        }
        // The `-c` substitution does NOT apply: bash rejects a top-level
        // syntax error before execution begins. `bash -c 'if'` exits 2.
        ErrorKind::Syntax => {
            if can_exit {
                Fatality::ExitShell(2)
            } else {
                Fatality::Continue
            }
        }
    }
}
```

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p huck-engine --lib error_fatality -- --test-threads 4`
Expected: PASS (10 tests).

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add -A && git commit -m "feat(#198): the error-fatality classifier

Pure decision, no wiring yet. Every rule is measured against bash 5.2.21;
the two that look wrong and are not: a plain top-level syntax error keeps 2
under \`-c\` where every other fatal takes 127, and only \`history\` with too
many arguments aborts the list among 15 measured builtin errors.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: The matrix harness, committed RED

**Files:**
- Create: `tests/scripts/error_fatality_diff_check.sh`

- [ ] **Step 1: Write the harness**

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for #198: when an error occurs, does it
# abort the current command LIST, exit the SHELL, or neither — and with what
# exit code?
#
# ⚠️ THE DISCRIMINATOR IS A TWO-LINE SCRIPT. `-c` cannot tell "abort the rest
# of this command list" from "exit the shell", because both suppress
# everything after the error. Two lines separate them:
#
#     line 1:   <ERROR>; echo SAME
#     line 2:   echo NEXT
#
#     SAME NEXT -> continue     NEXT -> abort LIST     (empty) -> exit SHELL
#
# ⚠️ CAPTURE THE EXIT CODE SEPARATELY. `rc=$?` after a pipeline reads the
# pipeline's last stage, not the shell. That reported false agreement TWICE
# while this contract was being measured.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT

# $1 label, $2 prelude, $3 error fragment, $4 driver (dashc|script|stdin)
check() {
    local label="$1" pre="$2" frag="$3" drv="$4" b h bo brc ho hrc
    printf '%s\n%s; echo SAME\necho NEXT\n' "$pre" "$frag" > "$TMP/s.sh"
    for sh in bash "$HUCK_BIN"; do
        case "$drv" in
            dashc)  o=$( ulimit -v 800000; timeout 10 "$sh" -c "$pre
$frag; echo SAME
echo NEXT" 2>/dev/null | head -c 400 ); c=$? ;;
            script) o=$( ulimit -v 800000; timeout 10 "$sh" "$TMP/s.sh" 2>/dev/null | head -c 400 ); c=$? ;;
            stdin)  o=$( ulimit -v 800000; timeout 10 "$sh" < "$TMP/s.sh" 2>/dev/null | head -c 400 ); c=$? ;;
        esac
        if [ "$sh" = bash ]; then bo="$o"; brc=$c; else ho="$o"; hrc=$c; fi
    done
    b="[$(echo $bo)] rc=$brc"; h="[$(echo $ho)] rc=$hrc"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s (%s)\n' "$label" "$drv"; PASS=$((PASS+1))
    else printf 'FAIL: %s (%s)\n    bash %s\n    huck %s\n' "$label" "$drv" "$b" "$h"; FAIL=$((FAIL+1)); fi
}

for drv in dashc script stdin; do
    # --- expansion ---------------------------------------------------------
    check "arith"              ''  'echo $((1/0))'          "$drv"
    check "bad substitution"   ''  'echo ${x[}'             "$drv"
    check "readonly assign"    'readonly r=1'  'r=2'        "$drv"
    check "nounset"            'set -u'  'echo $undef_zz'   "$drv"
    check "posix arith"        'set -o posix'  'echo $((1/0))' "$drv"
    check "posix readonly"     'set -o posix
readonly r=1'  'r=2'                                        "$drv"
    check "posix nounset"      'set -o posix
set -u'  'echo $undef_zz'                                   "$drv"
    # --- builtins ----------------------------------------------------------
    check "history too many"   ''  'history 1 2 3'          "$drv"
    check "history bad num"    ''  'history a'              "$drv"
    check "history opt"        ''  'history -Q'             "$drv"
    check "posix history many" 'set -o posix'  'history 1 2 3' "$drv"
    check "cd bad opt"         ''  'cd -Q'                  "$drv"
    check "cd missing dir"     ''  'cd /nonexistent-zz'     "$drv"
    check "kill bad opt"       ''  'kill -Q 1'              "$drv"
    check "read bad opt"       ''  'read -Q'                "$drv"
    check "getopts no args"    ''  'getopts'                "$drv"
    check "umask bad"          ''  'umask a b'              "$drv"
    check "special set -Q"     ''  'set -Q'                 "$drv"
    check "special unset -Q"   ''  'unset -Q x'             "$drv"
    check "special export -Q"  ''  'export -Q'              "$drv"
    check "special shift bad"  ''  'shift a b'              "$drv"
    check "break too many"     ''  'break 1 2'              "$drv"
    check "posix set -Q"       'set -o posix'  'set -Q'     "$drv"
    check "posix unset -Q"     'set -o posix'  'unset -Q x' "$drv"
    check "posix export -Q"    'set -o posix'  'export -Q'  "$drv"
    check "posix shift bad"    'set -o posix'  'shift a b'  "$drv"
    # --- syntax ------------------------------------------------------------
    check "backtick syntax"    ''  'echo `echo a; ; echo b`' "$drv"
    check "dollarparen syntax" ''  'echo $(echo a; ; echo b)' "$drv"
    check "posix backtick"     'set -o posix'  'echo `echo a; ; echo b`' "$drv"
    # --- must-not-change ---------------------------------------------------
    check "command not found"  ''  'no_such_cmd_zz'         "$drv"
    check "redirect failure"   ''  'echo hi > /proc/nope/x' "$drv"
    check "bad fd"             ''  'exec 3>&99'             "$drv"
    check "posix cmd notfound" 'set -o posix'  'no_such_cmd_zz' "$drv"
done

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
[[ $FAIL -eq 0 ]]
```

- [ ] **Step 2: Run it and record the RED count**

```bash
chmod +x tests/scripts/error_fatality_diff_check.sh
( ulimit -v 1500000; timeout 600 tests/scripts/error_fatality_diff_check.sh 2>&1 | tail -30 )
```

Expected: FAILS. Note the exact Pass/Fail numbers for the commit message. Every `must-not-change` row (command not found, redirect failure, bad fd) MUST pass at baseline — **if one of them is red now, stop and report**, because the contract is then not what the spec measured.

- [ ] **Step 3: Commit RED**

```bash
git add tests/scripts/error_fatality_diff_check.sh
git commit -m "test(#198): pin the error-fatality matrix, RED (<pass>/<total>)

kind x posix x driver, three drivers, asserted on OUTCOME and EXIT CODE.

Two traps are documented in the header because both produced false agreement
while this contract was being measured: \`-c\` cannot distinguish abort-list
from exit-shell (hence the two-line script), and \`rc=\$?\` after a pipeline
reads the pipeline, not the shell.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Route the expansion sites

Fixes #490 and the 127-vs-1 rule. This is where `posix_fatal` dies.

**Files:**
- Modify: `crates/huck-engine/src/shell_state.rs` — add `report_error`, delete `posix_fatal` (~3401)
- Modify: `crates/huck-engine/src/expand.rs` — 13 sites (448, 494, 656, 933, 940, 960, 989, 1202, 1225, 1331, 1358, 1468, 1695, 1967, 2003, 2060)
- Modify: `crates/huck-engine/src/param_expansion.rs` — the non-test raise site
- Modify: `crates/huck-engine/src/executor.rs` — 4194, 4204

**Interfaces:**
- Consumes: `fatality`, `ErrorKind`, `Fatality` from Task 2.
- Produces: `Shell::report_error(&mut self, kind: ErrorKind)`. Tasks 5-6 consume it.

- [ ] **Step 1: Add `report_error`**

In `shell_state.rs`, next to the unwind raisers:

```rust
/// Classify an error and raise whatever unwind it deserves. The ONLY public
/// way to make an error fatal — `raise_fatal` and `raise_discard` are sealed
/// in Task 7 so this cannot be bypassed.
pub fn report_error(&mut self, kind: crate::error_fatality::ErrorKind) {
    match crate::error_fatality::fatality(kind, self) {
        crate::error_fatality::Fatality::Continue => {}
        crate::error_fatality::Fatality::AbortList => self.raise_discard(),
        crate::error_fatality::Fatality::ExitShell(n) => self.raise_fatal(n),
    }
}
```

- [ ] **Step 2: Replace the four `posix_fatal` sites**

At `expand.rs:1331`, `expand.rs:2003`, `executor.rs:4194`, `executor.rs:4204`, each currently:

```rust
if shell.shell_options.posix && !shell.is_interactive {
    shell.posix_fatal(127);
} else {
    shell.raise_discard();
}
```

becomes:

```rust
shell.report_error(crate::error_fatality::ErrorKind::Expansion);
```

The posix test, the interactive test and the 127 all move into the classifier — the constant was wrong for a script driver anyway (bash gives 1).

- [ ] **Step 3: Replace the remaining expansion raises**

Every `shell.raise_fatal(1);` in `expand.rs` reached from an *unbound variable* diagnostic becomes:

```rust
shell.report_error(crate::error_fatality::ErrorKind::UnsetUnderNounset);
```

and every one reached from a *bad substitution* or other expansion diagnostic becomes:

```rust
shell.report_error(crate::error_fatality::ErrorKind::Expansion);
```

Use the message at each site to choose: `"{name}: unbound variable"` and `"{name}[{key}]: unbound variable"` are `UnsetUnderNounset`; `"bad substitution"` and the `"{e}"` arithmetic ones are `Expansion`. The `ExpansionResult::Fatal { status }` passthroughs at `expand.rs:1468` and `expand.rs:2060` keep `raise_fatal(status)` for now — they forward a status computed elsewhere, and Task 7 revisits them when the seal makes them a compile error.

- [ ] **Step 4: Delete `posix_fatal`**

Remove the method from `shell_state.rs`. The compiler will point at any site still calling it.

- [ ] **Step 5: Verify**

```bash
cargo build --locked --bin huck
( ulimit -v 1500000; timeout 600 tests/scripts/error_fatality_diff_check.sh 2>&1 | tail -20 )
( ulimit -v 4000000; timeout 900 cargo test -p huck-engine --lib -- --test-threads 4 )
for h in readonly_assign_discard arith_error_status arith_expansion_discard nounset_diff set_o_options; do
  [ -f tests/scripts/${h}_diff_check.sh ] && bash tests/scripts/${h}_diff_check.sh | tail -1
done
```

Expected: the expansion rows of the new harness go green in all three drivers. The neighbour harnesses stay green **with no expected-value edits**.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add -A && git commit -m "fix(#490,#198): route expansion errors through the classifier

Deletes \`posix_fatal\`, whose four callers all passed a hardcoded 127 while
the eleven expansion sites hardcoded 1 — two families each holding the other's
correct answer, which is the whole of the 127-vs-1 divergence.

Closes #490

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Route the syntax and command-substitution sites

Fixes #25.

**Files:**
- Modify: `crates/huck-engine/src/shell.rs:557` and the comsub error path that reaches it

- [ ] **Step 1: Find where a comsub syntax error is distinguished**

`shell.rs:555` already carries the comment *"…so their syntax errors stay non-fatal"* and `shell.rs:557` does `shell.raise_fatal(2)` gated on `top_level && !shell.is_interactive`. Locate the caller that reports a **command-substitution** syntax error and determine whether the body came from backticks or `$( )`. If the distinction is not available at the report site, thread a `bool backtick` from the lexer/parser error that produced it — do not guess it from the source text.

- [ ] **Step 2: Route**

```rust
// A script-level syntax error.
shell.report_error(crate::error_fatality::ErrorKind::Syntax);

// A syntax error inside a command substitution.
shell.report_error(crate::error_fatality::ErrorKind::ComsubSyntax { backtick });
```

- [ ] **Step 3: Verify**

```bash
cargo build --locked --bin huck
( ulimit -v 1500000; timeout 600 tests/scripts/error_fatality_diff_check.sh 2>&1 | tail -20 )
for h in cmdsub_comment syntax_error_shapes parse_error; do
  [ -f tests/scripts/${h}_diff_check.sh ] && bash tests/scripts/${h}_diff_check.sh | tail -1
done
```

Expected: the `backtick syntax`, `dollarparen syntax` and `posix backtick` rows go green in all three drivers. In particular `echo \`echo a; ; echo b\`; echo after` must now print `after` and exit 0.

- [ ] **Step 4: Commit**

```bash
cargo fmt --all
git add -A && git commit -m "fix(#25,#198): a backtick syntax error no longer kills the shell

A backtick body is parsed during EXPANSION, so bash reports its syntax error
and carries on; \$( ) is parsed with the script and its error is a shell
syntax error. huck exited for both. Measured, not inferred.

Closes #25

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: Route the builtin sites

Fixes #116 and #68's `set -Q` edge.

**Files:**
- Modify: `crates/huck-engine/src/builtins.rs` — the `history` too-many-arguments path and the special-builtin option/operand rejection paths

- [ ] **Step 1: `history` too many arguments**

Find the `history: too many arguments` emit site. After emitting:

```rust
shell.report_error(crate::error_fatality::ErrorKind::HistoryTooManyArgs);
```

Do **not** generalise this to other builtins. Measured across 15 cases, `history`-too-many-args is the only builtin error in bash that aborts the list.

- [ ] **Step 2: special-builtin usage errors**

For the POSIX special builtins (`:`, `.`, `break`, `continue`, `eval`, `exec`, `exit`, `export`, `readonly`, `return`, `set`, `shift`, `times`, `trap`, `unset`), an invalid option or operand emits its diagnostic and then:

```rust
shell.report_error(crate::error_fatality::ErrorKind::SpecialBuiltinUsage);
```

⚠️ `set -Q` currently reports *"not yet supported in this version"* and returns 0. bash reports `set: -Q: invalid option` plus the usage line and exits 2 in posix mode. Fix the message **and** the fatality — the harness rows check both, since they compare full output.

⚠️ `shift a b` and `break 1 2` are special builtins whose errors bash still **continues** past in both modes. They are `BuiltinError`, not `SpecialBuiltinUsage` — the rule is about *usage/option* rejection, not about every error a special builtin can raise. The harness has rows for both; if they go red you have over-applied the kind.

- [ ] **Step 3: Verify**

```bash
cargo build --locked --bin huck
( ulimit -v 1500000; timeout 600 tests/scripts/error_fatality_diff_check.sh 2>&1 | tail -25 )
( ulimit -v 4000000; timeout 900 cargo test -p huck-engine --lib -- --test-threads 4 )
for h in shopt set_o_options history builtin_usage; do
  [ -f tests/scripts/${h}_diff_check.sh ] && bash tests/scripts/${h}_diff_check.sh | tail -1
done
```

Expected: the whole harness is now green — all rows, all three drivers.

- [ ] **Step 4: Commit**

```bash
cargo fmt --all
git add -A && git commit -m "fix(#116,#68,#198): route builtin errors through the classifier

\`history\` with too many arguments aborts the list (the only builtin error in
bash that does, measured across 15 cases), and a special builtin's usage error
is fatal in POSIX mode. \`set -Q\` also stops claiming it is 'not yet
supported' and reports what bash reports.

#68's two message-text gaps (\`shift 99\` prints no diagnostic, \`unset -Q\`
omits its usage line) are NOT in scope and stay on the issue.

Closes #116

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 7: Seal the decision, then verify everything

The compiler errors from sealing ARE the completeness checklist — the v354 lesson.

**Files:**
- Modify: `crates/huck-engine/src/shell_state.rs` (`raise_fatal` ~3303, `raise_discard` ~3330)
- Modify: `docs/architecture.md`
- Create: `site/content/blog/<slug>.mdx`

- [ ] **Step 1: Seal**

Change `pub fn raise_fatal` and `pub fn raise_discard` to `pub(crate) fn`, and add:

```rust
// v358 (#198): NOT `pub`. Fatality is decided in one place, `error_fatality`,
// and reached through `report_error`. This visibility is the enforcement —
// the same mechanism that made v356's `Suppression` fix stick. A site that
// wants to decide for itself gets a compile error, not a divergence.
```

If the crate is one compilation unit, prefer moving the raisers behind a
narrower module boundary so `error_fatality` is genuinely the only caller.
Whatever the mechanism, the acceptance test is: **no site outside
`error_fatality` and `report_error` raises fatality.**

- [ ] **Step 2: Build and treat every error as a checklist item**

```bash
cargo build --locked --bin huck 2>&1 | grep -E "^error" -A 6
```

For each site the compiler rejects, classify it with an `ErrorKind` — do not
widen the visibility to make it compile. The `ExpansionResult::Fatal { status }`
passthroughs deferred in Task 4 land here: give them `ErrorKind::Expansion`
and let the classifier supply the code, or if the forwarded status is genuinely
not derivable, keep a narrow `pub(crate)` raiser and say why in a comment.

- [ ] **Step 3: Full verification**

```bash
cargo fmt --all --check
( ulimit -v 4000000; timeout 900 cargo test -p huck-engine --lib -- --test-threads 4 )
( ulimit -v 4000000; timeout 900 cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1 )
for t in set_options_integration param_indirect_extquote_integration script_mode_integration \
         command_not_found_integration subshell_integration trap_integration functions_integration; do
  ( ulimit -v 1500000; timeout 500 cargo test -p huck --test $t --jobs 1 -- --test-threads 1 ) | grep "^test result"
done
cargo build --release --locked --bin huck
# backgrounded — exceeds the 10-minute tool cap
( ulimit -v 1500000; timeout 3000 tests/scripts/run_diff_checks.sh > /tmp/v358_sweep.log 2>&1 )
```

**Expected test-tree behaviour, swept in advance so a red test is a real signal:**
- `param_indirect_extquote_integration.rs:89` asserts `rc == 1` for nounset but drives huck with `run_file` — the **script** driver, where 1 remains correct. It must stay green.
- `set_options_integration.rs::set_u_unset_errors` asserts `assert_ne!(rc, 0)` — driver-agnostic. Must stay green.
- No other test asserts an exit code for an expansion or syntax error.

**If any test needs its expected value edited, stop and report** — the sweep above says none should.

- [ ] **Step 4: bash-suite PASS-set diff vs main**

```bash
export BASH_SOURCE_DIR=/tmp/bash-5.2.21
tests/bash-test-suite/runner.sh > /tmp/suite_branch.log 2>&1
git checkout main && cargo build --release --locked --bin huck
tests/bash-test-suite/runner.sh > /tmp/suite_main.log 2>&1
git checkout - 
for f in branch main; do
  grep -E '^\| [a-z0-9_-]+ \| (PASS|FAIL|TIMEOUT|ERROR) \|$' /tmp/suite_$f.log | sort > /tmp/tbl_$f.txt
done
echo "rows: branch=$(wc -l < /tmp/tbl_branch.txt) main=$(wc -l < /tmp/tbl_main.txt)"
diff /tmp/tbl_main.txt /tmp/tbl_branch.txt && echo IDENTICAL
```

⚠️ **Use `git checkout main`, never `git stash`** — on a clean tree the stash is a no-op and the "baseline" is the branch. Confirm the differing `huck commit:` stamps in the two logs.
⚠️ **Print the row count next to any "identical" claim.** A wrong extraction reported a false IDENTICAL from two EMPTY sets during v356; the runner emits a Markdown table, not `PASS`-prefixed lines. Expect 82 rows a side.

The `errors`, `posix2` and `set-e` categories are the ones that could move. Movement in either direction needs explaining before merge.

- [ ] **Step 5: Docs, memory, blog**

Update `docs/architecture.md`: add `error_fatality.rs` to the module map, and record that fatality is decided in exactly one place with `raise_fatal`/`raise_discard` sealed — including the measured driver rule and the plain-syntax-error exception, so the next person does not "simplify" the 127 substitution into applying everywhere.

Write `site/content/blog/<slug>.mdx` per the blog-every-pass rule: user-visible symptom first, at least one REAL before/after pair built from a pre-v358 binary in a throwaway worktree, and validate with:

```bash
cd site && export NVM_DIR="$HOME/.nvm" && . "$NVM_DIR/nvm.sh" && nvm use default \
  && ( ulimit -v 12000000; npx velite --strict )
```

Record the iteration in `project_huck_iterations.md` + `MEMORY.md`.

- [ ] **Step 6: Open the PR**

```bash
git push -u origin v358-error-fatality
gh pr create --base main --title 'v358: the error-fatality classifier (#198)' --body '...Closes #198, #116, #25, #490...'
```

Wait for CI to FINISH and pass. **Do not merge** — a `vNN` iteration PR is the user's to review and merge.

---

## Self-Review

**Spec coverage.** Classifier module → Task 2. Enforcement by privacy → Task 7. Driver-dependent code + the `Syntax` exception → Tasks 1, 2. Migration table → Tasks 4, 5, 6. `posix_fatal` deleted → Task 4. The "not done" list (no rewrite of the ~360 emit-only sites, #68's message gaps out of scope, no fabricated builtin rule) → stated in Tasks 2, 6 and the constraints. Verification items 1-5 → Task 3 and Task 7.

**Placeholders.** None. The one genuinely open question — whether the backtick/`$( )` distinction is available at `shell.rs:557` — is written as an investigation step with an explicit instruction not to guess from source text, because the answer depends on code no measurement can settle.

**Type consistency.** `ErrorKind`, `Fatality`, `fatality(kind, shell)`, `Shell::report_error` are spelled identically in Tasks 2, 4, 5, 6, 7. The driver axis is the EXISTING `Shell::is_command_string` throughout — Task 1's `Invocation` was withdrawn once the field was found.
