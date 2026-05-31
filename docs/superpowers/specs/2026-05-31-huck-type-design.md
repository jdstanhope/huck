# huck v58 — `type` builtin (M-75)

## Goal

Add bash's `type` builtin for command-resolution introspection.
Reuses v53's `resolve_command_name` infrastructure; adds a
multi-match resolver for `-a`, a parameterized resolver for `-f`,
and new output-format variants for `-t`, `-p`, `-P`.

After v58:

- `type NAME` — verbose form (same text as `command -V NAME`).
- `type -t NAME` — type word: `alias` / `keyword` / `function` / `builtin` / `file`. Empty stdout + status 1 if not found.
- `type -a NAME` — ALL matches in resolution order, one per line. Lists ALL PATH hits, not just first.
- `type -p NAME` — path only (only outputs for File matches; empty for alias/function/builtin/keyword; empty for not-found). Status 1 if not found.
- `type -P NAME` — like `-p` but force PATH search (skip alias/function/builtin/keyword).
- `type -f NAME` — skip function lookup (treat any function with that name as not-set).
- Flag clustering: `-tp`, `-at`, `-af`, etc.
- Multi-name: `type cmd1 cmd2 cmd3`. Exit 1 if ANY name is not found.

New tracked divergence: **M-75: `type`**.

## Scope decisions (locked via AskUserQuestion)

Full bash form including `-f`.

## Out of scope (deferred)

- Function-body printing in default and `-a` output. Bash's
  `type` (without `-t`) prints the actual function body. huck
  prints `NAME is a function` (matches `command -V`). Pretty-
  printing AST is a separate concern.
- `-p` precedence over `-t` and vice versa when both supplied:
  pick one (we choose `-t` wins). Bash behavior is ambiguous /
  last-flag-wins; matching it byte-for-byte is not worth the
  complexity.

## Architecture

All in `src/builtins.rs`. Three new helpers + the main builtin.

### 1. Multi-match resolver

```rust
/// Returns ALL matches for `name` in bash's resolution order:
/// (1) alias if any, (2) function if any AND !skip_func, (3) builtin
/// if any, (4) keyword if any, (5) every PATH entry that contains
/// an executable `name`. Empty Vec = not found.
fn resolve_command_name_all(
    name: &str,
    shell: &Shell,
    skip_func: bool,
) -> Vec<CommandResolution> {
    let mut out: Vec<CommandResolution> = Vec::new();
    if let Some(v) = shell.aliases.get(name) {
        out.push(CommandResolution::Alias(v.clone()));
    }
    if !skip_func && shell.functions.contains_key(name) {
        out.push(CommandResolution::Function);
    }
    if is_builtin(name) {
        out.push(CommandResolution::Builtin);
    }
    if is_shell_keyword(name) {
        out.push(CommandResolution::Keyword);
    }
    for p in search_path_all(name, shell) {
        out.push(CommandResolution::File(p));
    }
    out
}
```

### 2. Single-match resolver with skip_func

```rust
/// Like v53's `resolve_command_name` but skips functions when
/// `skip_func` is true. (v53's existing signature is unchanged;
/// this is a new sibling that takes the flag.)
fn resolve_command_name_with(
    name: &str,
    shell: &Shell,
    skip_func: bool,
) -> CommandResolution {
    if let Some(v) = shell.aliases.get(name) {
        return CommandResolution::Alias(v.clone());
    }
    if !skip_func && shell.functions.contains_key(name) {
        return CommandResolution::Function;
    }
    if is_builtin(name) {
        return CommandResolution::Builtin;
    }
    if is_shell_keyword(name) {
        return CommandResolution::Keyword;
    }
    if let Some(p) = search_path_for(name, shell) {
        return CommandResolution::File(p);
    }
    CommandResolution::NotFound
}
```

### 3. Multi-PATH search

```rust
fn search_path_all(name: &str, shell: &Shell) -> Vec<std::path::PathBuf> {
    if name.contains('/') {
        let p = std::path::PathBuf::from(name);
        return if is_executable_file(&p) { vec![p] } else { vec![] };
    }
    let path_val = shell.lookup_var("PATH").unwrap_or_default();
    let mut out: Vec<std::path::PathBuf> = Vec::new();
    for segment in path_val.split(':') {
        if segment.is_empty() {
            continue;
        }
        let candidate = std::path::Path::new(segment).join(name);
        if is_executable_file(&candidate) {
            out.push(candidate);
        }
    }
    out
}
```

### 4. `builtin_type`

```rust
fn builtin_type(
    args: &[String],
    out: &mut dyn std::io::Write,
    shell: &mut Shell,
) -> ExecOutcome {
    let mut all = false;
    let mut type_only = false;
    let mut path_only = false;
    let mut force_path = false;
    let mut skip_func = false;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" { i += 1; break; }
        if !arg.starts_with('-') || arg.len() < 2 { break; }
        let bytes = arg.as_bytes();
        for &c in &bytes[1..] {
            match c {
                b'a' => all = true,
                b't' => type_only = true,
                b'p' => path_only = true,
                b'P' => { path_only = true; force_path = true; }
                b'f' => skip_func = true,
                other => {
                    eprintln!("huck: type: -{}: invalid option", other as char);
                    return ExecOutcome::Continue(2);
                }
            }
        }
        i += 1;
    }
    let names = &args[i..];
    if names.is_empty() {
        // Bash: bare `type` with no names succeeds (no output, exit 0).
        return ExecOutcome::Continue(0);
    }

    let mut exit = 0;
    for name in names {
        let resolutions: Vec<CommandResolution> = if force_path {
            search_path_all(name, shell)
                .into_iter()
                .map(CommandResolution::File)
                .collect()
        } else if all {
            resolve_command_name_all(name, shell, skip_func)
        } else {
            match resolve_command_name_with(name, shell, skip_func) {
                CommandResolution::NotFound => Vec::new(),
                other => vec![other],
            }
        };

        if resolutions.is_empty() {
            // Not found.
            if !type_only && !path_only {
                eprintln!("huck: type: {name}: not found");
            }
            exit = 1;
            continue;
        }

        for res in &resolutions {
            emit_type_entry(name, res, type_only, path_only, out);
        }
    }
    ExecOutcome::Continue(exit)
}
```

### 5. Output formatter

```rust
fn emit_type_entry(
    name: &str,
    res: &CommandResolution,
    type_only: bool,
    path_only: bool,
    out: &mut dyn std::io::Write,
) {
    // -t takes precedence over -p when both set.
    if type_only {
        let word: &str = match res {
            CommandResolution::Alias(_) => "alias",
            CommandResolution::Function => "function",
            CommandResolution::Builtin => "builtin",
            CommandResolution::Keyword => "keyword",
            CommandResolution::File(_) => "file",
            CommandResolution::NotFound => return,
        };
        let _ = writeln!(out, "{word}");
        return;
    }
    if path_only {
        if let CommandResolution::File(p) = res {
            let _ = writeln!(out, "{}", p.display());
        }
        // Non-file matches: silent.
        return;
    }
    // Default verbose.
    match res {
        CommandResolution::Alias(value) => {
            let _ = writeln!(out, "{name} is aliased to `{value}'");
        }
        CommandResolution::Function => {
            let _ = writeln!(out, "{name} is a function");
        }
        CommandResolution::Builtin => {
            let _ = writeln!(out, "{name} is a shell builtin");
        }
        CommandResolution::Keyword => {
            let _ = writeln!(out, "{name} is a shell keyword");
        }
        CommandResolution::File(p) => {
            let _ = writeln!(out, "{name} is {}", p.display());
        }
        CommandResolution::NotFound => {}
    }
}
```

### 6. Dispatch + `BUILTIN_NAMES`

- `"type"` added to `BUILTIN_NAMES`.
- NOT in `is_special_builtin` (bash-specific, regular).
- `"type" => builtin_type(args, out, shell)` dispatch arm.

## Behavior table

| Input | Output / behavior |
|---|---|
| `type echo` | `echo is a shell builtin` + exit 0 |
| `type if` | `if is a shell keyword` + exit 0 |
| `type ls` (in PATH) | `ls is /usr/bin/ls` + exit 0 |
| `type nope` | stderr `huck: type: nope: not found` + exit 1 |
| `type -t echo` | `builtin\n` + exit 0 |
| `type -t if` | `keyword\n` + exit 0 |
| `type -t ls` | `file\n` + exit 0 |
| `type -t nope` | (empty stdout, no stderr) + exit 1 |
| `type -p echo` | (empty — not a file) + exit 0 |
| `type -p ls` | `/usr/bin/ls\n` + exit 0 |
| `type -p nope` | (empty stdout, no stderr) + exit 1 |
| `type -P echo` | `/usr/bin/echo\n` (if in PATH, force-PATH overrides builtin precedence) |
| `type -a ls` (alias `ls=foo` + /usr/bin/ls) | `ls is aliased to \`foo'\nls is /usr/bin/ls\n` |
| `type -f f` (f is a function in shell) | stderr `huck: type: f: not found` + exit 1 (function skipped) |
| `type cmd1 cmd2` (cmd1 found, cmd2 missing) | cmd1 line on stdout, cmd2 error on stderr, exit 1 |
| `type` (no args) | exit 0 |
| `type -X foo` | stderr `huck: type: -X: invalid option` + exit 2 |

## Test plan

### Unit tests in `src/builtins.rs::mod type_tests` (15 tests)

1. `type_default_builtin` — `type echo` → `echo is a shell builtin\n`, exit 0.
2. `type_default_keyword` — `type if` → `if is a shell keyword\n`, exit 0.
3. `type_default_function` — define function `f`; `type f` → `f is a function\n`, exit 0.
4. `type_default_alias` — alias `ll=ls`; `type ll` → `ll is aliased to \`ls'\n`, exit 0.
5. `type_default_not_found` — `type __xyz__` → stdout empty, exit 1.
6. `type_t_builtin` — `type -t echo` → `builtin\n`.
7. `type_t_keyword` — `type -t if` → `keyword\n`.
8. `type_t_function` — function `f`; `type -t f` → `function\n`.
9. `type_t_not_found_silent` — `type -t __xyz__` → empty stdout, exit 1, no stderr.
10. `type_p_builtin_silent` — `type -p echo` → empty stdout, exit 0 (builtin is "found" but -p has nothing to print).
11. `type_a_multiple_matches` — function `f` + builtin `echo` would not normally overlap; instead test alias `echo=foo` + builtin `echo` → `-a` shows both.
12. `type_f_skips_function` — function `f`; `type -f f` → exit 1 (not found because function skipped).
13. `type_P_force_path` — `type -P echo` → outputs PATH match (a real shell may not have `/usr/bin/echo`; verify the test name's existence carefully — use `sh` instead since it's universal).
14. `type_multi_name_first_found_second_missing` — `type echo __xyz__` → builtin line for echo + stderr "not found" for xyz, exit 1.
15. `type_invalid_option_status_2` — `type -X echo` → status 2 + stderr.

### Integration tests in `tests/type_integration.rs` (8 tests)

1. `type_default_for_builtin` — `type echo` → stdout has "echo is a shell builtin".
2. `type_t_for_keyword` — `type -t if` → stdout has "keyword".
3. `type_p_for_builtin_is_empty` — `type -p echo` → stdout empty, exit 0.
4. `type_p_for_file_returns_path` — `type -p sh` → stdout contains "/sh".
5. `type_not_found_exit_1` — `type __no_such_command_xyzzy__\nrc=$?\necho rc=$rc\nexit\n` → "rc=1" + stderr.
6. `type_a_alias_then_path` — `alias ls=foo; type -a ls` → stdout has both alias line and at least one path line.
7. `type_P_force_path_for_builtin_finds_file` — `type -P sh` → stdout contains "/sh" (forces PATH search).
8. `type_f_skips_function` — `f() { :; }; type -f f\nrc=$?\necho rc=$rc\nexit\n` → "rc=1" + stderr "not found".

### Smoke

`cargo test --all-targets` green (PTY flake tolerated).

## Implementation tasks

1. **builtin_type + helpers + 15 unit tests** — `src/builtins.rs`
   only. Adds `search_path_all`, `resolve_command_name_with`,
   `resolve_command_name_all`, `builtin_type`, `emit_type_entry`,
   `"type"` to `BUILTIN_NAMES`, dispatch arm, `mod type_tests`.

2. **Integration tests** — `tests/type_integration.rs` with 8
   scenarios.

3. **Docs** — M-75 entry; change-log; README v58 row.

## Acceptance criteria

- 15 unit tests pass.
- 8 integration tests pass.
- `cargo test --all-targets` green.
- `cargo clippy --all-targets -- -D warnings` clean.
- `type` is regular (NOT in `is_special_builtin`).
- All five flags work: -a, -t, -f, -p, -P.
- `-t` precedes `-p` if both set.
- `-P` forces PATH search regardless of normal precedence.
- `-f` makes function-lookup invisible.
- `-a` lists ALL matches including multiple PATH entries.
- Multi-name: exit 1 if any not found; processes all names.
- All existing tests still pass.
