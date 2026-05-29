# huck v53 — trivials cluster Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship `:`, `true`, `false`, and `command -v`/`-V` as four
new builtins in a single iteration.

**Architecture:** Four standalone builtin functions in
`src/builtins.rs`. The first three are one-liners returning
`Continue(0)` / `Continue(1)`. `command -v/-V` adds a small
resolver that walks alias → function → builtin → keyword → `$PATH`
and prints either concise (`-v`) or verbose (`-V`) output.

**Tech Stack:** Rust. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-05-29-huck-trivials-design.md`

**Branch:** `v53-trivials` (created in preamble step P.1).

**Commit trailer convention:**

```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Preamble: Create the working branch

- [ ] **Step P.1: Create branch from main**

```bash
git checkout main
git pull --ff-only
git checkout -b v53-trivials
```

Expected: `Switched to a new branch 'v53-trivials'`.

The spec + this plan are committed as the first commit on the
branch by the controller before Task 1 begins.

---

## Task 1: 4 builtins + 12 unit tests

**Files:**
- Modify: `src/builtins.rs` — add 4 builtin functions + resolver
  helpers + dispatch + `BUILTIN_NAMES` + `is_special_builtin` +
  3 test modules.

### Step 1.1: Add `":"` to `is_special_builtin`

Current shape at `src/builtins.rs:34-40`:

```rust
pub fn is_special_builtin(name: &str) -> bool {
    matches!(name,
        "." | "break" | "continue" | "exit" | "export" | "return"
        | "set" | "shift" | "source" | "trap" | "unset"
    )
}
```

Add `":"` to the list:

```rust
pub fn is_special_builtin(name: &str) -> bool {
    matches!(name,
        ":" | "." | "break" | "continue" | "exit" | "export" | "return"
        | "set" | "shift" | "source" | "trap" | "unset"
    )
}
```

(Position `":"` first since it sorts ahead of `"."` ASCII-wise.)

The doc comment above the function says "expand here as huck adds
eval/exec/:/readonly" — leave the comment alone (still anticipating
`eval`/`exec`/`readonly`).

- [ ] **Step 1.1: Add `:` to is_special_builtin**

### Step 1.2: Add four entries to `BUILTIN_NAMES`

Current value at `src/builtins.rs:18-23`:

```rust
pub const BUILTIN_NAMES: &[&str] = &[
    "cd", "exit", "pwd", "echo", "export", "unset", "jobs",
    "wait", "fg", "bg", "kill", "disown", "history", "test", "[",
    "break", "continue", "return", "trap", "alias", "unalias",
    "set", "shift", ".", "source", "local",
];
```

Append `":"`, `"true"`, `"false"`, `"command"`:

```rust
pub const BUILTIN_NAMES: &[&str] = &[
    "cd", "exit", "pwd", "echo", "export", "unset", "jobs",
    "wait", "fg", "bg", "kill", "disown", "history", "test", "[",
    "break", "continue", "return", "trap", "alias", "unalias",
    "set", "shift", ".", "source", "local",
    ":", "true", "false", "command",
];
```

- [ ] **Step 1.2: Append to BUILTIN_NAMES**

### Step 1.3: Add the three trivial builtins

Insert near other small builtins (e.g., after `builtin_unset` or
near top of the impl section). Exact location flexible.

```rust
fn builtin_colon(_args: &[String], _shell: &mut Shell) -> ExecOutcome {
    ExecOutcome::Continue(0)
}

fn builtin_true(_args: &[String], _shell: &mut Shell) -> ExecOutcome {
    ExecOutcome::Continue(0)
}

fn builtin_false(_args: &[String], _shell: &mut Shell) -> ExecOutcome {
    ExecOutcome::Continue(1)
}
```

- [ ] **Step 1.3: Add `:` / `true` / `false`**

### Step 1.4: Add the `command` builtin and helpers

Insert in `src/builtins.rs`. Required imports near the top of the
file (only add what's not already imported):

```rust
use std::os::unix::fs::PermissionsExt;
```

(`std::path::PathBuf`, `std::fs`, `std::io::Write` are likely
already in scope or used via fully-qualified paths. Use fully
qualified paths if convenient to avoid touching existing imports.)

```rust
#[derive(Debug)]
enum CommandResolution {
    Alias(String),
    Function,
    Builtin,
    Keyword,
    File(std::path::PathBuf),
    NotFound,
}

fn is_shell_keyword(name: &str) -> bool {
    matches!(
        name,
        "if" | "then" | "elif" | "else" | "fi"
        | "while" | "until" | "do" | "done"
        | "for" | "in"
        | "case" | "esac"
        | "function"
        | "!"
        | "{" | "}"
        | "[[" | "]]"
    )
}

fn is_executable_file(p: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(p) {
        Ok(md) => md.is_file() && (md.permissions().mode() & 0o111 != 0),
        Err(_) => false,
    }
}

fn search_path_for(name: &str, shell: &Shell) -> Option<std::path::PathBuf> {
    if name.contains('/') {
        let p = std::path::PathBuf::from(name);
        if is_executable_file(&p) { Some(p) } else { None }
    } else {
        let path_val = shell.lookup_var("PATH").unwrap_or_default();
        for segment in path_val.split(':') {
            if segment.is_empty() {
                continue;
            }
            let candidate = std::path::Path::new(segment).join(name);
            if is_executable_file(&candidate) {
                return Some(candidate);
            }
        }
        None
    }
}

fn resolve_command_name(name: &str, shell: &Shell) -> CommandResolution {
    if let Some(value) = shell.aliases.get(name) {
        return CommandResolution::Alias(value.clone());
    }
    if shell.functions.contains_key(name) {
        return CommandResolution::Function;
    }
    if is_builtin(name) {
        return CommandResolution::Builtin;
    }
    if is_shell_keyword(name) {
        return CommandResolution::Keyword;
    }
    if let Some(path) = search_path_for(name, shell) {
        return CommandResolution::File(path);
    }
    CommandResolution::NotFound
}

fn builtin_command(
    args: &[String],
    out: &mut dyn std::io::Write,
    shell: &mut Shell,
) -> ExecOutcome {
    let mut concise = false;
    let mut verbose = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-v" => { concise = true; i += 1; }
            "-V" => { verbose = true; i += 1; }
            "--" => { i += 1; break; }
            s if s.starts_with('-') && s.len() > 1 => {
                eprintln!("huck: command: {s}: invalid option");
                return ExecOutcome::Continue(2);
            }
            _ => break,
        }
    }
    let names = &args[i..];

    if !concise && !verbose {
        // Bare `command cmd args` (run cmd bypassing function/alias
        // lookup) is deferred to a later iteration. With no name and
        // no flag, return 0 — matches bash's silent success.
        if names.is_empty() {
            return ExecOutcome::Continue(0);
        }
        eprintln!(
            "huck: command: bare form (without -v/-V) is not supported in this version"
        );
        return ExecOutcome::Continue(2);
    }

    if names.is_empty() {
        return ExecOutcome::Continue(0);
    }

    let mut any_not_found = false;
    for name in names {
        match resolve_command_name(name, shell) {
            CommandResolution::Alias(value) => {
                if concise {
                    let _ = writeln!(out, "alias {name}='{value}'");
                } else {
                    let _ = writeln!(out, "{name} is aliased to `{value}'");
                }
            }
            CommandResolution::Function => {
                if concise {
                    let _ = writeln!(out, "{name}");
                } else {
                    let _ = writeln!(out, "{name} is a function");
                }
            }
            CommandResolution::Builtin => {
                if concise {
                    let _ = writeln!(out, "{name}");
                } else {
                    let _ = writeln!(out, "{name} is a shell builtin");
                }
            }
            CommandResolution::Keyword => {
                if concise {
                    let _ = writeln!(out, "{name}");
                } else {
                    let _ = writeln!(out, "{name} is a shell keyword");
                }
            }
            CommandResolution::File(path) => {
                if concise {
                    let _ = writeln!(out, "{}", path.display());
                } else {
                    let _ = writeln!(out, "{name} is {}", path.display());
                }
            }
            CommandResolution::NotFound => {
                any_not_found = true;
                if verbose {
                    eprintln!("huck: command: {name}: not found");
                }
            }
        }
    }
    ExecOutcome::Continue(if any_not_found { 1 } else { 0 })
}
```

Notes:
- `Shell::aliases` is `pub` (verified at `shell_state.rs:32`).
- `Shell::functions` is `pub` (verified at `shell_state.rs:28`).
- `Shell::lookup_var` already exists on the public API surface.

- [ ] **Step 1.4: Add `command` + helpers**

### Step 1.5: Add dispatch arms

In `run_builtin`, add four arms. Natural location is near the
existing simple-builtins block. The "true"/"false" and ":" arms
can sit alongside the no-arg-style builtins; `command` near
`"alias"`/`"unalias"` reads well.

```rust
        ":" => builtin_colon(args, shell),
        "true" => builtin_true(args, shell),
        "false" => builtin_false(args, shell),
        "command" => builtin_command(args, out, shell),
```

(`out` is the writer parameter name — verify against the actual
`run_builtin` signature in `src/builtins.rs:44-49`.)

- [ ] **Step 1.5: Add dispatch arms**

### Step 1.6: Build

Run: `cargo build`
Expected: clean.

- [ ] **Step 1.6: Build clean**

### Step 1.7: Append `mod colon_tests` + `mod true_false_tests` + `mod command_tests`

At the end of `src/builtins.rs`, after the existing v52 `mod
local_tests`, append:

```rust
#[cfg(test)]
mod colon_tests {
    use super::*;
    use crate::shell_state::Shell;

    #[test]
    fn colon_exits_zero() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(":", &[], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
    }

    #[test]
    fn colon_with_args_exits_zero() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let args = vec!["one".to_string(), "two".to_string()];
        let outcome = run_builtin(":", &args, &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
    }
}

#[cfg(test)]
mod true_false_tests {
    use super::*;
    use crate::shell_state::Shell;

    #[test]
    fn true_exits_zero() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("true", &[], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
    }

    #[test]
    fn false_exits_one() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("false", &[], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn true_and_false_ignore_args() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let args = vec!["ignored".to_string()];
        let t = run_builtin("true", &args, &mut buf, &mut shell);
        let f = run_builtin("false", &args, &mut buf, &mut shell);
        assert!(matches!(t, ExecOutcome::Continue(0)));
        assert!(matches!(f, ExecOutcome::Continue(1)));
    }
}

#[cfg(test)]
mod command_tests {
    use super::*;
    use crate::shell_state::Shell;

    #[test]
    fn command_no_args_exits_zero() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("command", &[], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
    }

    #[test]
    fn command_bare_form_errors() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let args = vec!["echo".to_string(), "hi".to_string()];
        let outcome = run_builtin("command", &args, &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn command_dash_v_builtin_concise() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let args = vec!["-v".to_string(), "echo".to_string()];
        let outcome = run_builtin("command", &args, &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        assert_eq!(out.trim_end(), "echo");
    }

    #[test]
    fn command_dash_v_notfound_silent_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let args = vec!["-v".to_string(), "__no_such_cmd_xyzzy__".to_string()];
        let outcome = run_builtin("command", &args, &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
        let out = String::from_utf8(buf).unwrap();
        assert!(out.is_empty(), "expected silent stdout, got: {out:?}");
    }

    #[test]
    fn command_dash_V_builtin_verbose() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let args = vec!["-V".to_string(), "echo".to_string()];
        let outcome = run_builtin("command", &args, &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        assert_eq!(out.trim_end(), "echo is a shell builtin");
    }

    #[test]
    fn command_dash_V_keyword_verbose() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let args = vec!["-V".to_string(), "if".to_string()];
        let outcome = run_builtin("command", &args, &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        assert_eq!(out.trim_end(), "if is a shell keyword");
    }

    #[test]
    fn command_dash_v_function() {
        let mut shell = Shell::new();
        // Register a function directly. The body shape is irrelevant for
        // resolution; any Command value works. Use a no-op SimpleCommand.
        let body = Box::new(crate::command::Command::SimpleCommand(
            crate::command::SimpleCommand::default(),
        ));
        shell.functions.insert("myfn".to_string(), body);
        let mut buf: Vec<u8> = Vec::new();
        let args = vec!["-v".to_string(), "myfn".to_string()];
        let outcome = run_builtin("command", &args, &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let out = String::from_utf8(buf).unwrap();
        assert_eq!(out.trim_end(), "myfn");
    }
}
```

**Important about `command_dash_v_function`:** The test inserts
directly into `shell.functions`. The exact constructor for
`crate::command::Command` may differ — pick whatever the
existing code uses. If `SimpleCommand::default()` doesn't compile,
inspect the type and pick a minimal alternative (e.g.
`crate::command::Command::SimpleCommand(SimpleCommand { words:
vec![], assignments: vec![], redirs: vec![] })`). The test only
cares that `shell.functions.contains_key("myfn")` returns true.

- [ ] **Step 1.7: Append unit test modules**

### Step 1.8: Run unit tests

```bash
cargo test --bin huck colon_tests true_false_tests command_tests
```

Expected: 12 tests pass (2 + 3 + 7).

If `command_dash_v_function` fails to compile because of the
function-body constructor, adjust to match the actual `Command`
shape. Goal: insert a Command into `shell.functions` such that
`contains_key("myfn")` returns true.

- [ ] **Step 1.8: 12 unit tests pass**

### Step 1.9: Full unit suite

`cargo test --bin huck`
Expected: full unit suite green.

- [ ] **Step 1.9: Full unit suite green**

### Step 1.10: Clippy

`cargo clippy --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 1.10: Clippy clean**

### Step 1.11: Commit Task 1

```bash
git add src/builtins.rs
git commit -m "$(cat <<'EOF'
builtin: : / true / false / command -v/-V (v53 task 1)

Four new builtins:
- `:` (POSIX special): null command, exits 0 after argument
  expansion. Added to `is_special_builtin`.
- `true`: exits 0 (regular).
- `false`: exits 1 (regular).
- `command -v NAME` / `command -V NAME`: prints how NAME resolves,
  walking alias → function → builtin → keyword → $PATH.
  `-v` is concise (the name itself, or the resolved path); `-V` is
  verbose ("NAME is a shell builtin", "NAME is /usr/bin/NAME", etc).
  Exit status: 0 if all names resolved, 1 if any missing. `-V` writes
  "huck: command: NAME: not found" to stderr; `-v` is silent on
  stderr. Bare `command cmd args` (bypass function/alias lookup)
  is rejected with status 2 — deferred to a future iteration.

Resolver: walks `shell.aliases`, `shell.functions`, `is_builtin`,
`is_shell_keyword` (hardcoded set matching huck's parser), then
`is_executable_file` over `$PATH` (or literal path if `/` is in
the name). Uses `std::os::unix::fs::PermissionsExt` to check the
exec bit.

All four added to `BUILTIN_NAMES`. `:` joins `is_special_builtin`'s
set; the others stay regular.

12 new unit tests: 2 in `mod colon_tests`, 3 in `mod
true_false_tests`, 7 in `mod command_tests` (no-args; bare-form
error; -v builtin; -v not-found silent; -V builtin verbose;
-V keyword verbose; -v function).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

Stage exactly `src/builtins.rs`.

- [ ] **Step 1.11: Commit Task 1**

---

## Task 2: Integration tests

**Files:**
- Create: `tests/trivials_integration.rs`

8 binary-driven tests.

### Step 2.1: Create the integration test file

Match the helper shape used by `tests/local_integration.rs` (v52)
and `tests/source_integration.rs` (v51).

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
fn colon_is_no_op() {
    let (out, _) = run_capture(": anything\necho ok\nexit\n");
    assert!(out.lines().any(|l| l == "ok"), "stdout: {out:?}");
}

#[test]
fn colon_triggers_param_default_assignment() {
    let (out, _) = run_capture(": ${X:=hello}\necho \"$X\"\nexit\n");
    assert!(out.lines().any(|l| l == "hello"), "stdout: {out:?}");
}

#[test]
fn true_in_conditional() {
    let (out, _) = run_capture("if true; then echo Y; fi\nexit\n");
    assert!(out.lines().any(|l| l == "Y"), "stdout: {out:?}");
}

#[test]
fn false_in_conditional() {
    let (out, _) = run_capture("if false; then echo Y; else echo N; fi\nexit\n");
    assert!(out.lines().any(|l| l == "N"), "stdout: {out:?}");
}

#[test]
fn command_v_finds_builtin() {
    let (out, _) = run_capture("command -v echo\nexit\n");
    assert!(
        out.lines().any(|l| l == "echo"),
        "expected `echo` line, got: {out:?}"
    );
}

#[test]
fn command_v_missing_status_1() {
    let (out, _) = run_capture(
        "command -v __no_such_cmd_xyzzy__\nrc=$?\necho rc=$rc\nexit\n",
    );
    let rc_line = out
        .lines()
        .find(|l| l.starts_with("rc="))
        .unwrap_or_else(|| panic!("no rc= line; got: {out:?}"));
    assert_eq!(rc_line, "rc=1", "stdout: {out:?}");
}

#[test]
fn command_v_finds_path_binary() {
    let (out, _) = run_capture("command -v sh\nexit\n");
    let sh_line = out.lines().find(|l| l.contains('/'));
    assert!(
        sh_line.is_some(),
        "expected a path containing `/`, got: {out:?}"
    );
}

#[test]
fn command_V_keyword() {
    let (out, _) = run_capture("command -V if\nexit\n");
    assert!(
        out.lines().any(|l| l == "if is a shell keyword"),
        "stdout: {out:?}"
    );
}
```

- [ ] **Step 2.1: Create the file**

### Step 2.2: Run the integration suite

```bash
cargo test --test trivials_integration -- --nocapture
```

Expected: 8 tests pass.

Failure mode to watch: `command_v_finds_path_binary` requires that
`sh` is findable on `$PATH`. On any standard Linux/macOS system
this is always true. If the test env truly has no `sh` on PATH,
swap to a more universal binary (`true` and `false` are NOT
suitable — they're builtins in huck).

- [ ] **Step 2.2: 8 tests pass**

### Step 2.3: Full integration suite

`cargo test --tests`
Expected: green (PTY flake tolerated).

- [ ] **Step 2.3: Full integration suite green**

### Step 2.4: Clippy

`cargo clippy --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 2.4: Clippy clean**

### Step 2.5: Commit Task 2

```bash
git add tests/trivials_integration.rs
git commit -m "$(cat <<'EOF'
test: trivials integration coverage (v53 task 2)

8 binary-driven tests exercising the new v53 builtins end-to-end:
- colon_is_no_op + colon_triggers_param_default_assignment.
- true_in_conditional + false_in_conditional.
- command_v_finds_builtin (`command -v echo` → "echo").
- command_v_missing_status_1 (rc=1 captured immediately as $?).
- command_v_finds_path_binary (PATH search for `sh` produces a
  path with `/`).
- command_V_keyword (`command -V if` → "if is a shell keyword").

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 2.5: Commit Task 2**

---

## Task 3: Docs

**Files:**
- Modify: `docs/bash-divergences.md` — add M-68, M-69, M-70 entries
  + change-log entry.
- Modify: `README.md` — add v53 row.

### Step 3.1: Add M-68, M-69, M-70 in `docs/bash-divergences.md`

Find the M-67 entry (v52). Insert IMMEDIATELY after it (same Tier
2 section):

```markdown
- **M-68: `:` (null command)** — `[fixed v53]` low. POSIX special builtin. Always exits 0 after huck's normal argument expansion runs, so `: ${VAR:=default}` triggers the param-expansion default-assignment side effect and `while :` is an infinite loop. Added to `is_special_builtin`.
- **M-69: `true` / `false`** — `[fixed v53]` low. Regular builtins. `true` exits 0; `false` exits 1. Both ignore their args (matches bash).
- **M-70: `command -v` / `-V`** — `[fixed v53]` medium. POSIX `command` builtin with `-v` (concise) and `-V` (verbose) introspection flags. Walks alias → function → builtin → keyword → `$PATH` (literal path if the name contains `/`). `-v` prints the name (or absolute path); `-V` prints "NAME is a shell builtin" / "NAME is a function" / "NAME is a shell keyword" / "NAME is aliased to \`value'" / "NAME is /path/to/NAME". Exit 0 if all names resolved, 1 if any missing. `-V` writes "huck: command: NAME: not found" to stderr on miss; `-v` is silent. Bare `command cmd args` (bypass function/alias lookup) and `-p` (default-PATH search) are deferred.
```

- [ ] **Step 3.1: Add M-68, M-69, M-70**

### Step 3.2: Add v53 change-log entry

Find `## Change log`. After the most recent v52 entry (M-67):

```markdown
- **2026-05-29**: M-68 (`:`), M-69 (`true`/`false`), M-70 (`command -v`/`-V`) shipped together as v53 — the trivials cluster. Four small builtins in `src/builtins.rs`: `builtin_colon` and `builtin_true` return `Continue(0)`, `builtin_false` returns `Continue(1)`, all ignoring args. `builtin_command` parses `-v`/`-V` flags from the left and resolves each remaining name via `resolve_command_name` (alias → function → builtin → keyword → `$PATH`). Helpers: `CommandResolution` enum, `is_shell_keyword` (hardcoded set), `search_path_for` (handles names containing `/` as literal paths; otherwise iterates `$PATH` split on `:`, skipping empty segments), `is_executable_file` (Unix `mode & 0o111`). `:` added to `is_special_builtin`; `true`/`false`/`command` regular. Bare-form `command cmd args` rejected with status 2 (deferred). 12 unit tests + 8 integration tests. No new L-* divergences.
```

- [ ] **Step 3.2: Add change-log entry**

### Step 3.3: Add v53 row to README

In `README.md`, find the version table. After the v52 row (search
for `| v52       |`), add IMMEDIATELY after it:

```markdown
| v53       | `:` (M-68), `true` / `false` (M-69), `command -v`/`-V` (M-70)  |
```

Verify the existing row's column widths and match them exactly.

- [ ] **Step 3.3: Add README v53 row**

### Step 3.4: Full suite

`cargo test --all-targets`
Expected: green (PTY flake tolerated).

- [ ] **Step 3.4: Full suite green**

### Step 3.5: Clippy

`cargo clippy --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 3.5: Clippy clean**

### Step 3.6: Commit Task 3

```bash
git add docs/bash-divergences.md README.md
git commit -m "$(cat <<'EOF'
docs: add M-68 (:), M-69 (true/false), M-70 (command) fixed v53

Three new divergence entries in docs/bash-divergences.md mark the
v53 trivials cluster as `[fixed v53]`:
- M-68 `:` (POSIX special null command, triggers arg expansion,
  always exits 0).
- M-69 `true` / `false` (exit 0 / exit 1; args ignored).
- M-70 `command -v` / `-V` (introspection flags; resolves alias /
  function / builtin / keyword / PATH; -v concise, -V verbose;
  status 1 if any name not found; bare form deferred).

Change log: 2026-05-29 v53 entry summarizing the four builtins,
the CommandResolution resolver, and the helper functions.

README: v53 row added to the version table.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 3.6: Commit Task 3**

---

## Final verification (controller, not a task)

After the three task commits land:

1. `cargo test --all-targets` once more.
2. `cargo clippy --all-targets -- -D warnings`.
3. Branch has exactly four commits ahead of `main`: docs preamble
   (spec + plan), task 1, task 2, task 3.
4. Dispatch a final cross-task code-reviewer subagent over
   `main..v53-trivials`.
5. Merge to `main` with `--no-ff`, push, delete the branch, update
   `huck iterations` memory with v53.
