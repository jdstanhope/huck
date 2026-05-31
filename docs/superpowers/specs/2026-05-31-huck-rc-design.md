# huck v62 — rc file support (M-77)

## Goal

Source `~/.huckrc` on interactive startup so users can persist
aliases, functions, exported variables, PS1 customizations, etc.
Add CLI flags `--rcfile PATH` (override the default) and
`--norc` (skip it entirely). Support `$HUCK_RC` env var as a
lower-precedence override.

After v62:

- Interactive `huck` (no flags) sources `~/.huckrc` if it exists.
- `huck --rcfile /path/to/file` sources that file instead.
  Missing file → error + exit 1 (explicit request, not silent).
- `huck --norc` skips rc loading entirely.
- `HUCK_RC=/path/to/file huck` sources that file. Missing →
  silent skip (matches bash's `BASH_ENV` semantics for the
  default-like case).
- Non-interactive (piped stdin) skips rc loading entirely.
- Errors INSIDE the rc file (a non-existent command, a bad
  alias, etc.) print to stderr but don't crash the shell.
  Matches bash.
- `exit N` inside rc file → shell exits with status N before
  showing any prompt.

New tracked divergence: **M-77: rc file support**.

## Scope decisions (locked via AskUserQuestion)

Tier B: default `~/.huckrc` + `--rcfile`/`--norc` flags +
`HUCK_RC` env override. No login-shell distinction (Tier C
deferred).

## Out of scope (deferred)

- Login-shell concept (`argv[0]` starting with `-`, `--login`
  flag).
- `~/.huck_profile` / fallback chain / `~/.huck_logout`.
- `--noprofile` (would need login-shell concept first).
- `/etc/huckrc` (system-wide rc; rare for a learning shell).
- Other startup options (`-i`, `-c`, etc.) — only `--rcfile`,
  `--norc`, and `--` (end-of-flags) for v62.

## Architecture

### CLI argument plumbing

`src/main.rs` becomes:

```rust
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(shell::run(&args));
}
```

`shell::run` signature widens from `fn run() -> i32` to
`fn run(args: &[String]) -> i32`. All existing call sites in
`huck` are via `main.rs`, so the signature change is safe.

### CLI parsing

New helper in `src/shell.rs`:

```rust
struct CliOptions {
    rcfile_path: Option<std::path::PathBuf>,
    norc: bool,
}

fn parse_cli(args: &[String]) -> Result<CliOptions, String> {
    let mut opts = CliOptions { rcfile_path: None, norc: false };
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--norc" => { opts.norc = true; i += 1; }
            "--rcfile" => {
                i += 1;
                if i >= args.len() {
                    return Err("--rcfile: requires an argument".to_string());
                }
                opts.rcfile_path = Some(std::path::PathBuf::from(&args[i]));
                i += 1;
            }
            "--" => { i += 1; break; }
            s if s.starts_with("--rcfile=") => {
                opts.rcfile_path = Some(
                    std::path::PathBuf::from(&s["--rcfile=".len()..]),
                );
                i += 1;
            }
            unknown => return Err(format!("unrecognized option: {unknown}")),
        }
    }
    Ok(opts)
}
```

Behavior:
- `--rcfile PATH` (separate): consumes next arg as the path.
  Missing path arg → usage error.
- `--rcfile=PATH` (joined): inline form.
- `--norc`: idempotent boolean.
- `--`: ends flag parsing (no positional args supported in v62;
  any trailing args after `--` are an error for now to keep the
  surface tight).
- Unknown flag → usage error + exit 2.

Bare positional args (e.g. `huck script.sh`) are NOT supported
in v62 — bash uses these for script-execution mode. Deferred.
If positional args are present after parsing, emit an error.

### rc file loader

```rust
fn maybe_source_rc_file(shell: &mut Shell, opts: &CliOptions) -> Option<i32> {
    if opts.norc { return None; }
    if !shell.is_interactive { return None; }

    // Resolution precedence: explicit --rcfile > $HUCK_RC > ~/.huckrc.
    // Missing-file behavior differs:
    //  - --rcfile: missing → status 1 error (explicit user request).
    //  - HUCK_RC / default ~/.huckrc: missing → silent skip.
    let (path, explicit) = match &opts.rcfile_path {
        Some(p) => (p.clone(), true),
        None => {
            let from_env = shell
                .lookup_var("HUCK_RC")
                .or_else(|| std::env::var("HUCK_RC").ok())
                .filter(|s| !s.is_empty())
                .map(std::path::PathBuf::from);
            match from_env {
                Some(p) => (p, false),
                None => match default_rc_path(shell) {
                    Some(p) => (p, false),
                    None => return None,
                },
            }
        }
    };
    if !path.exists() {
        if explicit {
            eprintln!("huck: {}: No such file or directory", path.display());
            return Some(1);
        }
        return None;
    }
    let contents = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("huck: {}: {}", path.display(), e);
            return Some(1);
        }
    };
    match crate::builtins::run_sourced_contents(&contents, &path, shell) {
        crate::executor::ExecOutcome::Exit(code) => Some(code),
        crate::executor::ExecOutcome::Continue(status) => {
            shell.set_last_status(status);
            None
        }
        _ => None,
    }
}

fn default_rc_path(shell: &Shell) -> Option<std::path::PathBuf> {
    let home = shell
        .lookup_var("HOME")
        .or_else(|| std::env::var("HOME").ok())
        .filter(|s| !s.is_empty())?;
    Some(std::path::PathBuf::from(home).join(".huckrc"))
}
```

`run_sourced_contents` (v51) is currently private (`fn`) in
`src/builtins.rs`. Promote to `pub(crate) fn` so `src/shell.rs`
can call it.

### Wire-in in `shell::run`

After `Shell::new()`, `history.load()`, and signal-handler
installation (lines ~50-58 of `src/shell.rs`), before the main
REPL loop:

```rust
if let Some(exit_code) = maybe_source_rc_file(&mut shell, &opts) {
    crate::traps::fire_exit_trap(&mut shell);
    shell.hangup_jobs();
    shell.history.save();
    return exit_code;
}
```

This mirrors the existing Exit-cleanup path for user commands.

### CLI errors

At the very top of `shell::run`, before any other setup:

```rust
let opts = match parse_cli(args) {
    Ok(o) => o,
    Err(e) => {
        eprintln!("huck: {e}");
        return 2;
    }
};
```

Returning 2 matches bash's "usage error" convention.

## Behavior table

| Invocation | Behavior |
|---|---|
| `huck` (interactive, no `~/.huckrc`) | No-op, normal startup |
| `huck` (interactive, `~/.huckrc` exists) | Sources it, then prompt |
| `huck` (piped stdin) | rc loading skipped entirely |
| `huck --norc` (interactive, with `~/.huckrc`) | rc skipped |
| `huck --rcfile /path/to/file` (file exists) | Sources `/path/to/file` |
| `huck --rcfile /path/to/missing` | stderr "No such file" + exit 1 |
| `huck --rcfile` (no arg) | usage error + exit 2 |
| `huck --rcfile=/path/to/file` | same as `--rcfile /path/to/file` |
| `HUCK_RC=/x huck` (interactive, file exists) | Sources `/x` |
| `HUCK_RC=/missing huck` | silent skip (no error) |
| `HUCK_RC= huck` (empty) | falls back to default `~/.huckrc` |
| `huck --rcfile /x --norc` | `--norc` wins (skip; --rcfile irrelevant) |
| Multiple `--rcfile X --rcfile Y` | last wins (Y) |
| rc file has `exit 7` | huck exits 7 before any prompt |
| rc file has syntax error | stderr error + continues |
| `huck --bogus` | usage error + exit 2 |
| `huck -- positional` | error: positional args not supported |

## Test plan

### Unit tests in `src/shell.rs::rc_tests` (~10 tests)

CLI parser (5):
1. `parse_cli_empty` — empty args → defaults (no rcfile, norc=false).
2. `parse_cli_norc` — `["--norc"]` → norc=true.
3. `parse_cli_rcfile_separate` — `["--rcfile", "/x"]` → rcfile_path=Some(/x).
4. `parse_cli_rcfile_joined` — `["--rcfile=/x"]` → rcfile_path=Some(/x).
5. `parse_cli_unknown_errors` — `["--bogus"]` → Err.
6. `parse_cli_rcfile_no_arg_errors` — `["--rcfile"]` → Err.

rc loader (5):
7. `rc_skips_when_norc` — interactive shell + write a tempfile + opts.norc=true; returns None; shell state unchanged.
8. `rc_skips_when_non_interactive` — non-interactive shell + tempfile in opts; returns None.
9. `rc_sources_explicit_path` — tempfile with `export TEST_VAR=hello` and opts.rcfile_path=Some(tempfile); returns None; shell.lookup_var("TEST_VAR") == Some("hello").
10. `rc_explicit_missing_errors` — opts.rcfile_path=Some("/no/such/file"); returns Some(1).
11. `rc_explicit_exit_propagates` — tempfile with `exit 42`; returns Some(42).
12. `rc_default_missing_silent` — opts.rcfile_path=None and HOME pointing somewhere with no .huckrc; returns None silently.

(That's 12 tests total — slightly more than the "~8" I quoted. Worth it for coverage.)

### Integration tests

Skipped. rc-loading only fires in interactive mode, and the integration test harness uses piped stdin. The unit tests cover the wire correctly via direct `maybe_source_rc_file` calls with a tempfile.

### Smoke

`cargo test --all-targets` green (PTY flake tolerated).

## Implementation tasks

1. **CLI parsing + rc loader + 12 unit tests** — modify
   `src/main.rs` (pass argv); modify `src/shell.rs::run`
   signature and add wire-in; add `parse_cli` and
   `maybe_source_rc_file` helpers; promote
   `run_sourced_contents` in `src/builtins.rs` to
   `pub(crate)`; add `mod rc_tests`.

2. **Docs** — new M-77 entry; change-log; README v62 row.

## Acceptance criteria

- 12 unit tests pass.
- `cargo test --all-targets` green.
- `cargo clippy --all-targets -- -D warnings` clean.
- Bare `huck` (interactive) sources `~/.huckrc` when it exists.
- `--rcfile` overrides the default; missing file errors with
  exit 1.
- `--norc` skips rc loading.
- `HUCK_RC` env var works; missing file is silent.
- Non-interactive runs skip rc loading entirely.
- `exit N` inside rc file exits the shell with status N.
- M-77 doc entry added.
