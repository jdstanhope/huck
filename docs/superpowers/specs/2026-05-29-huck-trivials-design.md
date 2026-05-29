# huck v53 — trivials cluster (M-68, M-69, M-70)

## Goal

Ship four small but essential builtins in a single iteration:

- `:` — POSIX null command. Always exit 0 (after argument expansion).
- `true` — exit 0.
- `false` — exit 1.
- `command -v NAME` / `command -V NAME` — print how NAME would be
  resolved (alias / function / builtin / keyword / external file)
  or report not-found.

These are all bash-compat and (except `command`) have near-zero
implementation complexity. After v53:

- `: ${VAR:=default}` works (the side-effect of the param-expansion
  default-assignment trigger fires; status is 0).
- `while :` infinite loops are spellable.
- `true && X`, `false || X` work as expected.
- `command -v cmd` lets scripts test for availability before use.
- `command -V cmd` gives human-readable resolution text.

Three new tracked divergences: **M-68: `:`**, **M-69: `true`/`false`**,
**M-70: `command -v` / `-V`**.

## Scope decisions (locked)

1. **`readonly`** — split to v54. Touches the `Variable` model
   (needs a `readonly` flag) and ripples through
   `set`/`unset`/`export`/`local`. Big enough for its own iteration.
2. **`command` flags**: `-v` and `-V` only. Bare-form
   `command cmd args` (run cmd bypassing function/alias lookup) is
   deferred. `-p` (use POSIX default PATH) is deferred.
3. **`:` arg expansion**: huck's executor already expands a
   simple-command's words before dispatching to the builtin. So
   `: ${VAR:=default}` triggers param-expansion's side effect
   naturally, and `builtin_colon` just returns
   `ExecOutcome::Continue(0)`. No changes to the arg-eval path.
4. **`:` is a POSIX special builtin** — add to `is_special_builtin`.
   `true`/`false`/`command` are regular.

## Out of scope (deferred)

- `readonly` (v54).
- `command` bare-form and `-p` flag.
- Shell-keyword recognition for *arbitrary* keywords in
  `command -v` — we hardcode a small set covering the keywords
  huck actually parses (see "Keyword set" below). If a user passes
  a keyword huck doesn't parse, `command -v` will treat it as
  not-found, which is acceptable: huck genuinely doesn't recognize
  it.

## Architecture

Four new builtin functions in `src/builtins.rs`. Three are
one-liners; `command` has a flag parser, a resolver, and per-arg
output logic.

### `:` / `true` / `false`

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

All three ignore their args. Bash does the same.

### `command -v` / `-V`

```rust
fn builtin_command(
    args: &[String],
    shell: &mut Shell,
    sink: &mut StdoutSink,
) -> ExecOutcome {
    // Parse flags from the left.
    let mut concise = false;     // -v
    let mut verbose = false;     // -V
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

    // Bare form (no -v/-V) is deferred — error.
    if !concise && !verbose {
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
                    let _ = writeln!(sink, "alias {name}='{value}'");
                } else {
                    let _ = writeln!(sink, "{name} is aliased to `{value}'");
                }
            }
            CommandResolution::Function => {
                if concise {
                    let _ = writeln!(sink, "{name}");
                } else {
                    let _ = writeln!(sink, "{name} is a function");
                }
            }
            CommandResolution::Builtin => {
                if concise {
                    let _ = writeln!(sink, "{name}");
                } else {
                    let _ = writeln!(sink, "{name} is a shell builtin");
                }
            }
            CommandResolution::Keyword => {
                if concise {
                    let _ = writeln!(sink, "{name}");
                } else {
                    let _ = writeln!(sink, "{name} is a shell keyword");
                }
            }
            CommandResolution::File(path) => {
                if concise {
                    let _ = writeln!(sink, "{}", path.display());
                } else {
                    let _ = writeln!(sink, "{name} is {}", path.display());
                }
            }
            CommandResolution::NotFound => {
                any_not_found = true;
                if verbose {
                    eprintln!("huck: command: {name}: not found");
                }
                // -v: silent on stderr; non-zero exit is the only signal
            }
        }
    }
    ExecOutcome::Continue(if any_not_found { 1 } else { 0 })
}
```

Resolver:

```rust
enum CommandResolution {
    Alias(String),
    Function,
    Builtin,
    Keyword,
    File(std::path::PathBuf),
    NotFound,
}

fn resolve_command_name(name: &str, shell: &Shell) -> CommandResolution {
    // Resolution order matches bash:
    //   alias > function > builtin > keyword > $PATH > not-found.
    // (Keywords come after builtins because, in interactive use, builtins
    // overriding keywords doesn't really happen — but bash's `type` does
    // report builtin before keyword for names like `[`.)
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

fn search_path_for(name: &str, shell: &Shell) -> Option<std::path::PathBuf> {
    // If `name` contains `/`, bash reports the literal path if it's
    // an executable file. Otherwise, walk $PATH.
    if name.contains('/') {
        let p = std::path::PathBuf::from(name);
        if is_executable_file(&p) { Some(p) } else { None }
    } else {
        let path_val = shell.lookup_var("PATH").unwrap_or_default();
        for segment in path_val.split(':') {
            if segment.is_empty() { continue; }
            let candidate = std::path::Path::new(segment).join(name);
            if is_executable_file(&candidate) {
                return Some(candidate);
            }
        }
        None
    }
}

fn is_executable_file(p: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(p) {
        Ok(md) => {
            md.is_file() && (md.permissions().mode() & 0o111 != 0)
        }
        Err(_) => false,
    }
}
```

Note: `Shell::aliases` is already public on the struct (used by
v48). `Shell::functions` is `pub` per `shell_state.rs:28`.

### Dispatch + BUILTIN_NAMES + is_special_builtin

Append four entries to `BUILTIN_NAMES`:

```rust
pub const BUILTIN_NAMES: &[&str] = &[
    // ... existing 27 ...
    ":", "true", "false", "command",
];
```

Add `":"` to `is_special_builtin`'s matched set:

```rust
matches!(name, "break" | "continue" | "return" | "exit" | "trap"
    | "set" | "shift" | "." | "source" | "export" | "unset" | ":")
```

(Verify the actual existing form — adapt to it.)

Add dispatch arms in `run_builtin`:

```rust
":" => builtin_colon(args, shell),
"true" => builtin_true(args, shell),
"false" => builtin_false(args, shell),
"command" => builtin_command(args, shell, sink),
```

Position them near other zero-arg / no-side-effect builtins.

## Behavior table

| Input | Behavior |
|---|---|
| `:` | exit 0 |
| `: anything ${X:=v}` | args expanded (X set if unset), exit 0 |
| `true` | exit 0 |
| `true ignored args` | exit 0 |
| `false` | exit 1 |
| `false ignored args` | exit 1 |
| `command` (no args) | exit 0 (no-op; matches bash `command` with no name) |
| `command cmd args` (no -v/-V) | error + exit 2 (bare form deferred) |
| `command -v echo` | "echo" + exit 0 |
| `command -v notfound` | (silent) + exit 1 |
| `command -v ls` | "/usr/bin/ls" (or wherever PATH finds it) + exit 0 |
| `command -v ./foo` (executable) | "./foo" + exit 0 |
| `command -V echo` | "echo is a shell builtin" + exit 0 |
| `command -V if` | "if is a shell keyword" + exit 0 |
| `command -V notfound` | stderr "huck: command: notfound: not found" + exit 1 |
| `command -v alias-name` (alias `foo=bar`) | "alias foo='bar'" + exit 0 |
| `command -v func-name` (function defined) | "func-name" + exit 0 |
| `command -v cmd1 cmd2` (cmd1 found, cmd2 missing) | "cmd1" (line) + exit 1 |
| `command -- -v` | (no -v/-V parsed before --, bare form) → error + exit 2 |
| `command -X` | "huck: command: -X: invalid option" + exit 2 |

## Test plan

### Unit tests in `src/builtins.rs`

Append four small `#[cfg(test)] mod`s (or one combined module).
Total: 12 unit tests.

**`mod colon_tests`** (2):
1. `colon_exits_zero` — `builtin_colon(&[], ...)` → `Continue(0)`.
2. `colon_with_args_exits_zero` — args ignored.

**`mod true_false_tests`** (3):
3. `true_exits_zero`.
4. `false_exits_one`.
5. `true_and_false_ignore_args`.

**`mod command_tests`** (7):
6. `command_no_args_exits_zero`.
7. `command_bare_form_errors` — `command echo hi` → status 2 + stderr.
8. `command_dash_v_builtin_concise` — `command -v echo` stdout "echo".
9. `command_dash_v_notfound_silent_status_1` — `command -v __no_such_cmd__` → no stdout, status 1.
10. `command_dash_V_builtin_verbose` — stdout "echo is a shell builtin".
11. `command_dash_V_notfound_stderr_status_1` — stderr "huck: command: __no_such_cmd__: not found" + status 1.
12. `command_dash_v_function` — define function `foo`, then `command -v foo` → "foo", status 0.

(Aliases and PATH lookup are exercised by the integration suite —
unit tests for them require either mocking PATH or setting up files
on disk, which is integration-test territory.)

### Integration tests in `tests/trivials_integration.rs`

8 binary-driven scenarios:

1. `colon_is_no_op` — `: anything\necho ok\nexit\n` → stdout has "ok".
2. `colon_triggers_param_default_assignment` — `: ${X:=hello}\necho "$X"\nexit\n` → stdout has "hello".
3. `true_in_conditional` — `if true; then echo Y; fi\nexit\n` → "Y".
4. `false_in_conditional` — `if false; then echo Y; else echo N; fi\nexit\n` → "N".
5. `command_v_finds_builtin` — `command -v echo\nexit\n` → stdout has "echo", exit 0.
6. `command_v_missing_status_1` — `command -v __no_such_cmd_xyzzy__\nrc=$?\necho rc=$rc\nexit\n` → "rc=1".
7. `command_v_finds_path_binary` — `command -v sh\nexit\n` → stdout has a "/" (a real path).
8. `command_V_keyword` — `command -V if\nexit\n` → stdout has "if is a shell keyword".

### Smoke

`cargo test --all-targets` passes. PTY flake in
`pty_interactive.rs::pty_compound_stage_pipeline_stops_and_resumes`
tolerated as usual.

## Implementation tasks

1. **4 builtins + 12 unit tests** — touch `src/builtins.rs` only.
   Adds `builtin_colon`, `builtin_true`, `builtin_false`,
   `builtin_command` + `resolve_command_name` + `is_shell_keyword`
   + `search_path_for` + `is_executable_file` + the
   `CommandResolution` enum. Updates `BUILTIN_NAMES` (+4),
   `is_special_builtin` (+`:`), and `run_builtin` dispatch (+4).

2. **Integration tests** — create `tests/trivials_integration.rs`
   with the 8 scenarios above.

3. **Docs** — add M-68 (`:`), M-69 (`true`/`false`), M-70
   (`command -v`/`-V`) entries to `docs/bash-divergences.md` as
   `[fixed v53]`; change-log entry; README v53 row.

## Acceptance criteria

- 12 new unit tests pass.
- 8 new integration tests pass.
- `cargo test --all-targets` green (modulo PTY flake).
- `cargo clippy --all-targets -- -D warnings` clean.
- `:` is special; `true`/`false`/`command` are regular.
- `: ${X:=v}` sets X (existing param-expansion behavior unchanged;
  this just confirms the trigger fires).
- `command -v` exits non-zero (1) when ANY name is not found,
  0 only when all are resolved.
- `command -V` writes the not-found error to stderr; `command -v`
  is silent on stderr.
- All existing tests still pass.
