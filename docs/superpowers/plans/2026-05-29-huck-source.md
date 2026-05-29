# huck v51 — `source` and `.` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the POSIX special builtin `.` and its bash alias
`source` — read and execute commands from a file in the current shell
context. PATH search for slashless filenames, optional argument
passing, recursive depth cap at 64, `return N` early-exits the source.

**Architecture:** Two-file change. `src/shell_state.rs` gains a
`source_depth: u32` counter. `src/shell.rs` makes two error-message
helpers `pub(crate)`. `src/builtins.rs` adds the builtin (with helpers
`resolve_source_path` and `run_sourced_contents`), dispatch arms,
`BUILTIN_NAMES`/`is_special_builtin` updates, and 5 unit tests. The
multi-line accumulation in the sourced file reuses
`crate::continuation::classify`.

**Tech Stack:** Rust. No new dependencies (integration tests use
`std::fs` for tmp files).

**Spec:** `docs/superpowers/specs/2026-05-29-huck-source-design.md`

**Branch:** `v51-source` (created in preamble step P.1).

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
git checkout -b v51-source
```

Expected: `Switched to a new branch 'v51-source'`.

The spec + this plan are committed as the first commit on this branch
(handled by the controller before Task 1 begins).

---

## Task 1: Foundation + builtin + 5 unit tests

**Files:**
- Modify: `src/shell_state.rs` — add `pub source_depth: u32` field;
  init in `Shell::new`.
- Modify: `src/shell.rs` — change `fn lex_error_message` and
  `fn parse_error_message` to `pub(crate)`.
- Modify: `src/builtins.rs` — add `builtin_source`,
  `resolve_source_path`, `run_sourced_contents`; dispatch arm;
  `BUILTIN_NAMES` and `is_special_builtin` extensions; doc-comment
  trim; append `mod source_tests` with 5 tests.

### Step 1.1: Add `source_depth` field to `Shell`

In `src/shell_state.rs`, find the `pub struct Shell { ... }` block
(starts at line 19). Inside the struct, add a new field. Natural
position: near other counters (e.g. after `err_suppressed_depth`):

```rust
    /// Recursive `source`/`.` call depth. Capped at 64 in
    /// `builtin_source` to prevent runaway loops. Increment on
    /// enter, decrement on exit.
    pub source_depth: u32,
```

Then in `impl Shell { pub fn new() -> Self { ... } }`, find the
struct literal and add the initializer alongside other zero-init
counters:

```rust
            source_depth: 0,
```

- [ ] **Step 1.1: Add the field**

### Step 1.2: Make error-message helpers `pub(crate)`

In `src/shell.rs:270`, find:

```rust
fn parse_error_message(error: ParseError) -> String {
```

Change to:

```rust
pub(crate) fn parse_error_message(error: ParseError) -> String {
```

In `src/shell.rs:315`, find:

```rust
fn lex_error_message(error: LexError) -> String {
```

Change to:

```rust
pub(crate) fn lex_error_message(error: LexError) -> String {
```

- [ ] **Step 1.2: Adjust visibility**

### Step 1.3: Build to confirm

Run: `cargo build`
Expected: clean.

- [ ] **Step 1.3: Build clean**

### Step 1.4: Add `builtin_source` + helpers in `src/builtins.rs`

Find a natural insertion point near other builtins. After the v50
`builtin_set` / `set_escape_value` is a good fit. Insert:

```rust
fn builtin_source(args: &[String], shell: &mut Shell) -> ExecOutcome {
    if args.is_empty() {
        eprintln!("huck: .: usage: . filename [arguments]");
        return ExecOutcome::Continue(2);
    }
    if shell.source_depth >= 64 {
        eprintln!("huck: .: maximum source depth (64) exceeded");
        return ExecOutcome::Continue(1);
    }
    let filename = &args[0];
    let path = match resolve_source_path(filename, shell) {
        Some(p) => p,
        None => {
            eprintln!("huck: .: {filename}: file not found");
            return ExecOutcome::Continue(1);
        }
    };
    let contents = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("huck: .: {}: {e}", path.display());
            return ExecOutcome::Continue(1);
        }
    };
    let extra: Vec<String> = args[1..].to_vec();
    let saved_positional = if !extra.is_empty() {
        let saved = std::mem::take(&mut shell.positional_args);
        shell.positional_args = extra;
        Some(saved)
    } else {
        None
    };

    shell.source_depth += 1;
    let result = run_sourced_contents(&contents, &path, shell);
    shell.source_depth -= 1;

    if let Some(saved) = saved_positional {
        shell.positional_args = saved;
    }
    result
}

fn resolve_source_path(
    filename: &str,
    shell: &crate::shell_state::Shell,
) -> Option<std::path::PathBuf> {
    use std::path::PathBuf;
    if filename.contains('/') {
        let p = PathBuf::from(filename);
        return if p.is_file() { Some(p) } else { None };
    }
    let path_var = shell.lookup_var("PATH").unwrap_or_default();
    for dir in path_var.split(':') {
        if dir.is_empty() {
            continue;
        }
        let candidate = PathBuf::from(dir).join(filename);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn run_sourced_contents(
    contents: &str,
    path: &std::path::Path,
    shell: &mut crate::shell_state::Shell,
) -> ExecOutcome {
    use crate::continuation::{classify, Completeness};
    let mut last_status = shell.last_status();
    let mut buf = String::new();
    for line in contents.lines() {
        buf.push_str(line);
        buf.push('\n');
        if let Completeness::Incomplete(_) = classify(&buf) {
            continue;
        }
        let tokens = match crate::lexer::tokenize(&buf) {
            Ok(t) if t.is_empty() => {
                buf.clear();
                continue;
            }
            Ok(t) => t,
            Err(e) => {
                eprintln!(
                    "huck: {}: syntax error{}",
                    path.display(),
                    crate::shell::lex_error_message(e)
                );
                last_status = 2;
                buf.clear();
                continue;
            }
        };
        match crate::command::parse(tokens) {
            Ok(Some(seq)) => {
                let outcome = crate::executor::execute(&seq, shell, &buf);
                buf.clear();
                match outcome {
                    ExecOutcome::Continue(c) => last_status = c,
                    ExecOutcome::Exit(n) => return ExecOutcome::Exit(n),
                    ExecOutcome::FunctionReturn(n) => {
                        return ExecOutcome::Continue(n);
                    }
                    ExecOutcome::LoopBreak | ExecOutcome::LoopContinue => {
                        last_status = 0;
                    }
                }
            }
            Ok(None) => buf.clear(),
            Err(e) => {
                eprintln!(
                    "huck: {}: syntax error: {}",
                    path.display(),
                    crate::shell::parse_error_message(e)
                );
                last_status = 2;
                buf.clear();
            }
        }
    }
    ExecOutcome::Continue(last_status)
}
```

- [ ] **Step 1.4: Insert the builtin + helpers**

### Step 1.5: Add `"."` and `"source"` to `BUILTIN_NAMES`

In `src/builtins.rs:18-22`, the current array (post-v50) reads:

```rust
pub const BUILTIN_NAMES: &[&str] = &[
    "cd", "exit", "pwd", "echo", "export", "unset", "jobs",
    "wait", "fg", "bg", "kill", "disown", "history", "test", "[",
    "break", "continue", "return", "trap", "alias", "unalias",
    "set", "shift",
];
```

Replace with:

```rust
pub const BUILTIN_NAMES: &[&str] = &[
    "cd", "exit", "pwd", "echo", "export", "unset", "jobs",
    "wait", "fg", "bg", "kill", "disown", "history", "test", "[",
    "break", "continue", "return", "trap", "alias", "unalias",
    "set", "shift", ".", "source",
];
```

- [ ] **Step 1.5: Update BUILTIN_NAMES**

### Step 1.6: Extend `is_special_builtin` + trim doc comment

In `src/builtins.rs` (post-v50), the current code reads roughly:

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

Replace with:

```rust
/// True for POSIX "special builtins" (2.14). Inline assignments preceding a
/// special builtin persist in the shell; assignments preceding a regular
/// builtin or external command are scoped to the command. The set is huck's
/// existing builtins intersected with the POSIX special list; expand here as
/// huck adds `eval`/`exec`/`:`/`readonly`.
pub fn is_special_builtin(name: &str) -> bool {
    matches!(name,
        "." | "break" | "continue" | "exit" | "export" | "return"
        | "set" | "shift" | "source" | "trap" | "unset"
    )
}
```

Note: `.` is the POSIX-special name; `source` is the bash synonym.
huck treats both as special since huck doesn't distinguish POSIX
from bash modes.

- [ ] **Step 1.6: Update `is_special_builtin`**

### Step 1.7: Add dispatch arm in `run_builtin`

In `src/builtins.rs`, find the `match name { ... }` block inside
`run_builtin` (around line 46-66 post-v50). The current state ends
with the v50 additions:

```rust
        "trap" => builtin_trap(args, out, shell),
        "set" => builtin_set(args, out, shell),
        "shift" => builtin_shift(args, shell),
        "test" | "[" => builtin_test(name, args),
        // ...
```

Add a new arm right after `"shift"`:

```rust
        "shift" => builtin_shift(args, shell),
        "." | "source" => builtin_source(args, shell),
        "test" | "[" => builtin_test(name, args),
```

- [ ] **Step 1.7: Add dispatch arm**

### Step 1.8: Build + run a smoke test

Run: `cargo build`
Expected: clean.

Run: `cargo test --bin huck` (no filter, smoke test)
Expected: all 1242 pre-existing tests still pass (no new tests
yet — just smoke).

- [ ] **Step 1.8: Build + smoke green**

### Step 1.9: Append `mod source_tests` with 5 unit tests

At the end of `src/builtins.rs` (after the v50 `mod set_tests`),
append:

```rust
#[cfg(test)]
mod source_tests {
    use super::*;
    use crate::shell_state::Shell;

    #[test]
    fn source_no_args_returns_usage_status_2() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(".", &[], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(2)));
    }

    #[test]
    fn source_missing_file_errors_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            ".",
            &["/nonexistent/file/path/huck-v51-test".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn source_depth_limit_errors_status_1() {
        let mut shell = Shell::new();
        shell.source_depth = 64;
        let mut buf: Vec<u8> = Vec::new();
        // Use a path that would otherwise resolve fine — depth check
        // fires before the path resolution.
        let outcome = run_builtin(
            ".",
            &["/etc/hostname".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
        // Counter unchanged because the early return bypasses the
        // increment.
        assert_eq!(shell.source_depth, 64);
    }

    #[test]
    fn is_builtin_recognises_dot_and_source() {
        assert!(is_builtin("."));
        assert!(is_builtin("source"));
    }

    #[test]
    fn is_special_builtin_includes_dot_and_source() {
        assert!(is_special_builtin("."));
        assert!(is_special_builtin("source"));
    }
}
```

- [ ] **Step 1.9: Append unit tests**

### Step 1.10: Run the new tests

Run: `cargo test --bin huck source_tests:: -- --nocapture`
Expected: 5 tests pass.

- [ ] **Step 1.10: 5 unit tests pass**

### Step 1.11: Full unit suite

Run: `cargo test --bin huck`
Expected: all unit tests pass.

- [ ] **Step 1.11: Full unit suite passes**

### Step 1.12: Clippy

Run: `cargo clippy --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 1.12: Clippy clean**

### Step 1.13: Commit

```bash
git add src/shell_state.rs src/shell.rs src/builtins.rs
git commit -m "$(cat <<'EOF'
builtin: source / . (v51 task 1)

Add the POSIX special builtin `.` (and its bash alias `source`)
that reads and executes commands from a file in the current shell
context.

Foundation:
- New `pub source_depth: u32` field on Shell in src/shell_state.rs;
  initialized 0 in Shell::new.
- src/shell.rs: lex_error_message and parse_error_message gained
  pub(crate) visibility so the source builtin can render errors in
  the same format the REPL uses.

Builtin (src/builtins.rs):
- builtin_source: handles both `.` and `source`. No args → usage
  error 2. Depth check (>= 64) → "maximum source depth (64)
  exceeded" + 1. Path resolution via resolve_source_path: filename
  with '/' is literal; otherwise PATH search via $PATH split-on-':'.
  Missing or unreadable → status 1.
- run_sourced_contents: multi-line accumulation using
  crate::continuation::classify to defer Incomplete buffers. Each
  parsed Sequence executes through the existing executor::execute.
  Continue(c) updates last_status; Exit(n) propagates; FunctionReturn(n)
  early-exits this source with Continue(n) (bash-faithful `return`
  semantics inside a sourced file); LoopBreak/LoopContinue at the
  source's top level become no-ops with status 0.

Plumbing:
- `.` and `source` added to BUILTIN_NAMES.
- Both routed via `"." | "source" => builtin_source(...)` arm in
  run_builtin.
- Both added to is_special_builtin's matched set (POSIX classifies
  `.` as special; huck treats `source` symmetrically). Doc comment
  trimmed accordingly.

Extra args after the filename become positional during the sourced
execution and are restored on return. No extra args → positional
inherited unchanged.

5 new unit tests in mod source_tests: no-args usage status 2,
missing-file status 1, depth-limit status 1, is_builtin recognises
both names, is_special_builtin includes both.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 1.13: Commit Task 1**

---

## Task 2: Integration tests

**Files:**
- Create: `tests/source_integration.rs`

Five binary-driven tests + a `write_tmp` helper.

### Step 2.1: Create the integration test file

Create `tests/source_integration.rs` with this content:

```rust
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

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

fn run_capture_with_path(script: &str, extra_path: &str) -> (String, String) {
    let path = std::env::var("PATH").unwrap_or_default();
    let new_path = format!("{extra_path}:{path}");
    let mut child = Command::new(huck_binary())
        .env("PATH", &new_path)
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

/// Writes `content` to a unique tmp file and returns its path. Files
/// are left in /tmp; the OS sweeps them eventually.
fn write_tmp(content: &str) -> PathBuf {
    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .subsec_nanos();
    // Add a per-call counter as well so tests in the same process
    // don't collide if SystemTime resolution is coarse.
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("huck_v51_{pid}_{nanos}_{n}.sh"));
    std::fs::write(&path, content).expect("write tmp file");
    path
}

#[test]
fn source_runs_file_contents() {
    let tmp = write_tmp("echo HELLO\n");
    let script = format!("source {}\nexit\n", tmp.display());
    let (out, _) = run_capture(&script);
    assert!(
        out.lines().any(|l| l == "HELLO"),
        "expected HELLO in: {:?}",
        out
    );
}

#[test]
fn source_passes_extra_args_as_positional() {
    let tmp = write_tmp("echo \"$1 $2\"\n");
    let script = format!("source {} A B\nexit\n", tmp.display());
    let (out, _) = run_capture(&script);
    assert!(
        out.lines().any(|l| l == "A B"),
        "expected `A B` in: {:?}",
        out
    );
}

#[test]
fn source_return_early_exits() {
    let tmp = write_tmp("echo BEFORE\nreturn 0\necho SKIP\n");
    let script = format!("source {}\necho AFTER\nexit\n", tmp.display());
    let (out, _) = run_capture(&script);
    assert!(
        out.lines().any(|l| l == "BEFORE"),
        "expected BEFORE in: {:?}",
        out
    );
    assert!(
        !out.lines().any(|l| l == "SKIP"),
        "expected SKIP to be suppressed: {:?}",
        out
    );
    assert!(
        out.lines().any(|l| l == "AFTER"),
        "expected AFTER (host shell continues): {:?}",
        out
    );
}

#[test]
fn source_via_dot_alias() {
    let tmp = write_tmp("echo HELLO_DOT\n");
    let script = format!(". {}\nexit\n", tmp.display());
    let (out, _) = run_capture(&script);
    assert!(
        out.lines().any(|l| l == "HELLO_DOT"),
        "expected HELLO_DOT in: {:?}",
        out
    );
}

#[test]
fn source_path_lookup() {
    // Place a tmp file in a directory we'll prepend to PATH.
    // tmp file lives in std::env::temp_dir(); prepend that dir to
    // PATH; source by bare basename.
    let tmp = write_tmp("echo PATH_HIT\n");
    let dir = tmp.parent().unwrap().to_path_buf();
    let basename = tmp.file_name().unwrap().to_string_lossy().to_string();
    let script = format!("source {basename}\nexit\n");
    let (out, _) = run_capture_with_path(&script, dir.to_string_lossy().as_ref());
    assert!(
        out.lines().any(|l| l == "PATH_HIT"),
        "expected PATH_HIT in: {:?}",
        out
    );
}
```

- [ ] **Step 2.1: Create the file**

### Step 2.2: Run the integration suite

Run: `cargo test --test source_integration -- --nocapture`
Expected: all 5 tests pass.

If `source_path_lookup` fails because PATH inheritance interacts
weirdly with `Command::env`: `Command` doesn't inherit parent env
by default when `.env()` is called — confirm by inspecting the
`run_capture_with_path` helper. It DOES inherit (the `.env()` call
only overrides the named var). If the test still fails, the
implementer should investigate.

If `source_return_early_exits` fails because `return` outside a
function bubbles up to the REPL and resets last_status to 0: that's
the existing REPL behavior. The test verifies the source's
top-level `return` is caught INSIDE `run_sourced_contents` (Task 1
step 1.4 has that arm), so it should work.

- [ ] **Step 2.2: Tests pass**

### Step 2.3: Full integration suite

Run: `cargo test --tests`
Expected: all integration tests pass. PTY flake tolerated.

- [ ] **Step 2.3: Full integration suite green**

### Step 2.4: Clippy

Run: `cargo clippy --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 2.4: Clippy clean**

### Step 2.5: Commit

```bash
git add tests/source_integration.rs
git commit -m "$(cat <<'EOF'
test: source / . integration coverage (v51 task 2)

Five binary-driven tests verifying source end-to-end:
- source_runs_file_contents: write `echo HELLO` to tmp; source
  reads + executes; stdout has HELLO.
- source_passes_extra_args_as_positional: `source file A B` makes
  $1=A, $2=B during the sourced run.
- source_return_early_exits: `return 0` mid-source suppresses
  subsequent lines but the host shell continues normally.
- source_via_dot_alias: same as the first test using `.` instead of
  `source`.
- source_path_lookup: bare-name source resolves via $PATH.

Test helper write_tmp uses pid + nanos + atomic counter for
collision-free tmp paths; cleanup is OS-driven (files live in
/tmp/huck_v51_*). run_capture_with_path prepends a dir to PATH
for the path-lookup test.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 2.5: Commit Task 2**

---

## Task 3: Docs

**Files:**
- Modify: `docs/bash-divergences.md` — add new M-66 entry,
  change-log entry.
- Modify: `README.md` — v51 row.

### Step 3.1: Add M-66 entry in `docs/bash-divergences.md`

Find a Tier 2 section that fits — "Special builtins" or "Builtins"
naturally houses this. Search for `### Special builtins` or
`### Builtins` (the v50 commit added a similar entry; place M-66
adjacent).

Add this entry:

```markdown
- **M-66: `source` and `.`** — `[fixed v51]` medium. Reads and executes commands from a file in the current shell context. Filename without `/` is searched in `$PATH` (bash-faithful). Optional arguments after the filename become positional parameters during the sourced execution, restored on return. `return N` inside a sourced file early-exits with status N (bash-faithful). Recursive depth capped at 64 to prevent runaway loops. `.` and `source` are aliases — both are added to `is_special_builtin`. Multi-line constructs are accumulated via `crate::continuation::classify`, matching the REPL's line-continuation behavior.
```

- [ ] **Step 3.1: Add M-66 entry**

### Step 3.2: Add v51 change-log entry

In `docs/bash-divergences.md`, find `## Change log` and the most
recent `**2026-05-29**` entry (v50, M-65). Add IMMEDIATELY after
it:

```markdown
- **2026-05-29**: M-66 (`source` / `.`) shipped as v51. New `Shell.source_depth: u32` field. `builtin_source` resolves the path (literal if slash present, else `$PATH` lookup), reads the file, optionally pushes extra args as positional, increments depth, and runs lines with `crate::continuation::classify` handling multi-line accumulation. `return` at the source's top level catches as `Continue(n)` (bash-faithful early-exit); `exit` propagates as-is. `lex_error_message` and `parse_error_message` in `src/shell.rs` gained `pub(crate)` visibility so the source builtin can render parse errors in the REPL's format. No new L-* divergences.
```

- [ ] **Step 3.2: Add change-log entry**

### Step 3.3: Add v51 row to README

In `README.md`, find the version table. After the v50 row (search
for `| v50       |`), add IMMEDIATELY after it:

```markdown
| v51       | `source` / `.` (M-66)                                          |
```

Match column padding to v49/v50 (count actual trailing spaces).

- [ ] **Step 3.3: Add README v51 row**

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
docs: add M-66 (source / .) fixed v51

New M-66 entry in docs/bash-divergences.md tracks the POSIX
special builtin `.` and its bash alias `source` as [fixed v51].
Covers PATH search for slashless filenames, optional positional-
arg pass-through, bash-faithful return-early-exits, and the
64-level depth cap.

Change log: 2026-05-29 v51 entry summarizing the
Shell.source_depth field, builtin_source's path/read/dispatch
loop, the continuation-classifier reuse for multi-line
accumulation, and the pub(crate) visibility bumps for the
error-message helpers.

README: v51 row added to the version table.

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
   full diff (`main..v51-source`).
5. Merge to `main` with `--no-ff`, push, delete the branch, update
   the `huck iterations` memory with v51.
