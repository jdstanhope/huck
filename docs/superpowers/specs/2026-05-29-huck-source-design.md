# huck v51 — `source` and `.` (M-66)

## Goal

Add the POSIX special builtin `.` (and its bash alias `source`) that
reads and executes commands from a file in the current shell context.

After v51:

- `. file` and `source file` work identically.
- Filename without `/` is searched in `$PATH`.
- Optional arguments after the filename become positional parameters
  during the sourced execution, restored on return.
- `return N` inside the sourced file early-exits with status N
  (bash-faithful).
- Recursive depth capped at 64 to prevent runaway loops.

New tracked divergence: **M-66: `source` and `.`**.

## Scope decisions (locked)

1. **PATH search**: filenames without `/` are searched in `$PATH`
   (bash-faithful for both `.` and `source`).
2. **Recursive depth limit**: 64 levels. Exceeding errors with
   "maximum source depth exceeded" + status 1.
3. **`return` inside source**: early-exits the source with the given
   status. Bash-faithful.
4. **`exit` inside source**: propagates up; exits the whole shell.
5. **Arguments**: extra args become positional only DURING the source;
   restored on return. If no extra args, positional inherited
   (unchanged) from caller.

## Out of scope (deferred)

- `set -e` errexit interaction (`set` options not yet implemented).
- Sourcing FIFOs / device files / binary files.
- Cycle detection beyond the simple depth limit (e.g. `a.sh`
  sources `b.sh` sources `a.sh`).

## Architecture

Two-file change.

### `src/shell_state.rs`

Add a depth counter to `Shell`:

```rust
pub source_depth: u32,
```

Initialize to 0 in `Shell::new`. Public so the builtin can read +
mutate.

### `src/shell.rs`

Make the two existing private error-message helpers visible to
`src/builtins.rs`:

```rust
pub(crate) fn lex_error_message(error: LexError) -> String { ... }
pub(crate) fn parse_error_message(error: ParseError) -> String { ... }
```

These currently live at `src/shell.rs:270` and `:315`. Just change
the visibility — no body changes.

### `src/builtins.rs` — new builtin

```rust
fn builtin_source(args: &[String], shell: &mut Shell) -> ExecOutcome {
    // 1. Usage.
    if args.is_empty() {
        eprintln!("huck: .: usage: . filename [arguments]");
        return ExecOutcome::Continue(2);
    }
    // 2. Depth check (pre-increment).
    if shell.source_depth >= 64 {
        eprintln!("huck: .: maximum source depth (64) exceeded");
        return ExecOutcome::Continue(1);
    }
    // 3. Path resolution.
    let filename = &args[0];
    let path = match resolve_source_path(filename, shell) {
        Some(p) => p,
        None => {
            eprintln!("huck: .: {filename}: file not found");
            return ExecOutcome::Continue(1);
        }
    };
    // 4. Read file.
    let contents = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("huck: .: {}: {e}", path.display());
            return ExecOutcome::Continue(1);
        }
    };
    // 5. Save positional iff extra args provided.
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
        // Use the continuation classifier to defer multi-line constructs.
        match classify(&buf) {
            Completeness::Incomplete(_) => continue,
            Completeness::Error => { /* fall through to parse for error reporting */ }
            Completeness::Complete => { /* fall through */ }
        }
        let tokens = match crate::lexer::tokenize(&buf) {
            Ok(t) if t.is_empty() => { buf.clear(); continue; }
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
                        // `return` at top level of sourced file:
                        // early-exit with N.
                        return ExecOutcome::Continue(n);
                    }
                    ExecOutcome::LoopBreak | ExecOutcome::LoopContinue => {
                        // `break`/`continue` outside a loop at source's top
                        // level: bash silently treats as no-op + status 0.
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

### Dispatch

In `src/builtins.rs:46-66`, the `run_builtin` match block. Add two
arms (both pointing to the same builtin):

```rust
        "." | "source" => builtin_source(args, shell),
```

Place naturally — e.g. after `"trap"` or near the v50 `"set"`/`"shift"`
arms.

### `BUILTIN_NAMES` + `is_special_builtin`

Add `"."` and `"source"` to `BUILTIN_NAMES`. Extend
`is_special_builtin` to include both:

```rust
pub fn is_special_builtin(name: &str) -> bool {
    matches!(name,
        "." | "break" | "continue" | "exit" | "export" | "return"
        | "set" | "shift" | "source" | "trap" | "unset"
    )
}
```

POSIX classifies `.` as special. Bash also treats `source` as a
synonym in the special-builtin list when the shell is interactive.
huck doesn't distinguish POSIX vs bash mode, so we treat both as
special.

Trim the `is_special_builtin` doc comment's "future additions"
list: drop `.` (now shipped). The comment still mentions
`eval`/`exec`/`:`/`readonly` as future.

## Behavior table

| Input | Behavior |
|---|---|
| `. file` | run `file`'s contents, return last status |
| `source file` | identical to `. file` |
| `. file a b` | positional during run = `[a, b]`; restored on return |
| `. file` (no extra args) | positional inherited from caller, unchanged |
| `. ./script.sh` | literal path (has slash) |
| `. script.sh` | search `$PATH` |
| `. nofile` | "file not found" + status 1 |
| `. /etc/shadow` (unreadable) | "Permission denied" + status 1 |
| `. ""` (empty arg) | "file not found" + status 1 (empty searched in PATH, nothing matches) |
| `. file` containing `return 7` | early-exits, returns 7 |
| `. file` containing `exit 5` | exits shell with 5 |
| 65-level recursion | "maximum source depth (64) exceeded" + status 1 |

## Test plan

### Unit tests in `src/builtins.rs#[cfg(test)] mod source_tests`

5 tests:

1. `source_no_args_returns_usage_status_2`.
2. `source_missing_file_errors_status_1`.
3. `source_depth_limit_errors_status_1` — set
   `shell.source_depth = 64`; call with any (even valid) file →
   error 1 without reading file.
4. `is_builtin_recognises_dot_and_source` — both pass `is_builtin`.
5. `is_special_builtin_includes_dot_and_source`.

### Integration tests in `tests/source_integration.rs`

5 tests using `tempfile`-style on-the-fly `/tmp/huck_v51_*` files
(no new dependency — `std::fs::write` + a random-ish name from
`std::process::id()` + a counter):

1. `source_runs_file_contents` — write file `echo HELLO`; script:
   `source <tmpfile>\nexit\n`; stdout contains HELLO.
2. `source_passes_extra_args_as_positional` — file content
   `echo "$1 $2"`; script: `source <tmpfile> A B\nexit\n`; stdout
   contains `A B`.
3. `source_return_early_exits` — file content
   `echo BEFORE\nreturn 0\necho SKIP`; script: `source <tmpfile>\nexit\n`;
   stdout contains BEFORE but NOT SKIP.
4. `source_via_dot_alias` — same as test 1 but using `. <tmpfile>`.
5. `source_path_lookup` — write file in `/tmp` with random name;
   prepend `/tmp` to PATH; source by bare name; verify contents
   run.

Test helper:

```rust
fn write_tmp(content: &str) -> std::path::PathBuf {
    use std::time::{SystemTime, UNIX_EPOCH};
    let pid = std::process::id();
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().subsec_nanos();
    let path = std::env::temp_dir().join(format!("huck_v51_{pid}_{nanos}.sh"));
    std::fs::write(&path, content).expect("write tmp file");
    path
}
```

Cleanup: leave files in `/tmp` — they're small, OS sweeps them. If
tests want strict cleanup, wrap in a Drop guard.

### Smoke

`cargo test --all-targets` must pass. PTY flake tolerated.

## Implementation tasks

1. **Foundation + builtin + 5 unit tests**:
   - Add `pub source_depth: u32` to `Shell`; init in `Shell::new`.
   - Make `lex_error_message` and `parse_error_message` in
     `src/shell.rs` `pub(crate)`.
   - Add `builtin_source` + `resolve_source_path` +
     `run_sourced_contents` in `src/builtins.rs`.
   - Add `"."` and `"source"` to `BUILTIN_NAMES`.
   - Add `"." | "source" => builtin_source(args, shell)` to
     `run_builtin` dispatch.
   - Extend `is_special_builtin` to include both.
   - Trim doc comment.
   - Append `mod source_tests` with 5 tests.

2. **Integration tests**: create `tests/source_integration.rs`
   with the 5 scenarios + `write_tmp` helper.

3. **Docs**: new M-66 entry; change-log entry; README v51 row.

Three tasks. TDD per task.

## Acceptance criteria

- All 5 unit tests pass.
- All 5 integration tests pass.
- `cargo test --all-targets` passes (modulo PTY flake).
- `cargo clippy --all-targets -- -D warnings` passes.
- `docs/bash-divergences.md` has the new M-66 entry as
  `[fixed v51]`.
- `. ./script.sh` and `source ./script.sh` run the file.
- `. script.sh` (no slash) searches PATH.
- `return N` inside a sourced file early-exits.
- Depth limit prevents runaway recursion.
- Pre-existing tests still pass after the `pub(crate)` visibility
  changes.
