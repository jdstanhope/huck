# huck v64 — `declare` / `typeset` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship bash's `declare`/`typeset` builtin (Tier A:
wires to existing huck primitives — v54 readonly, v52 local
scopes, existing export machinery).

**Architecture:** One new builtin `builtin_declare`, three new
helpers (`snapshot_for_local_scope`, `format_declare_line`,
`escape_double_quote_value`), two listing helpers
(`declare_list_all_vars`, `declare_list_functions`), one new
Shell method (`unexport`), and possibly one (`iter_vars` if the
private `vars` field isn't accessible enough). Both `declare`
and `typeset` dispatch to the same builtin.

**Tech Stack:** Rust. No new deps.

**Spec:** `docs/superpowers/specs/2026-05-31-huck-declare-design.md`

**Branch:** `v64-declare`.

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
git checkout -b v64-declare
```

Spec + this plan are committed before Task 1.

---

## Task 1: Foundation + builtin + 14 unit tests

**Files:**
- Modify `src/shell_state.rs` — add `unexport`; add `iter_vars`
  if `vars` is private.
- Modify `src/builtins.rs` — helpers + builtin + dispatch +
  `BUILTIN_NAMES` + `mod declare_tests`.

### Step 1.1: Add `Shell::unexport`

In `src/shell_state.rs`, add after `export_set` (around line
210):

```rust
/// Flips the `exported` flag off on an existing variable. No-op
/// if the variable doesn't exist. Used by `declare +x NAME`.
pub fn unexport(&mut self, name: &str) {
    if let Some(v) = self.vars.get_mut(name) {
        v.exported = false;
    }
}
```

- [ ] **Step 1.1**

### Step 1.2: Ensure `iter_vars` access exists

Check `Shell` struct: `vars` was declared as
`vars: HashMap<...>` (private, per v52 memory). Add a public
iterator method if not already present:

```rust
/// Iterator over all variable entries (name, Variable).
pub fn iter_vars(&self) -> impl Iterator<Item = (&String, &Variable)> {
    self.vars.iter()
}
```

If `vars` is already `pub(crate)` or the helper exists under
another name, skip this step.

- [ ] **Step 1.2**

### Step 1.3: Build

`cargo build`. Expected: clean.

- [ ] **Step 1.3**

### Step 1.4: Append `"declare"` and `"typeset"` to `BUILTIN_NAMES`

Current (post-v63), `src/builtins.rs:18-27` ends with
`"pushd", "popd", "dirs"`. Append both:

```rust
"declare", "typeset",
```

DO NOT add to `is_special_builtin`. Both are bash-specific
regular builtins.

- [ ] **Step 1.4**

### Step 1.5: Add pure helpers

Insert near other formatting helpers (or before `builtin_declare`):

```rust
fn escape_double_quote_value(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' | '\\' | '$' | '`' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

fn format_declare_line(name: &str, var: &crate::shell_state::Variable) -> String {
    let mut attrs = String::new();
    if var.readonly { attrs.push('r'); }
    if var.exported { attrs.push('x'); }
    let flag_str = if attrs.is_empty() {
        "--".to_string()
    } else {
        format!("-{attrs}")
    };
    let escaped = escape_double_quote_value(&var.value);
    format!("declare {flag_str} {name}=\"{escaped}\"")
}

fn snapshot_for_local_scope(shell: &mut Shell, name: &str) {
    if shell.local_scopes.is_empty() {
        return;
    }
    let already_saved = shell
        .local_scopes
        .last()
        .map(|f| f.contains_key(name))
        .unwrap_or(false);
    if already_saved {
        return;
    }
    let snap = shell.snapshot_var(name);
    shell
        .local_scopes
        .last_mut()
        .unwrap()
        .insert(name.to_string(), snap);
}
```

- [ ] **Step 1.5**

### Step 1.6: Add listing helpers

```rust
fn declare_list_all_vars(
    out: &mut dyn std::io::Write,
    shell: &Shell,
) -> ExecOutcome {
    let mut entries: Vec<(&String, &crate::shell_state::Variable)> =
        shell.iter_vars().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));
    for (name, var) in entries {
        let _ = writeln!(out, "{}", format_declare_line(name, var));
    }
    ExecOutcome::Continue(0)
}

fn declare_list_functions(
    names: &[String],
    _names_only: bool,
    out: &mut dyn std::io::Write,
    shell: &mut Shell,
) -> ExecOutcome {
    if names.is_empty() {
        let mut fnames: Vec<&String> = shell.functions.keys().collect();
        fnames.sort();
        for n in fnames {
            let _ = writeln!(out, "declare -f {n}");
        }
        return ExecOutcome::Continue(0);
    }
    let mut exit: i32 = 0;
    for name in names {
        if shell.functions.contains_key(name) {
            let _ = writeln!(out, "declare -f {name}");
        } else {
            eprintln!("huck: declare: {name}: not found");
            exit = 1;
        }
    }
    ExecOutcome::Continue(exit)
}
```

- [ ] **Step 1.6**

### Step 1.7: Add `builtin_declare`

Full code is in spec §"`builtin_declare`". Key paths:
- Flag-cluster parsing handles `-`-prefix AND `+`-prefix args.
  Each char dispatched individually. `+r` errors;
  `+x` sets `want_remove_export`; `-X` (where X is a deferred flag like `-i`) errors with "not yet implemented".
- `function_mode` (set by `-f`/`-F`) routes to
  `declare_list_functions`.
- `print_mode` (`-p`) with names: print each via
  `format_declare_line`; missing → exit 1.
- Empty names + non-function-mode → list everything via
  `declare_list_all_vars`.
- Per-name with mutations:
  - Snapshot for function-scope unwind BEFORE any mutation.
  - `-r` with value: check `is_readonly`, then set + `mark_readonly`. (Already-readonly + value → error.)
  - `-x` with value: check `is_readonly`; if `-r` was also set, just `export` after the set; else `export_set`.
  - `-x` without value: `export`.
  - `+x`: `unexport`.
  - Plain `declare NAME=val`: `try_set`. On Err: "readonly variable" + exit 1.
  - Plain `declare NAME` (no value): snapshot was already taken; no-op otherwise.

- [ ] **Step 1.7**

### Step 1.8: Add dispatch arm

In `run_builtin`'s match block:

```rust
"declare" | "typeset" => builtin_declare(args, out, shell),
```

Position near other variable-related builtins (export, unset,
readonly, local).

- [ ] **Step 1.8**

### Step 1.9: Build

`cargo build`. Expected: clean.

- [ ] **Step 1.9**

### Step 1.10: Append `mod declare_tests` (14 tests)

At end of `src/builtins.rs`:

```rust
#[cfg(test)]
mod declare_tests {
    use super::*;
    use crate::shell_state::Shell;

    fn run(args: &[&str], shell: &mut Shell) -> (ExecOutcome, String) {
        let args_owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("declare", &args_owned, &mut buf, shell);
        (outcome, String::from_utf8(buf).unwrap())
    }

    fn run_typeset(args: &[&str], shell: &mut Shell) -> ExecOutcome {
        let args_owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let mut buf: Vec<u8> = Vec::new();
        run_builtin("typeset", &args_owned, &mut buf, shell)
    }

    #[test]
    fn declare_bare_sets_var() {
        let mut shell = Shell::new();
        let (oc, _) = run(&["X_DECL=hi"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert_eq!(shell.lookup_var("X_DECL").as_deref(), Some("hi"));
    }

    #[test]
    fn declare_r_sets_and_locks() {
        let mut shell = Shell::new();
        let (oc, _) = run(&["-r", "X_DECL_R=hi"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert_eq!(shell.lookup_var("X_DECL_R").as_deref(), Some("hi"));
        assert!(shell.is_readonly("X_DECL_R"));
    }

    #[test]
    fn declare_x_sets_and_exports() {
        let mut shell = Shell::new();
        let (oc, _) = run(&["-x", "X_DECL_X=hi"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert_eq!(shell.lookup_var("X_DECL_X").as_deref(), Some("hi"));
        assert!(shell.is_exported("X_DECL_X"));
    }

    #[test]
    fn declare_rx_combines() {
        let mut shell = Shell::new();
        let (oc, _) = run(&["-rx", "X_DECL_RX=hi"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert!(shell.is_readonly("X_DECL_RX"));
        assert!(shell.is_exported("X_DECL_RX"));
    }

    #[test]
    fn declare_plus_x_unexports() {
        let mut shell = Shell::new();
        shell.export_set("X_DECL_UNEX", "v".to_string());
        assert!(shell.is_exported("X_DECL_UNEX"));
        let (oc, _) = run(&["+x", "X_DECL_UNEX"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert_eq!(shell.lookup_var("X_DECL_UNEX").as_deref(), Some("v"));
        assert!(!shell.is_exported("X_DECL_UNEX"));
    }

    #[test]
    fn declare_plus_r_errors() {
        let mut shell = Shell::new();
        let (oc, _) = run(&["+r", "X_FOO"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(1)));
    }

    #[test]
    fn declare_p_prints_known_var() {
        let mut shell = Shell::new();
        shell.set("X_DECL_P", "hi".to_string());
        let (oc, out) = run(&["-p", "X_DECL_P"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert_eq!(out, "declare -- X_DECL_P=\"hi\"\n");
    }

    #[test]
    fn declare_p_missing_errors() {
        let mut shell = Shell::new();
        let (oc, _) = run(&["-p", "X_DECL_MISSING"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(1)));
    }

    #[test]
    fn declare_f_lists_functions() {
        let mut shell = Shell::new();
        let body = Box::new(crate::command::Command::Simple(
            crate::command::SimpleCommand::Assign(vec![]),
        ));
        shell.functions.insert("fn1".to_string(), body.clone());
        shell.functions.insert("fn2".to_string(), body);
        let (oc, out) = run(&["-f"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        // Sorted; both present.
        assert!(out.contains("declare -f fn1"));
        assert!(out.contains("declare -f fn2"));
        assert!(
            out.find("fn1").unwrap() < out.find("fn2").unwrap(),
            "expected sorted; got {out:?}",
        );
    }

    #[test]
    fn declare_F_named_function_found() {
        let mut shell = Shell::new();
        let body = Box::new(crate::command::Command::Simple(
            crate::command::SimpleCommand::Assign(vec![]),
        ));
        shell.functions.insert("fn1".to_string(), body);
        let (oc, out) = run(&["-F", "fn1"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert_eq!(out, "declare -f fn1\n");
    }

    #[test]
    fn declare_F_named_function_missing() {
        let mut shell = Shell::new();
        let (oc, _) = run(&["-F", "fn_none"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(1)));
    }

    #[test]
    fn declare_invalid_identifier() {
        let mut shell = Shell::new();
        let (oc, _) = run(&["1foo=bar"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(1)));
        assert!(shell.lookup_var("1foo").is_none());
    }

    #[test]
    fn declare_typeset_alias() {
        let mut shell = Shell::new();
        let oc = run_typeset(&["-r", "X_TS=hi"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(0)));
        assert_eq!(shell.lookup_var("X_TS").as_deref(), Some("hi"));
        assert!(shell.is_readonly("X_TS"));
    }

    #[test]
    fn declare_deferred_flag_errors() {
        let mut shell = Shell::new();
        let (oc, _) = run(&["-i", "X=5"], &mut shell);
        assert!(matches!(oc, ExecOutcome::Continue(1)));
    }
}
```

Note: `shell.is_exported(name)` may not exist. If not, add it as a small helper next to `is_readonly`:

```rust
pub fn is_exported(&self, name: &str) -> bool {
    self.vars.get(name).map(|v| v.exported).unwrap_or(false)
}
```

(Add this in Step 1.1's same edit if it's missing.)

For `declare_f_lists_functions`: the test clones the same Command body for both functions. If `Box<Command>` isn't `Clone`, restructure to construct two separate Box values.

- [ ] **Step 1.10**

### Step 1.11: Run new tests

```bash
cargo test --bin huck declare_tests
```

Expected: 14 pass.

- [ ] **Step 1.11**

### Step 1.12: Full unit suite

`cargo test --bin huck`. Expected: green.

- [ ] **Step 1.12**

### Step 1.13: Clippy

`cargo clippy --all-targets -- -D warnings`. Expected: clean.

- [ ] **Step 1.13**

### Step 1.14: Commit Task 1

```bash
git add src/shell_state.rs src/builtins.rs
git commit -m "$(cat <<'EOF'
builtin: declare/typeset (v64 task 1)

Add bash's \`declare\` builtin (with \`typeset\` as alias) — Tier A:
wires to huck's existing variable infrastructure (v54 readonly,
v52 local_scopes, existing export machinery). No new Variable
attributes added; the deferred -i/-l/-u/-a/-A/-n/-g flags
report "not yet implemented" with exit 1.

Flags supported:
- -r: mark readonly (via v54 mark_readonly + try_set/set).
- -x: mark exported (via export / export_set).
- +x: un-export (new Shell::unexport method just flips the flag).
- +r: error "readonly attribute cannot be removed" + exit 1.
- -f: list functions (names only for v64; bodies deferred).
- -F: same as -f for v64.
- -p: print declarations of named vars (or missing → exit 1).
- --: ends flag parsing.
- Cluster combinations: -rx, -rxp, etc.

Inside-function semantics: \`declare NAME=val\` (or any mutation
form) snapshots the pre-state into the current local_scopes
frame BEFORE the mutation, so attribute changes unwind on
function exit. This makes \`declare\` behave like \`local\`
inside a function (matches bash without -g). The
\`snapshot_for_local_scope\` helper mirrors v52's per-frame
idempotent pattern.

Bare \`declare\` (no args + no flags): lists all variables
sorted by name as \`declare ATTR NAME="value"\` lines. Empty
attrs print as \`declare --\`; with readonly+exported,
\`declare -rx\`. Value escaping handles \", \\, \$, backtick
with backslash.

src/shell_state.rs: new \`pub fn unexport\` and (if missing)
\`pub fn is_exported\`, \`pub fn iter_vars\`.

\"declare\", \"typeset\" added to BUILTIN_NAMES; neither in
is_special_builtin (bash-specific, regular). Single dispatch
arm covers both names.

14 unit tests in \`mod declare_tests\`: bare-sets, -r locks,
-x exports, -rx combines, +x unexports, +r errors, -p prints,
-p missing errors, -f lists functions, -F named found/missing,
invalid-identifier, typeset alias, deferred -i errors.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

Stage exactly: `src/shell_state.rs src/builtins.rs`.

- [ ] **Step 1.14**

---

## Task 2: Integration tests

**Files:**
- Create `tests/declare_integration.rs`.

8 binary-driven tests.

### Step 2.1: Create the test file

Use the standard helper shape from prior integration tests:

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
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn declare_bare_assigns() {
    let (out, _, _) = run_capture("declare X=hi\necho \"[$X]\"\nexit\n");
    assert!(out.lines().any(|l| l == "[hi]"), "stdout: {out:?}");
}

#[test]
fn declare_p_prints_decl() {
    let (out, _, _) = run_capture("X=hi\ndeclare -p X\nexit\n");
    assert!(
        out.lines().any(|l| l == "declare -- X=\"hi\""),
        "stdout: {out:?}",
    );
}

#[test]
fn declare_r_is_readonly() {
    let (out, err, _) = run_capture(
        "declare -r X=hi\nX=new\nrc=$?\necho \"rc=$rc\"\nexit\n",
    );
    assert!(err.contains("readonly"), "stderr: {err:?}");
    assert!(out.lines().any(|l| l == "rc=1"), "stdout: {out:?}");
}

#[test]
fn declare_x_is_exported() {
    let (out, _, _) = run_capture(
        "declare -x X=hi\ndeclare -p X\nexit\n",
    );
    assert!(
        out.lines().any(|l| l == "declare -x X=\"hi\""),
        "stdout: {out:?}",
    );
}

#[test]
fn declare_plus_x_unexports() {
    let (out, _, _) = run_capture(
        "declare -x X=hi\ndeclare +x X\ndeclare -p X\nexit\n",
    );
    // After +x, attrs should be -- not -x.
    assert!(
        out.lines().any(|l| l == "declare -- X=\"hi\""),
        "stdout: {out:?}",
    );
}

#[test]
fn declare_inside_function_is_local() {
    let (out, _, _) = run_capture(
        "f() { declare X_LOCAL_DECL=in; }\nf\necho \"[$X_LOCAL_DECL]\"\nexit\n",
    );
    // X should be unset after function returns.
    assert!(
        out.lines().any(|l| l == "[]"),
        "stdout: {out:?}",
    );
}

#[test]
fn declare_F_lists_functions() {
    let (out, _, _) = run_capture(
        "f() { :; }\ndeclare -F\nexit\n",
    );
    assert!(
        out.lines().any(|l| l == "declare -f f"),
        "stdout: {out:?}",
    );
}

#[test]
fn typeset_alias_works() {
    let (out, err, _) = run_capture(
        "typeset -r X=hi\nX=new\nrc=$?\necho \"rc=$rc\"\nexit\n",
    );
    assert!(err.contains("readonly"), "stderr: {err:?}");
    assert!(out.lines().any(|l| l == "rc=1"), "stdout: {out:?}");
}
```

- [ ] **Step 2.1**

### Step 2.2: Run integration tests

```bash
cargo test --test declare_integration -- --nocapture
```

Expected: 8 pass.

- [ ] **Step 2.2**

### Step 2.3: Full integration suite

`cargo test --tests`. Expected: green (PTY flake tolerated).

- [ ] **Step 2.3**

### Step 2.4: Clippy

`cargo clippy --all-targets -- -D warnings`. Expected: clean.

- [ ] **Step 2.4**

### Step 2.5: Commit Task 2

```bash
git add tests/declare_integration.rs
git commit -m "$(cat <<'EOF'
test: declare integration coverage (v64 task 2)

8 binary-driven tests exercising declare/typeset end-to-end:

- declare_bare_assigns — declare X=hi; echo \$X → "hi".
- declare_p_prints_decl — X=hi; declare -p X → declare -- X="hi".
- declare_r_is_readonly — readonly enforcement post-declare -r.
- declare_x_is_exported — declare -p shows -x attribute.
- declare_plus_x_unexports — +x flips attribute back to --.
- declare_inside_function_is_local — function-scope semantics:
  X unset after function returns.
- declare_F_lists_functions — output contains "declare -f f".
- typeset_alias_works — typeset -r enforces readonly.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 2.5**

---

## Task 3: Docs

**Files:**
- Modify `docs/bash-divergences.md` — M-79 entry + v64
  change-log.
- Modify `README.md` — v64 row.

### Step 3.1: Add M-79 entry

After M-78 (v63 dirstack):

```markdown
- **M-79: `declare` / `typeset`** — `[fixed v64 partial]` medium. Bash variable-attribute builtin (Tier A: wires to existing huck primitives). `declare` and `typeset` dispatch to the same builtin (POSIX-classification: bash-specific regular). Supported flags: `-r` (readonly via v54's `mark_readonly`), `-x` (export), `+x` (un-export via new `Shell::unexport`), `+r` (errors: "readonly attribute cannot be removed"), `-f` / `-F` (list function names as `declare -f NAME` per line; bodies deferred), `-p` (print declarations of named vars). Flags cluster: `-rx`, `-rxp`. `--` ends flag parsing. Bare `declare` lists all variables sorted by name as `declare ATTR NAME="value"` (empty attrs print as `declare --`; readonly+exported → `declare -rx`). Value escaping uses `\\` for `"`, `\\`, `$`, backtick. Inside a function, `declare NAME[=val]` mutations snapshot the pre-state into the current local_scopes frame via the new `snapshot_for_local_scope` helper (mirrors v52's per-frame idempotency) so attribute changes unwind on function exit — matches bash's `declare = local without -g` semantics. **Deferred** ("not yet implemented" + exit 1 if used): `-i` (integer coercion), `-l` (lowercase), `-u` (uppercase), `-a` (indexed array — huck has no arrays), `-A` (associative array), `-n` (nameref), `-g` (force global). Also deferred: function-body printing in `-f` output (huck just emits `declare -f NAME` since there's no AST pretty-printer yet).
```

- [ ] **Step 3.1**

### Step 3.2: Add v64 change-log entry

In `## Change log` after v63:

```markdown
- **2026-05-31**: M-79 (`declare` / `typeset`) shipped as v64 partial. New `builtin_declare` in `src/builtins.rs` dispatched for both `"declare"` and `"typeset"`. Flag-cluster parser handles `-` and `+` prefixes; `+r` errors; `+x` sets the un-export intent. Per-name processing: snapshot for function-scope unwinding via new `snapshot_for_local_scope` helper (mirrors v52's per-frame snapshot pattern). Mutations route through v54's `mark_readonly`/`try_set` (for `-r`), existing `export`/`export_set` (for `-x`), and new `Shell::unexport` method (for `+x`). New `Shell::is_exported` and `Shell::iter_vars` accessors. `format_declare_line` builds `declare ATTR NAME="value"` strings with `\\` escaping of `"`, `\\`, `$`, backtick. Bare `declare` lists all vars sorted; `declare -p NAME` prints just that one; `declare -F` lists function names. Deferred flags (`-i`/`-l`/`-u`/`-a`/`-A`/`-n`/`-g`) emit "not yet implemented" + exit 1. `"declare"` and `"typeset"` added to `BUILTIN_NAMES`; neither in `is_special_builtin`. 14 unit tests + 8 integration tests. No new L-* divergences.
```

- [ ] **Step 3.2**

### Step 3.3: Add v64 row to README

After v63:

```markdown
| v64       | `declare` / `typeset` (M-79 partial)                           |
```

Match v63 column padding.

- [ ] **Step 3.3**

### Step 3.4: Full suite

`cargo test --all-targets`. Expected: green (PTY flake tolerated).

- [ ] **Step 3.4**

### Step 3.5: Clippy

`cargo clippy --all-targets -- -D warnings`. Expected: clean.

- [ ] **Step 3.5**

### Step 3.6: Commit Task 3

```bash
git add docs/bash-divergences.md README.md
git commit -m "$(cat <<'EOF'
docs: add M-79 (declare/typeset) fixed v64 partial

New M-79 entry in docs/bash-divergences.md covers Tier A scope:
-r/-x/+x/-f/-F/-p flags wired to v54 readonly + existing export
+ v52 local_scopes. Lists deferred attributes (-i/-l/-u/-a/-A/
-n/-g) and the function-body printing deferral.

Change log: 2026-05-31 v64 entry summarizing the new builtin,
snapshot_for_local_scope, format_declare_line/escape helpers,
the new Shell methods (unexport, is_exported, iter_vars), and
the 14+8 test split.

README: v64 row added.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 3.6**

---

## Final verification (controller)

1. `cargo test --all-targets` once more.
2. `cargo clippy --all-targets -- -D warnings`.
3. Branch is four commits ahead of `main`: docs preamble + 3
   task commits.
4. Dispatch a final cross-task reviewer.
5. Merge to `main` with `--no-ff`, push, delete branch, update
   memory.
