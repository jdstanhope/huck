# huck v69 — `set -e`/`-u`/`-o` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship `set -e` (errexit), `set -u` (nounset), and the
`-o`/`+o` long-form syntax + `$-` special parameter. Finishes
the deferred half of M-08.

**Architecture:** New `ShellOptions { errexit: bool, nounset:
bool }` struct on `Shell`. v50's `builtin_set` flag-rejection
path replaced with real handling. Reuses v36's
`err_suppressed_depth` as the errexit gate (same suppression
contexts as the ERR trap). Bare `$VAR` expansion path in
`src/expand.rs` adds a nounset check. `$-` joins the special-
parameter switch in `Shell::lookup_var`.

**Tech Stack:** Rust. No new deps.

**Spec:** `docs/superpowers/specs/2026-06-01-huck-set-options-design.md`

**Branch:** `v69-set-options`.

**Commit trailer:**

```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Preamble: Create the working branch

- [ ] **Step P.1**

```bash
git checkout main
git pull --ff-only
git checkout -b v69-set-options
```

Spec + this plan are committed before Task 1.

---

## Task 1: ShellOptions + builtin_set extension + errexit + nounset + $- + 10 unit tests

**Files:**
- Modify `src/shell_state.rs` — add `ShellOptions` struct +
  `Shell.shell_options` field + `Default` init + `lookup_var`
  case for `"-"` + `dollar_dash_value` method.
- Modify `src/builtins.rs` — add `OptionInfo` registry,
  `option_get`/`option_set` helpers, `print_options_table` +
  `print_options_reinput`; rewrite `builtin_set`'s flag-handling
  to dispatch real options; `mod set_options_tests`.
- Modify `src/executor.rs` — add `maybe_errexit` helper + wire
  it into every callsite that currently fires the ERR trap (so
  errexit and ERR-trap share gates).
- Modify `src/expand.rs` — bare-`$VAR` expansion checks nounset
  before treating unset as empty.

### Step 1.1: Add `ShellOptions` struct + `Shell` field

In `src/shell_state.rs`, after the `Variable` struct (or near
other state types), add:

```rust
/// Persistent shell-option state controlled by `set -X` / `set
/// -o NAME`. Extend the struct AND the option-name table in
/// `src/builtins.rs` together when adding a new option.
#[derive(Debug, Clone, Default)]
pub struct ShellOptions {
    pub errexit: bool,
    pub nounset: bool,
}
```

In the `pub struct Shell` block, add the field near
`is_interactive`:

```rust
pub shell_options: ShellOptions,
```

In `Shell::new`, add the initializer:

```rust
shell_options: ShellOptions::default(),
```

- [ ] **Step 1.1**

### Step 1.2: Add `dollar_dash_value` method + `lookup_var` case

In `impl Shell { ... }`, after existing helpers:

```rust
/// Returns the value of `$-` — alphabetical concatenation of
/// short-flag letters reflecting current shell-options state
/// and the interactive flag.
pub fn dollar_dash_value(&self) -> String {
    let mut out = String::new();
    if self.shell_options.errexit { out.push('e'); }
    if self.is_interactive { out.push('i'); }
    if self.shell_options.nounset { out.push('u'); }
    out
}
```

In `lookup_var`, add a case to the special-parameter match (the
block that handles `"0"`, `"$"`, `"!"`):

```rust
"-" => return Some(self.dollar_dash_value()),
```

- [ ] **Step 1.2**

### Step 1.3: Build

`cargo build`. Expected: clean.

- [ ] **Step 1.3**

### Step 1.4: Add option-name registry + helpers

In `src/builtins.rs`, near other static tables (or just before
`builtin_set`):

```rust
struct OptionInfo {
    name: &'static str,
    #[allow(dead_code)]
    short: Option<char>,
}

const SHELL_OPTIONS: &[OptionInfo] = &[
    OptionInfo { name: "errexit", short: Some('e') },
    OptionInfo { name: "nounset", short: Some('u') },
];

fn option_get(shell: &Shell, name: &str) -> Option<bool> {
    match name {
        "errexit" => Some(shell.shell_options.errexit),
        "nounset" => Some(shell.shell_options.nounset),
        _ => None,
    }
}

fn option_set(shell: &mut Shell, name: &str, value: bool) -> Result<(), ()> {
    match name {
        "errexit" => { shell.shell_options.errexit = value; Ok(()) }
        "nounset" => { shell.shell_options.nounset = value; Ok(()) }
        _ => Err(()),
    }
}

fn print_options_table(out: &mut dyn std::io::Write, shell: &Shell) -> ExecOutcome {
    for opt in SHELL_OPTIONS {
        let val = option_get(shell, opt.name).unwrap_or(false);
        let _ = writeln!(out, "{:<16}{}", opt.name, if val { "on" } else { "off" });
    }
    ExecOutcome::Continue(0)
}

fn print_options_reinput(out: &mut dyn std::io::Write, shell: &Shell) -> ExecOutcome {
    for opt in SHELL_OPTIONS {
        let val = option_get(shell, opt.name).unwrap_or(false);
        let sign = if val { '-' } else { '+' };
        let _ = writeln!(out, "set {sign}o {}", opt.name);
    }
    ExecOutcome::Continue(0)
}
```

The `#[allow(dead_code)]` on `short` is because v69 doesn't yet
need to map short→long programmatically (the flag parser dispatches
directly). Future iterations will use it.

- [ ] **Step 1.4**

### Step 1.5: Rewrite `builtin_set`'s flag-handling

Find the existing v50 `builtin_set` (with the "not yet supported"
rejection arm). Replace its flag-parsing prefix with the new
handler from the spec §"`builtin_set` extension". Key changes:
- `-o NAME` calls `option_set(shell, name, true)`.
- `-o` (no NAME) calls `print_options_table`.
- `+o NAME` calls `option_set(shell, name, false)`.
- `+o` (no NAME) calls `print_options_reinput`.
- `-e`/`-u`/`+e`/`+u` flip the respective fields directly via
  the same `option_set` helper (or via direct field writes).
- Other short flags continue to reject with "not yet supported".
- `--` ends flag parsing.
- Non-flag arg (or after `--`) → positional-args replacement
  (existing v50 logic preserved).

Watch for the "bare set" (no args at all) case — that should still
list all variables (v50's M-65). Easiest: check `args.is_empty()`
first; if so, the v50 var-listing path runs.

- [ ] **Step 1.5**

### Step 1.6: Add `maybe_errexit` helper in `src/executor.rs`

Insert near other top-level helpers:

```rust
/// Called after a simple command's status is set. If errexit is
/// on, the status is non-zero, and we're not in an err-suppressed
/// context (matches v36's ERR-trap gate), returns the Exit
/// outcome to terminate the shell with that status. Caller
/// returns the outcome.
fn maybe_errexit(shell: &crate::shell_state::Shell, status: i32) -> Option<ExecOutcome> {
    if shell.shell_options.errexit
        && shell.err_suppressed_depth == 0
        && status != 0
    {
        Some(ExecOutcome::Exit(status))
    } else {
        None
    }
}
```

### Step 1.7: Wire `maybe_errexit` into command-completion paths

Grep for `fire_err_trap` callsites — those are the natural
places. After each one, add:

```rust
if let Some(out) = maybe_errexit(shell, status) {
    return out;
}
```

Likely sites (based on v36 memory):
- `run_command` SimpleCommand arms after the command returns.
- After each statement in Sequence/AndOr bodies.
- After function-body command completion.

Each site already has access to a `status` value (the last
command's exit code). Adapt the local variable name as needed.

- [ ] **Step 1.6**
- [ ] **Step 1.7**

### Step 1.8: Add nounset check in `src/expand.rs`

Find the bare-`$VAR` expansion path. Looking at `src/expand.rs`,
there's likely a function that handles plain variable
substitution (no modifier) — it currently calls
`shell.lookup_var(name)` and treats `None` as the empty string.

Change the unset branch to:

```rust
let value = match shell.lookup_var(name) {
    Some(v) => v,
    None => {
        if shell.shell_options.nounset {
            eprintln!("huck: {name}: unbound variable");
            shell.pending_fatal_pe_error = Some(1);
            return /* match existing fatal-PE return path */;
        }
        String::new()
    }
};
```

The exact return type depends on the function's signature.
Look at how `ExpansionResult::Fatal { status: 1 }` is signaled
in `src/param_expansion.rs::expand_modifier` (the
`AssignDefault` arm post-v54). Use the same mechanism here.

If there are MULTIPLE bare-`$VAR` paths (e.g., one for `$VAR`,
one for `${VAR}` without modifier), apply the check to all of
them. Modifier paths (`${VAR:-default}`, etc.) are NOT touched
— bash exempts them.

- [ ] **Step 1.8**

### Step 1.9: Build

`cargo build`. Expected: clean. If clippy complains about
`#[allow(dead_code)]` on `short`, leave as is.

- [ ] **Step 1.9**

### Step 1.10: Append `mod set_options_tests` (10 tests)

At end of `src/builtins.rs` (after the v67 `mod help_tests`):

```rust
#[cfg(test)]
mod set_options_tests {
    use super::*;
    use crate::shell_state::Shell;

    fn run(args: &[&str], shell: &mut Shell) -> (ExecOutcome, String) {
        let args_owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("set", &args_owned, &mut buf, shell);
        (outcome, String::from_utf8(buf).unwrap())
    }

    #[test]
    fn set_e_enables_errexit() {
        let mut shell = Shell::new();
        let (oc, _) = run(&["-e"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert!(shell.shell_options.errexit);
    }

    #[test]
    fn set_plus_e_disables() {
        let mut shell = Shell::new();
        shell.shell_options.errexit = true;
        let (oc, _) = run(&["+e"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert!(!shell.shell_options.errexit);
    }

    #[test]
    fn set_o_errexit_long_form() {
        let mut shell = Shell::new();
        let (oc, _) = run(&["-o", "errexit"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert!(shell.shell_options.errexit);
    }

    #[test]
    fn set_plus_o_errexit_disables() {
        let mut shell = Shell::new();
        shell.shell_options.errexit = true;
        let (oc, _) = run(&["+o", "errexit"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert!(!shell.shell_options.errexit);
    }

    #[test]
    fn set_dollar_dash_reflects_flags() {
        let mut shell = Shell::new();
        // No flags set, not interactive by default in tests.
        let dash = shell.lookup_var("-").unwrap_or_default();
        assert!(dash.is_empty() || dash == "i");
        // Enable errexit.
        run(&["-e"], &mut shell);
        let dash = shell.lookup_var("-").unwrap_or_default();
        assert!(dash.contains('e'));
        // Enable nounset.
        run(&["-u"], &mut shell);
        let dash = shell.lookup_var("-").unwrap_or_default();
        assert!(dash.contains('e'));
        assert!(dash.contains('u'));
    }

    #[test]
    fn set_invalid_o_name_errors() {
        let mut shell = Shell::new();
        let (oc, _) = run(&["-o", "nope_no_such_opt"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(2)));
    }

    #[test]
    fn set_o_listing_shows_state() {
        let mut shell = Shell::new();
        let (oc, out) = run(&["-o"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert!(out.lines().any(|l| l.starts_with("errexit")));
        assert!(out.lines().any(|l| l.starts_with("nounset")));
    }

    #[test]
    fn set_plus_o_listing_reinput_form() {
        let mut shell = Shell::new();
        let (oc, out) = run(&["+o"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        // Both off by default.
        assert!(out.lines().any(|l| l == "set +o errexit"));
        assert!(out.lines().any(|l| l == "set +o nounset"));
    }

    #[test]
    fn set_eu_cluster() {
        let mut shell = Shell::new();
        let (oc, _) = run(&["-eu"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert!(shell.shell_options.errexit);
        assert!(shell.shell_options.nounset);
    }

    #[test]
    fn set_dash_dash_resets_positional() {
        let mut shell = Shell::new();
        let (oc, _) = run(&["-e", "--", "a", "b", "c"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert!(shell.shell_options.errexit);
        assert_eq!(shell.positional_args, vec!["a".to_string(), "b".to_string(), "c".to_string()]);
    }
}
```

- [ ] **Step 1.10**

### Step 1.11: Run new tests

```bash
cargo test --bin huck set_options_tests
```

Expected: 10 pass.

- [ ] **Step 1.11**

### Step 1.12: Full unit suite

`cargo test --bin huck`. Expected: green. WATCH for regressions
in v50 `set`/`shift` tests and v36 ERR-trap tests — both
interact with the touched code.

- [ ] **Step 1.12**

### Step 1.13: Clippy

`cargo clippy --all-targets -- -D warnings`. Expected: clean.

- [ ] **Step 1.13**

### Step 1.14: Commit Task 1

```bash
git add src/shell_state.rs src/builtins.rs src/executor.rs src/expand.rs
git commit -m "$(cat <<'EOF'
set: -e / -u / -o long form + $- (v69 task 1)

Finish the deferred half of M-08: ship the \`set\` option flags
\`-e\` (errexit), \`-u\` (nounset), the \`-o\`/\`+o\` long-form
syntax, and the \`\$-\` special parameter.

Foundation (src/shell_state.rs):
- New \`ShellOptions { errexit: bool, nounset: bool }\` struct
  + \`Shell.shell_options\` field + Default init.
- New \`Shell::dollar_dash_value\` method returns the
  alphabetical concatenation of short-flag letters reflecting
  current option state (\`e\` for errexit, \`i\` for
  is_interactive, \`u\` for nounset). New case \`"-"\` in
  lookup_var routes to it.

builtin_set extension (src/builtins.rs):
- New OptionInfo registry + option_get / option_set / print_
  options_table / print_options_reinput helpers.
- Replaced v50's flag-rejection arm with real handling:
  -e/+e/-u/+u flip the corresponding fields; -o NAME / +o NAME
  go through option_set; -o (no name) and +o (no name) list
  options in human-readable and re-input forms respectively.
- Unknown -o / +o name → exit 2. Other short flags (-x, etc.)
  continue to reject with "not yet supported in this version"
  pending future iterations.

Errexit wire-in (src/executor.rs):
- New \`maybe_errexit(shell, status)\` helper returns
  Some(Exit(status)) when errexit is on, status != 0, and
  err_suppressed_depth == 0. Otherwise None.
- Wired in at every callsite that fires the v36 ERR trap —
  same gate, same contexts (if/elif/while/until conditions,
  &&/|| LHS, !-negated pipelines are exempt via the existing
  err_suppressed_depth tracking).

Nounset wire-in (src/expand.rs):
- Bare \`\$VAR\` and \`\${VAR}\` (no modifier) expansion paths
  check shell.shell_options.nounset before treating unset as
  empty. Set pending_fatal_pe_error = Some(1) and emit
  "huck: NAME: unbound variable" to stderr.
- Modifier paths (\`\${VAR:-default}\`, \`\${VAR:=default}\`,
  \`\${VAR:?msg}\`, \`\${VAR:+alt}\`, \`\${VAR#pat}\`, etc.)
  unchanged — they exempt unset per bash semantics.

10 unit tests in \`mod set_options_tests\`: -e enables; +e
disables; -o errexit long form; +o errexit disables;
\$- reflects flags; -o invalid name → exit 2; -o listing;
+o listing in re-input form; -eu cluster; -e -- resets
positional args.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

Stage exactly: `src/shell_state.rs src/builtins.rs src/executor.rs src/expand.rs`.

- [ ] **Step 1.14**

---

## Task 2: Integration tests

**Files:**
- Create `tests/set_options_integration.rs`.

9 binary-driven tests.

### Step 2.1: Create the file

```rust
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run_capture(script: &str) -> (String, String, i32) {
    let mut child = Command::new(huck_binary())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");
    child.stdin.as_mut().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn set_e_exits_on_failure() {
    let (out, _, rc) = run_capture("set -e\nfalse\necho X\nexit\n");
    assert_eq!(rc, 1, "expected rc=1; got {rc}; stdout: {out:?}");
    assert!(!out.lines().any(|l| l == "X"), "stdout should not have X: {out:?}");
}

#[test]
fn set_e_exempt_in_if() {
    let (out, _, _) = run_capture(
        "set -e\nif false; then :; fi\necho X\nexit\n",
    );
    assert!(out.lines().any(|l| l == "X"), "expected X in stdout: {out:?}");
}

#[test]
fn set_e_exempt_in_or_chain() {
    let (out, _, _) = run_capture(
        "set -e\nfalse || true\necho X\nexit\n",
    );
    assert!(out.lines().any(|l| l == "X"), "expected X in stdout: {out:?}");
}

#[test]
fn set_e_in_function_exits() {
    let (out, _, rc) = run_capture(
        "set -e\nf() { false; echo F; }\nf\necho M\nexit\n",
    );
    assert_eq!(rc, 1, "expected rc=1; got {rc}; stdout: {out:?}");
    assert!(!out.lines().any(|l| l == "F"));
    assert!(!out.lines().any(|l| l == "M"));
}

#[test]
fn set_u_unset_errors() {
    let (out, err, rc) = run_capture(
        "set -u\necho $XYZ_UNSET\necho X\nexit\n",
    );
    // In non-interactive mode, fatal PE error exits the shell.
    assert!(err.contains("unbound variable"), "stderr: {err:?}");
    assert!(!out.lines().any(|l| l == "X"), "stdout: {out:?}");
    assert_ne!(rc, 0, "expected non-zero rc; got {rc}");
}

#[test]
fn set_u_default_modifier_ok() {
    let (out, _, _) = run_capture(
        "set -u\necho \"${XYZ_UNSET:-default}\"\necho X\nexit\n",
    );
    assert!(out.lines().any(|l| l == "default"), "stdout: {out:?}");
    assert!(out.lines().any(|l| l == "X"), "stdout: {out:?}");
}

#[test]
fn set_o_errexit_works_as_dash_e() {
    let (out, _, rc) = run_capture(
        "set -o errexit\nfalse\necho X\nexit\n",
    );
    assert_eq!(rc, 1, "expected rc=1; got {rc}");
    assert!(!out.lines().any(|l| l == "X"));
}

#[test]
fn dollar_dash_includes_e_after_set_e() {
    let (out, _, _) = run_capture("set -e\necho \"[$-]\"\nexit\n");
    let line = out.lines().find(|l| l.starts_with("[") && l.ends_with("]"))
        .unwrap_or("");
    assert!(line.contains('e'), "$- should contain 'e'; got: {line:?}");
}

#[test]
fn set_minus_o_lists_options() {
    let (out, _, _) = run_capture("set -o\nexit\n");
    assert!(out.lines().any(|l| l.starts_with("errexit")), "stdout: {out:?}");
    assert!(out.lines().any(|l| l.starts_with("nounset")), "stdout: {out:?}");
}
```

- [ ] **Step 2.1**

### Step 2.2: Run integration tests

```bash
cargo test --test set_options_integration -- --nocapture
```

Expected: 9 pass.

- [ ] **Step 2.2**

### Step 2.3: Full suite + clippy

```bash
cargo test --tests
cargo clippy --all-targets -- -D warnings
```

- [ ] **Step 2.3**

### Step 2.4: Commit Task 2

```bash
git add tests/set_options_integration.rs
git commit -m "$(cat <<'EOF'
test: set options integration coverage (v69 task 2)

9 binary-driven tests for the set -e/-u/-o options:

- set_e_exits_on_failure — \`false\` after \`set -e\` exits rc=1.
- set_e_exempt_in_if — if-condition failure doesn't trigger.
- set_e_exempt_in_or_chain — \`false || true\` doesn't trigger.
- set_e_in_function_exits — failure inside a function exits.
- set_u_unset_errors — bare \$UNSET under -u → fatal PE error.
- set_u_default_modifier_ok — \${UNSET:-default} exempt from -u.
- set_o_errexit_works_as_dash_e — long-form mirrors -e.
- dollar_dash_includes_e_after_set_e — \$- contains 'e'.
- set_minus_o_lists_options — \`set -o\` lists errexit + nounset.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 2.4**

---

## Task 3: Docs

**Files:**
- Modify `docs/bash-divergences.md` — update M-08 from
  `[deferred]` to `[fixed v69 partial]`; add change-log entry.
- Modify `README.md` — v69 row.

### Step 3.1: Update M-08

Current (post-v68 cleanup):

```markdown
- **M-08: `set` option flags** — `[deferred]` medium. ...
```

Replace with:

```markdown
- **M-08: `set` option flags** — `[fixed v69 partial]` medium. `set -e` (errexit) and `set -u` (nounset) shipped via new `Shell.shell_options: ShellOptions` field + wire-ins at the v36 ERR-trap gate (errexit) and the bare-`$VAR` expansion path (nounset). Long-form `set -o NAME` / `set +o NAME` works for both; `set -o` (no args) lists current state; `set +o` lists in re-input form. New `$-` special parameter reflects current short-flag letters (alphabetical: `e`, `i`, `u`). Cluster forms (`set -eu`) supported. **Still deferred**: `-x` (xtrace), `pipefail`, `-n` (noexec), `-f` (noglob), `-a` (allexport), `-C` (noclobber), `-b` (notify), `-v` (verbose), `-h`, monitor. v69's `builtin_set` still rejects unimplemented short flags with "not yet supported in this version".
```

- [ ] **Step 3.1**

### Step 3.2: Add v69 change-log entry

In `## Change log` after v68:

```markdown
- **2026-06-01**: v69 finishes the deferred half of M-08. New `ShellOptions { errexit: bool, nounset: bool }` struct + `Shell.shell_options` field + Default init. `Shell::dollar_dash_value` builds `$-` from option flags + `is_interactive` (alphabetical: `e`/`i`/`u`); `lookup_var` routes `"-"` to it. `builtin_set` extension in `src/builtins.rs`: new OptionInfo registry + option_get/option_set/print_options_table/print_options_reinput helpers. Replaced v50's flag-rejection arm with real `-e`/`+e`/`-u`/`+u`/`-o NAME`/`+o NAME` handling; cluster forms (`-eu`); `-o`/`+o` with no name lists state; unknown names → exit 2. Other short flags continue to reject pending future iterations. **Errexit wire-in** (src/executor.rs): new `maybe_errexit(shell, status)` helper called after each command's status is set, at every site that fires v36's ERR trap. Same suppression contexts (if/elif/while/until conditions, &&/|| LHS, !-negated pipelines) via the existing `err_suppressed_depth`. **Nounset wire-in** (src/expand.rs): bare `$VAR`/`${VAR}` (no modifier) expansion paths check `shell.shell_options.nounset` before treating unset as empty; on unset+nounset, emit "huck: NAME: unbound variable" + set pending_fatal_pe_error. Modifier paths (`:-`, `:=`, `:?`, `:+`, `#`, `%`, etc.) exempt per bash. 10 unit tests + 9 binary-driven integration tests. Updates M-08 from [deferred] to [fixed v69 partial] — `-x`/pipefail/-n/-f/-a/-C/-b/-v/-h/monitor still deferred.
```

- [ ] **Step 3.2**

### Step 3.3: Add v69 row to README

After v68:

```markdown
| v69       | `set -e`/`-u`/`-o` long-form + `$-` (M-08 cont.)               |
```

Match v68 column padding.

- [ ] **Step 3.3**

### Step 3.4: Full suite + clippy

```bash
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
```

- [ ] **Step 3.4**

### Step 3.5: Commit Task 3

```bash
git add docs/bash-divergences.md README.md
git commit -m "$(cat <<'EOF'
docs: set -e/-u/-o shipped v69 (M-08 cont.)

M-08 updated from [deferred] to [fixed v69 partial]. The
positional-reset half was shipped in v50 (M-65); v69 ships the
option flags -e (errexit), -u (nounset), -o/+o long-form, and
the \$- special parameter.

Still deferred: -x (xtrace), pipefail, -n (noexec), -f
(noglob), -a (allexport), -C (noclobber), -b (notify), -v
(verbose), -h, monitor. Each is a future iteration.

Change log: 2026-06-01 v69 entry summarizing ShellOptions,
the builtin_set flag-handler rewrite, the errexit wire-in
sharing v36's ERR-trap gate, and the nounset wire-in in
expand.rs.

README: v69 row added.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 3.5**

---

## Final verification (controller)

1. `cargo test --all-targets`.
2. `cargo clippy --all-targets -- -D warnings`.
3. Branch is four commits ahead of `main`: docs preamble + 3
   task commits.
4. Dispatch a final cross-task reviewer (multi-touch surface —
   shell_state + builtins + executor + expand — worth the
   review pass).
5. Merge to `main` with `--no-ff`, push, delete branch, update
   memory.
