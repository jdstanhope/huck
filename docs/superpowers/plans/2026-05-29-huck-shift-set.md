# huck v50 — `shift` and `set --` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the POSIX special builtins `shift` (remove first N
positional params) and `set` (no-args lists vars; `set --` and
`set -- args` and bare `set args` replace positional; option flags
rejected for now).

**Architecture:** Single-file change in `src/builtins.rs`. Two new
builtins + one helper + dispatch arms + `BUILTIN_NAMES` and
`is_special_builtin` updates. Both builtins mutate
`Shell.positional_args: Vec<String>` directly.

**Tech Stack:** Rust. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-05-29-huck-shift-set-design.md`

**Branch:** `v50-shift-set` (created in preamble step P.1).

**Commit trailer convention**:

```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Preamble: Create the working branch

- [ ] **Step P.1: Create branch from main and check it out**

```bash
git checkout main
git pull --ff-only
git checkout -b v50-shift-set
```

Expected: `Switched to a new branch 'v50-shift-set'`.

The spec + this plan are committed as the first commit on this branch
(handled by the controller before Task 1 begins).

---

## Task 1: Builtins + 13 unit tests

**Files:**
- Modify: `src/builtins.rs` — add `builtin_shift`, `builtin_set`,
  `set_escape_value`; dispatch arms; `BUILTIN_NAMES` extension;
  `is_special_builtin` extension; doc-comment trim; append
  `mod shift_tests` (7 tests) and `mod set_tests` (6 tests).

### Step 1.1: Add `"set"` and `"shift"` to `BUILTIN_NAMES`

In `src/builtins.rs:18-22`, replace:

```rust
pub const BUILTIN_NAMES: &[&str] = &[
    "cd", "exit", "pwd", "echo", "export", "unset", "jobs",
    "wait", "fg", "bg", "kill", "disown", "history", "test", "[",
    "break", "continue", "return", "trap", "alias", "unalias",
];
```

With:

```rust
pub const BUILTIN_NAMES: &[&str] = &[
    "cd", "exit", "pwd", "echo", "export", "unset", "jobs",
    "wait", "fg", "bg", "kill", "disown", "history", "test", "[",
    "break", "continue", "return", "trap", "alias", "unalias",
    "set", "shift",
];
```

- [ ] **Step 1.1: Add to BUILTIN_NAMES**

### Step 1.2: Trim doc comment + extend `is_special_builtin`

In `src/builtins.rs` (around lines 28-35), the current doc comment and function read:

```rust
/// True for POSIX "special builtins" (2.14). Inline assignments preceding a
/// special builtin persist in the shell; assignments preceding a regular
/// builtin or external command are scoped to the command. The set is huck's
/// existing builtins intersected with the POSIX special list; expand here as
/// huck adds `set`/`shift`/`trap`/`eval`/`exec`/`:`/`readonly`/`.`.
pub fn is_special_builtin(name: &str) -> bool {
    matches!(name, "break" | "continue" | "exit" | "export" | "return" | "trap" | "unset")
}
```

Replace with:

```rust
/// True for POSIX "special builtins" (2.14). Inline assignments preceding a
/// special builtin persist in the shell; assignments preceding a regular
/// builtin or external command are scoped to the command. The set is huck's
/// existing builtins intersected with the POSIX special list; expand here as
/// huck adds `eval`/`exec`/`:`/`readonly`/`.`.
pub fn is_special_builtin(name: &str) -> bool {
    matches!(name,
        "break" | "continue" | "exit" | "export" | "return"
        | "set" | "shift" | "trap" | "unset"
    )
}
```

- [ ] **Step 1.2: Update `is_special_builtin`**

### Step 1.3: Add dispatch arms in `run_builtin`

In `src/builtins.rs`, find the `match name { ... }` block inside `run_builtin` (around lines 46-61). The current arms end with something like:

```rust
        "trap" => builtin_trap(args, out, shell),
        "test" | "[" => builtin_test(name, args),
        "break" => ExecOutcome::LoopBreak,
        "continue" => ExecOutcome::LoopContinue,
        "return" => { ... }
        // ...
```

Add the new arms — natural position right after `"trap"`:

```rust
        "trap" => builtin_trap(args, out, shell),
        "set" => builtin_set(args, out, shell),
        "shift" => builtin_shift(args, shell),
        "test" | "[" => builtin_test(name, args),
        // ... rest unchanged
```

- [ ] **Step 1.3: Add dispatch arms**

### Step 1.4: Add `builtin_shift` + `builtin_set` + `set_escape_value`

In `src/builtins.rs`, find a natural insertion point — after `builtin_trap` and its helpers (around line 1300+, but search for `fn builtin_trap` to locate). After the trap helpers (e.g. after `print_signal_table` and `signal_number_to_name`, before `fn builtin_test` or `fn builtin_alias`), insert:

```rust
fn builtin_shift(args: &[String], shell: &mut Shell) -> ExecOutcome {
    let n: usize = match args.first() {
        None => 1,
        Some(s) => match s.parse::<usize>() {
            Ok(n) => n,
            Err(_) => {
                eprintln!("huck: shift: {s}: numeric argument required");
                return ExecOutcome::Continue(1);
            }
        },
    };
    if n > shell.positional_args.len() {
        eprintln!("huck: shift: shift count out of range");
        return ExecOutcome::Continue(1);
    }
    shell.positional_args.drain(0..n);
    ExecOutcome::Continue(0)
}

fn builtin_set(args: &[String], out: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    if args.is_empty() {
        let mut names: Vec<String> = shell.var_names().map(|s| s.to_string()).collect();
        names.sort();
        for name in &names {
            if let Some(v) = shell.lookup_var(name) {
                let _ = writeln!(out, "{}={}", name, set_escape_value(&v));
            }
        }
        return ExecOutcome::Continue(0);
    }

    let first = &args[0];
    if first == "--" {
        shell.positional_args = args[1..].to_vec();
        return ExecOutcome::Continue(0);
    }
    if (first.starts_with('-') || first.starts_with('+')) && first.len() > 1 {
        eprintln!("huck: set: {first}: options not yet supported in this version");
        return ExecOutcome::Continue(2);
    }
    // No leading -- or option flag — replace positional with all args.
    shell.positional_args = args.to_vec();
    ExecOutcome::Continue(0)
}

fn set_escape_value(v: &str) -> String {
    format!("'{}'", v.replace('\'', r#"'\''"#))
}
```

- [ ] **Step 1.4: Add the builtins + helper**

### Step 1.5: Build

Run: `cargo build`
Expected: clean.

- [ ] **Step 1.5: Build clean**

### Step 1.6: Append `mod shift_tests` with 7 tests

At the end of `src/builtins.rs` (after the last existing `mod` block), append:

```rust
#[cfg(test)]
mod shift_tests {
    use super::*;
    use crate::shell_state::Shell;

    #[test]
    fn shift_no_args_removes_first() {
        let mut shell = Shell::new();
        shell.positional_args = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("shift", &[], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.positional_args, vec!["b", "c"]);
    }

    #[test]
    fn shift_n_removes_n() {
        let mut shell = Shell::new();
        shell.positional_args = vec![
            "a".to_string(), "b".to_string(), "c".to_string(), "d".to_string(),
        ];
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("shift", &["2".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.positional_args, vec!["c", "d"]);
    }

    #[test]
    fn shift_default_when_no_args_equals_one() {
        let mut shell_a = Shell::new();
        shell_a.positional_args = vec!["x".to_string(), "y".to_string()];
        let mut shell_b = Shell::new();
        shell_b.positional_args = vec!["x".to_string(), "y".to_string()];

        let mut buf: Vec<u8> = Vec::new();
        let _ = run_builtin("shift", &[], &mut buf, &mut shell_a);
        let _ = run_builtin("shift", &["1".to_string()], &mut buf, &mut shell_b);

        assert_eq!(shell_a.positional_args, shell_b.positional_args);
        assert_eq!(shell_a.positional_args, vec!["y"]);
    }

    #[test]
    fn shift_too_large_errors_status_1() {
        let mut shell = Shell::new();
        shell.positional_args = vec!["a".to_string()];
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("shift", &["5".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
        // Positional unchanged after error.
        assert_eq!(shell.positional_args, vec!["a"]);
    }

    #[test]
    fn shift_zero_is_noop() {
        let mut shell = Shell::new();
        shell.positional_args = vec!["a".to_string(), "b".to_string()];
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("shift", &["0".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.positional_args, vec!["a", "b"]);
    }

    #[test]
    fn shift_non_numeric_errors_status_1() {
        let mut shell = Shell::new();
        shell.positional_args = vec!["a".to_string()];
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("shift", &["abc".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
        assert_eq!(shell.positional_args, vec!["a"]);
    }

    #[test]
    fn shift_negative_errors_status_1() {
        let mut shell = Shell::new();
        shell.positional_args = vec!["a".to_string(), "b".to_string()];
        let mut buf: Vec<u8> = Vec::new();
        // `-1` fails parse::<usize>() because usize can't be negative.
        let outcome = run_builtin("shift", &["-1".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
        assert_eq!(shell.positional_args, vec!["a", "b"]);
    }
}
```

- [ ] **Step 1.6: Append shift_tests**

### Step 1.7: Append `mod set_tests` with 6 tests

After `mod shift_tests` at the end of `src/builtins.rs`, append:

```rust
#[cfg(test)]
mod set_tests {
    use super::*;
    use crate::shell_state::Shell;

    #[test]
    fn set_no_args_lists_sorted_vars() {
        let mut shell = Shell::new();
        // Use unique names unlikely to collide with environment.
        shell.set("ZZTEST_C".to_string(), "three".to_string());
        shell.set("ZZTEST_A".to_string(), "one".to_string());
        shell.set("ZZTEST_B".to_string(), "two".to_string());
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("set", &[], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        // Find the three target lines and confirm they appear in
        // sorted order relative to each other.
        let a_idx = out.find("ZZTEST_A=").expect("missing A");
        let b_idx = out.find("ZZTEST_B=").expect("missing B");
        let c_idx = out.find("ZZTEST_C=").expect("missing C");
        assert!(a_idx < b_idx, "A should come before B");
        assert!(b_idx < c_idx, "B should come before C");
        // Format check: value should be single-quoted.
        assert!(out.contains("ZZTEST_A='one'"), "expected single-quoted value: {out:?}");
    }

    #[test]
    fn set_double_dash_alone_clears_positional() {
        let mut shell = Shell::new();
        shell.positional_args = vec!["a".to_string(), "b".to_string()];
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("set", &["--".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert!(shell.positional_args.is_empty());
    }

    #[test]
    fn set_double_dash_with_args_replaces() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "set",
            &["--".to_string(), "one".to_string(), "two".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.positional_args, vec!["one", "two"]);
    }

    #[test]
    fn set_bare_args_replaces_positional() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "set",
            &["one".to_string(), "two".to_string(), "three".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        assert_eq!(shell.positional_args, vec!["one", "two", "three"]);
    }

    #[test]
    fn set_dash_e_rejects_with_status_2() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("set", &["-e".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn set_plus_x_rejects_with_status_2() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("set", &["+x".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }
}
```

Note: the `set_no_args_lists_sorted_vars` test calls `shell.set(name, value)`. The actual signature in `src/shell_state.rs:157` is `pub fn set(&mut self, name: &str, value: String)`. If `String::to_string` doesn't coerce to `&str` automatically in the call site, the implementer should adjust to `shell.set("ZZTEST_C", "three".to_string())` directly.

- [ ] **Step 1.7: Append set_tests**

### Step 1.8: Run the new test mods

Run: `cargo test --bin huck shift_tests:: set_tests:: -- --nocapture`
Expected: 13 tests pass.

If `set_no_args_lists_sorted_vars` fails because exported environment variables also appear in the listing and they don't sort alphabetically: `Shell.var_names()` likely returns all variables including pre-seeded ones from the process environment. The fix is the assertion already uses substring positions (`a_idx < b_idx < c_idx`), so as long as the three `ZZTEST_*` lines appear in that relative order, the test passes regardless of other env vars. Confirm by inspecting actual output if it fails.

- [ ] **Step 1.8: 13 tests pass**

### Step 1.9: Run full unit suite

Run: `cargo test --bin huck`
Expected: all unit tests pass.

- [ ] **Step 1.9: Full unit suite passes**

### Step 1.10: Clippy

Run: `cargo clippy --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 1.10: Clippy clean**

### Step 1.11: Commit

```bash
git add src/builtins.rs
git commit -m "$(cat <<'EOF'
builtin: shift + set -- (v50 task 1)

Add the POSIX special builtins `shift` and `set`.

`shift [N]` removes the first N positional parameters (N defaults
to 1). N must parse as a non-negative integer (negative or
non-numeric → "numeric argument required" + status 1). N > current
count → "shift count out of range" + status 1.

`set` supports the subset needed for positional manipulation:
- No args: list all shell variables in sorted `name='value'` form.
- `set --`: clear positional.
- `set -- args`: replace positional.
- `set args` (no leading `--`): bash-faithful — also replaces
  positional, as long as the first arg doesn't look like an option
  flag.
- `set -e`/`set -x`/`set +o`/etc. (option flags): explicitly
  rejected with `set: <flag>: options not yet supported in this
  version` + status 2. These are deferred to a future iteration.

Both added to BUILTIN_NAMES and run_builtin dispatch. Both join
is_special_builtin's matched set (POSIX classifies them as
special; inline assignments preceding them persist in the shell).
The doc comment on is_special_builtin is trimmed to drop set/shift
from the "future additions" list.

13 new unit tests: 7 in shift_tests (no-args/N/default-equals-1/
too-large/zero-is-noop/non-numeric/negative) + 6 in set_tests
(sorted listing/double-dash-alone/double-dash-with-args/bare-args/
-e-rejected/+x-rejected).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 1.11: Commit Task 1**

---

## Task 2: Integration tests

**Files:**
- Create: `tests/shift_set_integration.rs`

Two binary-driven tests.

### Step 2.1: Create the integration test file

Create `tests/shift_set_integration.rs` with this exact content:

```rust
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run_capture(script: &str) -> (String, String) {
    let mut child = Command::new(huck_binary())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn shift_advances_positional_in_function() {
    // Define a function that shifts and echoes the new $1.
    // Call with `a b c` → after shift, $1 should be `b`.
    let script = "f() { shift; echo $1; }\nf a b c\nexit\n";
    let (out, _) = run_capture(script);
    assert!(
        out.lines().any(|l| l == "b"),
        "expected line `b` (the shifted $1) in: {:?}",
        out
    );
}

#[test]
fn set_then_for_loop_positional() {
    // `set --` followed by a for loop over "$@".
    let script = "set -- one two three\nfor arg in \"$@\"; do echo $arg; done\nexit\n";
    let (out, _) = run_capture(script);
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines.contains(&"one"), "missing `one` in: {:?}", out);
    assert!(lines.contains(&"two"), "missing `two` in: {:?}", out);
    assert!(lines.contains(&"three"), "missing `three` in: {:?}", out);
}
```

- [ ] **Step 2.1: Create the file**

### Step 2.2: Run the integration suite

Run: `cargo test --test shift_set_integration -- --nocapture`
Expected: both tests pass.

If `shift_advances_positional_in_function` fails: verify that huck's function call mechanism preserves the new positional state across the `shift` and `echo` lines inside the function body. Per `src/executor.rs:1385-1401`, `call_function` saves and restores positional around the call, but `shift` inside the function should mutate the active (in-function) `Shell.positional_args`. If the test fails, the most likely culprit is that the in-function shift happens but the echo reads from a stale snapshot — investigate Task 1 plumbing.

If `set_then_for_loop_positional` fails: `set --` at top level should populate `Shell.positional_args`. The `for arg in "$@"` should iterate them. If the iteration produces nothing: check that `"$@"` expands correctly post-set (it should, since v22-era code already reads `shell.positional_args` for `$@`).

Do NOT relax assertions — fix Task 1 or report BLOCKED.

- [ ] **Step 2.2: Tests pass**

### Step 2.3: Full integration suite

Run: `cargo test --tests`
Expected: all integration tests pass. PTY flake `pty_compound_stage_pipeline_stops_and_resumes` may flake; re-run in isolation if hit.

- [ ] **Step 2.3: Full integration suite green**

### Step 2.4: Clippy

Run: `cargo clippy --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 2.4: Clippy clean**

### Step 2.5: Commit

```bash
git add tests/shift_set_integration.rs
git commit -m "$(cat <<'EOF'
test: shift + set -- integration coverage (v50 task 2)

Two binary-driven tests verifying that the new positional-param
builtins work end-to-end through the huck binary.
shift_advances_positional_in_function defines a function that
shifts and echoes the new $1, calls it with `a b c`, and asserts
stdout contains `b`. set_then_for_loop_positional uses `set --
one two three` followed by `for arg in "$@"; do echo $arg; done`
and asserts all three lines appear.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 2.5: Commit Task 2**

---

## Task 3: Docs

**Files:**
- Modify: `docs/bash-divergences.md` — add new M-65 entry,
  change-log entry.
- Modify: `README.md` — v50 row.

### Step 3.1: Add M-65 entry in `docs/bash-divergences.md`

Neither `shift` nor `set --` currently has a tracked M-* entry. We
add M-65 as a NEW entry. M-64 was the originally-planned (later
consolidated) number for v49; the next free number is M-65.

Find an appropriate Tier 2 section. "Builtins" or "Special
builtins" is the natural home — search the file for `### Builtins`
or similar. If none of the existing subsections fits, add a new
`### Special builtins` subsection.

Add this entry:

```markdown
- **M-65: `shift` and `set --`** — `[fixed v50]` medium. `shift [N]` removes the first N positional parameters (N defaults to 1; negative or non-numeric → status 1; N > count → status 1). `set` with no args lists all shell variables in sorted `name='value'` form; `set --`, `set -- args`, and bare `set args` (no leading dash) all replace the current positional parameters. `set -e`/`set -x`/`set -u`/`set +o`/etc. (option flags) are explicitly rejected with status 2 and a clear "not yet supported in this version" message — these are a future iteration. Both join `is_special_builtin`'s set (POSIX classifies them special; inline assignments preceding them persist in the shell).
```

- [ ] **Step 3.1: Add M-65 entry**

### Step 3.2: Add v50 change-log entry

In `docs/bash-divergences.md`, find `## Change log` and the most recent `**2026-05-29**` entry (v49, M-52). Add IMMEDIATELY after it:

```markdown
- **2026-05-29**: M-65 (`shift` + `set --`) shipped as v50. Single-file change in `src/builtins.rs`. `builtin_shift` mutates `Shell.positional_args` via `drain(0..n)` with bounds + numeric validation. `builtin_set` lists vars (no args) or replaces positional via `args[1..]` after `--` or via `args[..]` for the bare form. Option flags rejected with status 2. Both added to `BUILTIN_NAMES`, `run_builtin` dispatch, and `is_special_builtin`'s matched set. The `is_special_builtin` doc comment is trimmed to drop set/shift from the future-additions list. No new L-* divergences.
```

- [ ] **Step 3.2: Add change-log entry**

### Step 3.3: Add v50 row to README

In `README.md`, find the version table. After the v49 row (search for `| v49       |`), add IMMEDIATELY after it:

```markdown
| v50       | `shift` + `set --` (M-65)                                      |
```

Match column padding to v48/v49 (count actual trailing spaces in the file so the closing `|` lines up visually).

- [ ] **Step 3.3: Add README v50 row**

### Step 3.4: Full suite

Run: `cargo test --all-targets`
Expected: all tests pass (modulo PTY flake).

- [ ] **Step 3.4: Full suite green**

### Step 3.5: Clippy

Run: `cargo clippy --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 3.5: Clippy clean**

### Step 3.6: Commit

```bash
git add docs/bash-divergences.md README.md
git commit -m "$(cat <<'EOF'
docs: add M-65 (shift + set --) fixed v50

New M-65 entry in docs/bash-divergences.md tracks the two new
POSIX special builtins as [fixed v50]. Covers `shift [N]` semantics
(default 1, bounds + numeric validation, error status 1) and
`set`'s supported subset (no-args lists vars sorted; `set --`,
`set -- args`, bare `set args` replace positional; option flags
explicitly rejected with status 2).

Change log: 2026-05-29 v50 entry summarizing the single-file
change in src/builtins.rs.

README: v50 row added to the version table.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 3.6: Commit Task 3**

---

## Final verification (controller, not a task)

After the three task commits land:

1. Run `cargo test --all-targets` once more.
2. Run `cargo clippy --all-targets -- -D warnings`.
3. Confirm the branch has exactly four commits ahead of `main`:
   docs preamble (spec + plan), task 1, task 2, task 3.
4. Dispatch a final cross-task code-reviewer subagent over the
   full diff (`main..v50-shift-set`).
5. Merge to `main` with `--no-ff`, push, delete the branch, update
   the `huck iterations` memory with v50.
