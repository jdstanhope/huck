# huck v59 — `hash` builtin (M-34 partial)

## Goal

Add bash's `hash` builtin surface — table maintenance, listing,
explicit add/delete, and re-input-form output. Hit counts are
stored but stay at 0 (no executor integration in v59).

After v59:

- `hash NAME [NAME ...]` — PATH-search each NAME; add to the
  hash table. Error per name if not found.
- `hash` (no args) — list the hash table; "hash table empty" to
  stdout when empty.
- `hash -r` — clear the table.
- `hash -d NAME ...` — delete specific entries.
- `hash -p PATH NAME` — associate NAME with PATH directly (no
  PATH search). PATH need not exist.
- `hash -l` — list in re-input form: `builtin hash -p PATH NAME`.
- `hash -t NAME [NAME ...]` — print path(s). Multiple names get
  prefixed with `NAME\t`. Status 1 if any not hashed.

Updates the existing **M-34** tracked divergence from
`[deferred]` to `[fixed v59 partial]` (the executor integration
remains deferred).

## Scope decisions (locked via AskUserQuestion)

**Builtin surface only**. No executor integration: real
command-lookup PATH walks do NOT populate the hash, and the
executor does NOT consult the hash to short-circuit PATH walks.
Hit counts stay at 0. This matches what M-34 calls "the API
surface" — scripts that explicitly call `hash` work; the
performance optimization stays deferred.

## Out of scope (deferred)

- Executor integration (auto-populate from `run_exec_single`'s
  PATH lookup; consult the hash before searching). M-34 stays
  `[fixed v59 partial]`.
- Real hit-count tracking. Hit counts are stored as `u32` but
  never incremented. The listing format still shows the column
  for bash compatibility.

## Architecture

### 1. `Shell.command_hash` field (`src/shell_state.rs`)

```rust
/// Command-name hash table populated by the `hash` builtin.
/// Maps a bare name (no `/`) to its absolute path plus a hit
/// counter (always 0 in v59 — no executor integration yet, so
/// hits are never bumped; the field exists for the bash-compat
/// listing format).
pub command_hash: std::collections::HashMap<String, (std::path::PathBuf, u32)>,
```

Init to empty in `Shell::new`.

### 2. `builtin_hash` (`src/builtins.rs`)

Reuses v53's `search_path_for` for PATH-lookup. The builtin
parses flag clusters and dispatches on mode.

```rust
fn builtin_hash(
    args: &[String],
    out: &mut dyn std::io::Write,
    shell: &mut Shell,
) -> ExecOutcome {
    // Mode-selector flags. Last one set wins (deterministic
    // priority: reset > delete > set_path > list > type_only >
    // default).
    let mut reset = false;
    let mut delete = false;
    let mut set_path = false;
    let mut list = false;
    let mut type_only = false;
    let mut explicit_path: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" { i += 1; break; }
        if !arg.starts_with('-') || arg.len() < 2 { break; }
        // Walk the cluster. -p takes a value (rest-of-arg OR
        // next arg).
        let bytes = arg.as_bytes();
        let mut j = 1;
        while j < bytes.len() {
            match bytes[j] {
                b'r' => reset = true,
                b'd' => delete = true,
                b'l' => list = true,
                b't' => type_only = true,
                b'p' => {
                    set_path = true;
                    // Consume the path value.
                    if j + 1 < bytes.len() {
                        // -p inline: "-pPATH"
                        explicit_path = Some(
                            String::from_utf8_lossy(&bytes[j + 1..]).into_owned()
                        );
                        j = bytes.len(); // skip rest
                        break;
                    } else {
                        // -p separate: next arg
                        i += 1;
                        if i >= args.len() {
                            eprintln!("huck: hash: -p: option requires an argument");
                            return ExecOutcome::Continue(2);
                        }
                        explicit_path = Some(args[i].clone());
                        break;
                    }
                }
                c => {
                    eprintln!("huck: hash: -{}: invalid option", c as char);
                    return ExecOutcome::Continue(2);
                }
            }
            j += 1;
        }
        i += 1;
    }
    let names = &args[i..];

    if reset {
        shell.command_hash.clear();
        return ExecOutcome::Continue(0);
    }

    if delete {
        if names.is_empty() {
            eprintln!("huck: hash: -d: at least one name required");
            return ExecOutcome::Continue(2);
        }
        let mut exit: i32 = 0;
        for name in names {
            if shell.command_hash.remove(name).is_none() {
                eprintln!("huck: hash: {name}: not found");
                exit = 1;
            }
        }
        return ExecOutcome::Continue(exit);
    }

    if set_path {
        // Exactly one name required.
        if names.len() != 1 {
            eprintln!("huck: hash: -p: exactly one name required");
            return ExecOutcome::Continue(2);
        }
        let name = &names[0];
        if name.contains('/') {
            eprintln!("huck: hash: {name}: must not contain `/'");
            return ExecOutcome::Continue(1);
        }
        let path = explicit_path.unwrap();   // safe: set_path implies Some
        shell.command_hash.insert(
            name.clone(),
            (std::path::PathBuf::from(path), 0u32),
        );
        return ExecOutcome::Continue(0);
    }

    if list {
        // re-input form: `builtin hash -p PATH NAME`
        let mut entries: Vec<(&String, &(std::path::PathBuf, u32))> =
            shell.command_hash.iter().collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));
        for (name, (path, _)) in entries {
            let _ = writeln!(out, "builtin hash -p {} {}", path.display(), name);
        }
        return ExecOutcome::Continue(0);
    }

    if type_only {
        if names.is_empty() {
            eprintln!("huck: hash: -t: at least one name required");
            return ExecOutcome::Continue(2);
        }
        let mut exit: i32 = 0;
        for name in names {
            match shell.command_hash.get(name) {
                Some((path, _)) => {
                    if names.len() == 1 {
                        let _ = writeln!(out, "{}", path.display());
                    } else {
                        let _ = writeln!(out, "{}\t{}", name, path.display());
                    }
                }
                None => {
                    eprintln!("huck: hash: {name}: not found");
                    exit = 1;
                }
            }
        }
        return ExecOutcome::Continue(exit);
    }

    // Default: with names → resolve+add; without → list.
    if names.is_empty() {
        if shell.command_hash.is_empty() {
            let _ = writeln!(out, "hash table empty");
            return ExecOutcome::Continue(0);
        }
        let mut entries: Vec<(&String, &(std::path::PathBuf, u32))> =
            shell.command_hash.iter().collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));
        let _ = writeln!(out, "hits\tcommand");
        for (name, (path, hits)) in entries {
            let _ = writeln!(out, "{:>4}\t{}", hits, path.display());
            let _ = name; // suppress unused; the bash format only
                          // shows path, not name. Keep the binding
                          // here in case a future iteration adds
                          // name display.
        }
        return ExecOutcome::Continue(0);
    }

    let mut exit: i32 = 0;
    for name in names {
        if name.contains('/') {
            eprintln!("huck: hash: {name}: must not contain `/'");
            exit = 1;
            continue;
        }
        match search_path_for(name, shell) {
            Some(path) => {
                shell.command_hash.insert(name.clone(), (path, 0u32));
            }
            None => {
                eprintln!("huck: hash: {name}: not found");
                exit = 1;
            }
        }
    }
    ExecOutcome::Continue(exit)
}
```

**Bash listing format note**: bash actually prints both `hits`
and `command`, where `command` is the absolute path (NOT the
name) — the hashed name is the key for lookup; the value
displayed is the resolved path. Match bash.

(Actually re-checking bash: the listing IS just `hits\tcommand`
where command is the path. The name is the key but not shown in
the default listing. `-l` shows the re-input form which DOES
include the name.)

### 3. Dispatch + `BUILTIN_NAMES`

- `"hash"` added to `BUILTIN_NAMES`.
- NOT in `is_special_builtin` (POSIX classifies regular).
- `"hash" => builtin_hash(args, out, shell)` arm.

## Behavior table

| Input | Output / behavior |
|---|---|
| `hash` (empty table) | stdout `hash table empty`, exit 0 |
| `hash ls` (ls in PATH) | (no output), exit 0, table now `ls → /usr/bin/ls` |
| `hash __nope__` | stderr `huck: hash: __nope__: not found`, exit 1 |
| `hash ls cat` (both in PATH) | (no output), exit 0, both entries added |
| `hash ls __nope__` | stderr error for `__nope__`, exit 1, BUT `ls` still added |
| `hash` (with entries) | listing: `hits\tcommand\n   0\t/usr/bin/ls\n` etc. |
| `hash -r` | clears table, exit 0 |
| `hash -d ls` | deletes ls if present; exit 1 if not present |
| `hash -d ls cat` | tries both; exit 1 if any missing |
| `hash -p /custom/path mycmd` | adds `mycmd → /custom/path` regardless of PATH |
| `hash -p` (no path) | usage error, exit 2 |
| `hash -p /foo` (no name) | exit 2 "exactly one name required" |
| `hash -p /foo a b` | exit 2 "exactly one name required" |
| `hash -l` (with entries) | `builtin hash -p PATH NAME` lines, one per entry |
| `hash -t ls` (in hash) | prints `/usr/bin/ls`, exit 0 |
| `hash -t ls cat` (both in) | `ls\t/usr/bin/ls\ncat\t/bin/cat\n` |
| `hash -t __nope__` | stderr error, exit 1 |
| `hash a/b` (path-like) | stderr `must not contain '/'`, exit 1 |
| `hash -X` | exit 2, stderr invalid option |

## Test plan

### Unit tests in `src/builtins.rs::mod hash_tests` (12 tests)

1. `hash_empty_lists_empty` — fresh shell + bare `hash` → stdout `hash table empty\n`.
2. `hash_p_adds_direct` — `hash -p /custom mycmd` → entry inserted; verify `shell.command_hash["mycmd"] == (/custom, 0)`.
3. `hash_r_clears` — populate via -p, then `hash -r` → table empty.
4. `hash_d_removes` — populate, then `hash -d mycmd` → table empty, exit 0.
5. `hash_d_missing_errors` — bare `hash -d mycmd` (not present) → exit 1.
6. `hash_l_re_input_form` — `hash -p /foo a`, then `hash -l` → stdout `builtin hash -p /foo a\n`.
7. `hash_t_single_name` — `hash -p /foo a`, then `hash -t a` → stdout `/foo\n`.
8. `hash_t_multi_name_tabs` — populate a and b, then `hash -t a b` → `a\t/foo\nb\t/bar\n`.
9. `hash_t_missing_errors_status_1` — `hash -t a` (not present) → exit 1.
10. `hash_path_like_name_rejected` — `hash a/b` → exit 1 + stderr "must not contain".
11. `hash_invalid_option_status_2` — `hash -X` → exit 2.
12. `hash_p_no_arg_status_2` — `hash -p` (no value) → exit 2.

Note: tests for the PATH-lookup form (`hash ls`) are harder in
unit tests because PATH varies. Moving those to integration.

### Integration tests in `tests/hash_integration.rs` (7 tests)

1. `hash_empty_table_listing` — `hash\n` → stdout has `hash table empty`.
2. `hash_p_then_list` — `hash -p /foo a; hash -l` → stdout has `builtin hash -p /foo a`.
3. `hash_path_lookup_succeeds_for_sh` — `hash sh\n` exits 0; subsequent `hash -t sh` prints a `/sh` path.
4. `hash_path_lookup_fails_for_missing` — `hash __nope__` → stderr "not found", rc=1.
5. `hash_r_clears` — `hash -p /foo a; hash -r; hash -t a` → exit 1 (cleared).
6. `hash_t_multi_format` — `hash -p /foo a; hash -p /bar b; hash -t a b` → contains both `a\t/foo` and `b\t/bar`.
7. `hash_d_then_lookup_fails` — `hash -p /foo a; hash -d a; hash -t a` → exit 1.

### Smoke

`cargo test --all-targets` green (PTY flake tolerated).

## Implementation tasks

1. **Foundation + builtin + 12 unit tests** — `src/shell_state.rs`
   (add `command_hash` field + init) and `src/builtins.rs`
   (`builtin_hash`, dispatch, `BUILTIN_NAMES`, `mod hash_tests`).

2. **Integration tests** — `tests/hash_integration.rs` with 7
   scenarios.

3. **Docs** — Update M-34 entry from `[deferred]` to `[fixed v59
   partial]`; change-log; README v59 row.

## Acceptance criteria

- 12 unit tests pass.
- 7 integration tests pass.
- `cargo test --all-targets` green (PTY flake tolerated).
- `cargo clippy --all-targets -- -D warnings` clean.
- `hash` is regular (NOT in `is_special_builtin`).
- All six modes work: default (resolve+add or list), -r, -d, -p,
  -l, -t.
- `hash -p` requires exactly one name.
- Names containing `/` are rejected for `hash NAME` and
  `hash -p PATH NAME`.
- Listing format: `hits\tcommand` header + `   0\t/path` rows.
- All existing tests still pass.
- M-34 doc entry updated; partial-fix nature called out.
