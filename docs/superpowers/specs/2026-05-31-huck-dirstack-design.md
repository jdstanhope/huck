# huck v63 — `pushd`/`popd`/`dirs` (M-78)

## Goal

Ship bash's directory-stack builtins: `pushd`, `popd`, `dirs`.
Full bash form including `+N` / `-N` rotation/index forms.

After v63:

- `pushd DIR` — push current dir, `cd` to DIR, print new stack.
- `pushd` (no args) — swap top two; cd.
- `pushd +N` / `pushd -N` — rotate so the indexed entry is on top.
- `popd` — pop top, cd to new top.
- `popd +N` / `popd -N` — remove the indexed entry; cd only if
  index 0 was removed.
- `dirs` — print stack space-joined, `~`-collapsed.
- `dirs -c` — clear stack (keep current dir as top).
- `dirs -l` — print without `~` collapse.
- `dirs -p` — one entry per line.
- `dirs -v` — numbered, one per line.
- `dirs +N` / `dirs -N` — print just the Nth entry.

New tracked divergence: **M-78: `pushd`/`popd`/`dirs`**.

## Scope decisions (locked via AskUserQuestion)

**Tier C — full bash including `+N`/`-N` rotation**.

## Out of scope (deferred)

- `pushd -n DIR` / `popd -n` (bash extensions — push without
  cd, pop without cd).
- `DIRSTACK` shell array (huck has no arrays).
- The shopt `dotglob`/`extglob`/etc. interactions on `~` (none
  relevant here).

## Architecture

### Data model

New `Shell.dir_stack: Vec<PathBuf>` field. Conventions:

- **Top is index 0** (matches `dirs` output where the leftmost
  printed entry is current dir).
- `stack[0]` is always logically `$PWD`. Since `cd` doesn't
  touch the stack directly, we sync `stack[0]` from
  `lookup_var("PWD")` (or `current_dir()` fallback) at the top
  of each pushd/popd/dirs call.
- Initial state: stack starts empty; first sync populates it
  with one entry (current dir).

A sync helper:

```rust
fn sync_stack_top(shell: &mut Shell) {
    let cwd = shell
        .lookup_var("PWD")
        .or_else(|| std::env::current_dir().ok().map(|p| p.display().to_string()))
        .unwrap_or_default();
    let p = PathBuf::from(cwd);
    if shell.dir_stack.is_empty() {
        shell.dir_stack.push(p);
    } else {
        shell.dir_stack[0] = p;
    }
}
```

### Pure helpers

`parse_signed_index(s, stack_len) -> Result<usize, String>`:

```rust
/// Parses "+N" / "-N" into a left-indexed stack position.
/// +N: index N from left (0 = top).
/// -N: index N from right (0 = bottom).
fn parse_signed_index(s: &str, stack_len: usize) -> Result<usize, String> {
    let (sign_plus, digits) = if let Some(d) = s.strip_prefix('+') {
        (true, d)
    } else if let Some(d) = s.strip_prefix('-') {
        (false, d)
    } else {
        return Err(format!("{s}: not a +N or -N specifier"));
    };
    let n: usize = digits.parse()
        .map_err(|_| format!("{s}: invalid number"))?;
    if n >= stack_len {
        return Err(format!("{s}: directory stack index out of range"));
    }
    Ok(if sign_plus { n } else { stack_len - 1 - n })
}
```

`tilde_collapse(path, shell) -> String` — reuse v60's
`cwd_tilde` logic. Since `src/prompt.rs::cwd_tilde` only reads
`shell.lookup_var("HOME")` etc., factor a similar helper here or
just inline the small block.

Inline helper for v63:

```rust
fn dir_display(path: &Path, shell: &Shell, collapse: bool) -> String {
    let s = path.display().to_string();
    if !collapse { return s; }
    let home = shell.lookup_var("HOME")
        .or_else(|| std::env::var("HOME").ok())
        .unwrap_or_default();
    if home.is_empty() { return s; }
    if s == home { return "~".to_string(); }
    if let Some(rest) = s.strip_prefix(&format!("{home}/")) {
        return format!("~/{rest}");
    }
    s
}
```

### `builtin_pushd`

```rust
fn builtin_pushd(
    args: &[String],
    out: &mut dyn std::io::Write,
    shell: &mut Shell,
) -> ExecOutcome {
    sync_stack_top(shell);

    if args.is_empty() {
        // Swap top two.
        if shell.dir_stack.len() < 2 {
            eprintln!("huck: pushd: no other directory");
            return ExecOutcome::Continue(1);
        }
        shell.dir_stack.swap(0, 1);
        // cd to new top.
        let target = shell.dir_stack[0].clone();
        let cd_args = vec![target.display().to_string()];
        if let ExecOutcome::Continue(c) = builtin_cd(&cd_args, shell) {
            if c != 0 {
                // Swap back to undo on failure.
                shell.dir_stack.swap(0, 1);
                return ExecOutcome::Continue(c);
            }
        }
        return print_stack(out, shell, /*collapse*/ true, /*per_line*/ false, /*numbered*/ false);
    }

    let arg = &args[0];
    // +N / -N rotation.
    if arg.starts_with('+') || (arg.starts_with('-') && arg.len() > 1 && arg.as_bytes()[1].is_ascii_digit()) {
        let idx = match parse_signed_index(arg, shell.dir_stack.len()) {
            Ok(i) => i,
            Err(e) => {
                eprintln!("huck: pushd: {e}");
                return ExecOutcome::Continue(1);
            }
        };
        if idx == 0 {
            // No rotation needed.
            return print_stack(out, shell, true, false, false);
        }
        shell.dir_stack.rotate_left(idx);
        let target = shell.dir_stack[0].clone();
        let cd_args = vec![target.display().to_string()];
        if let ExecOutcome::Continue(c) = builtin_cd(&cd_args, shell) {
            if c != 0 {
                shell.dir_stack.rotate_right(idx);  // undo
                return ExecOutcome::Continue(c);
            }
        }
        return print_stack(out, shell, true, false, false);
    }

    // pushd DIR
    let cd_args = vec![arg.clone()];
    if let ExecOutcome::Continue(c) = builtin_cd(&cd_args, shell) {
        if c != 0 {
            return ExecOutcome::Continue(c);
        }
    }
    // Insert the new current dir at the front.
    let new_cwd = shell.lookup_var("PWD")
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from(arg));
    shell.dir_stack.insert(0, new_cwd);
    print_stack(out, shell, true, false, false)
}
```

### `builtin_popd`

```rust
fn builtin_popd(
    args: &[String],
    out: &mut dyn std::io::Write,
    shell: &mut Shell,
) -> ExecOutcome {
    sync_stack_top(shell);
    if shell.dir_stack.len() <= 1 {
        eprintln!("huck: popd: directory stack empty");
        return ExecOutcome::Continue(1);
    }

    let idx = if args.is_empty() {
        0
    } else {
        let arg = &args[0];
        if !(arg.starts_with('+') || (arg.starts_with('-') && arg.len() > 1 && arg.as_bytes()[1].is_ascii_digit())) {
            eprintln!("huck: popd: {arg}: invalid argument");
            return ExecOutcome::Continue(1);
        }
        match parse_signed_index(arg, shell.dir_stack.len()) {
            Ok(i) => i,
            Err(e) => {
                eprintln!("huck: popd: {e}");
                return ExecOutcome::Continue(1);
            }
        }
    };

    shell.dir_stack.remove(idx);
    if idx == 0 {
        let target = shell.dir_stack[0].clone();
        let cd_args = vec![target.display().to_string()];
        if let ExecOutcome::Continue(c) = builtin_cd(&cd_args, shell) {
            if c != 0 {
                return ExecOutcome::Continue(c);
            }
        }
    }
    print_stack(out, shell, true, false, false)
}
```

### `builtin_dirs`

```rust
fn builtin_dirs(
    args: &[String],
    out: &mut dyn std::io::Write,
    shell: &mut Shell,
) -> ExecOutcome {
    sync_stack_top(shell);

    let mut collapse = true;
    let mut per_line = false;
    let mut numbered = false;
    let mut clear = false;
    let mut index: Option<usize> = None;

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "-c" => { clear = true; i += 1; }
            "-l" => { collapse = false; i += 1; }
            "-p" => { per_line = true; i += 1; }
            "-v" => { per_line = true; numbered = true; i += 1; }
            s if s.starts_with('+') || (s.starts_with('-') && s.len() > 1 && s.as_bytes()[1].is_ascii_digit()) => {
                match parse_signed_index(s, shell.dir_stack.len()) {
                    Ok(idx) => index = Some(idx),
                    Err(e) => {
                        eprintln!("huck: dirs: {e}");
                        return ExecOutcome::Continue(1);
                    }
                }
                i += 1;
            }
            s if s.starts_with('-') && s.len() > 1 => {
                eprintln!("huck: dirs: {s}: invalid option");
                return ExecOutcome::Continue(2);
            }
            _ => break,
        }
    }

    if clear {
        shell.dir_stack.truncate(1);
        return ExecOutcome::Continue(0);
    }
    if let Some(idx) = index {
        let entry = &shell.dir_stack[idx];
        let _ = writeln!(out, "{}", dir_display(entry, shell, collapse));
        return ExecOutcome::Continue(0);
    }
    print_stack(out, shell, collapse, per_line, numbered)
}
```

### `print_stack` helper

```rust
fn print_stack(
    out: &mut dyn std::io::Write,
    shell: &Shell,
    collapse: bool,
    per_line: bool,
    numbered: bool,
) -> ExecOutcome {
    if per_line {
        for (i, p) in shell.dir_stack.iter().enumerate() {
            let disp = dir_display(p, shell, collapse);
            if numbered {
                let _ = writeln!(out, "{i:>2}  {disp}");
            } else {
                let _ = writeln!(out, "{disp}");
            }
        }
    } else {
        let parts: Vec<String> = shell
            .dir_stack
            .iter()
            .map(|p| dir_display(p, shell, collapse))
            .collect();
        let _ = writeln!(out, "{}", parts.join(" "));
    }
    ExecOutcome::Continue(0)
}
```

### `builtin_cd` integration

`builtin_cd` is currently private (`fn` in `src/builtins.rs`).
Promote to `pub(crate) fn` so the dirstack builtins can call it
directly.

### Dispatch + `BUILTIN_NAMES`

Append `"pushd"`, `"popd"`, `"dirs"` to `BUILTIN_NAMES`. None
are POSIX special. Add three dispatch arms.

## Behavior table

| Scenario | Behavior |
|---|---|
| (fresh shell) `dirs` | `~` (just current dir) |
| `pushd /tmp` (success) | cd to /tmp; print `/tmp ~/prev` |
| `pushd /nope` | cd fails → stack unchanged, exit 1 |
| `pushd` (stack=[A,B]) | swap → cd to B; print `B A` |
| `pushd` (stack=[A] only) | "no other directory" + exit 1 |
| `pushd +2` (stack=[A,B,C,D]) | rotate to C,D,A,B; cd C |
| `pushd -0` (stack=[A,B,C,D]) | -0 = last entry (D) → rotate to D,A,B,C |
| `popd` (stack=[A,B,C]) | remove A; cd to B; print `B C` |
| `popd` (stack=[A]) | "directory stack empty" + exit 1 |
| `popd +1` (stack=[A,B,C]) | remove B; no cd; print `A C` |
| `popd -0` (stack=[A,B,C]) | -0 = last (C) → remove; no cd; print `A B` |
| `dirs -c` | clear stack (keep only current dir) |
| `dirs -l` | print stack with no `~` collapse |
| `dirs -p` | one per line, no numbers |
| `dirs -v` | numbered (e.g., ` 0  ~`), one per line |
| `dirs +0` | print just current dir |
| `dirs -1` | print second-from-bottom |
| `pushd +5` (only 3 entries) | "out of range" + exit 1 |
| `dirs -X` | "invalid option" + exit 2 |

## Test plan

### Unit tests in `src/builtins.rs::mod dirstack_tests` (10 tests)

Pure helpers (no filesystem touches):

1. `parse_signed_index_plus` — `+0`, `+2`, `+5` against a length-10 stack.
2. `parse_signed_index_minus` — `-0` → last index; `-1` → second-from-last.
3. `parse_signed_index_out_of_range` — `+10` against length-10 → Err.
4. `parse_signed_index_invalid` — `+abc` → Err.
5. `parse_signed_index_no_sign` — `2` (no prefix) → Err.
6. `dir_display_no_home` — empty HOME, just returns path verbatim.
7. `dir_display_home_match_collapses` — HOME=/h/me, path=/h/me → `~`.
8. `dir_display_home_subdir_collapses` — HOME=/h/me, path=/h/me/x → `~/x`.
9. `dir_display_no_collapse_flag` — collapse=false returns path verbatim even when under HOME.
10. `dir_display_unrelated_path_passes_through` — HOME=/h/me, path=/etc/foo → `/etc/foo`.

### Integration tests in `tests/dirstack_integration.rs` (10 tests)

Filesystem-touching scenarios:

1. `pushd_dir_then_dirs` — `pushd /tmp; dirs` → first entry contains `/tmp`.
2. `pushd_then_popd_returns_to_origin` — start at HOME (or fixed dir); `pushd /tmp; popd; pwd` → original.
3. `pushd_no_args_swaps_top_two` — `pushd /tmp; pushd /var; pushd; pwd` → /tmp.
4. `pushd_only_one_entry_errors` — bare `pushd` on fresh shell → rc=1 + stderr.
5. `popd_empty_errors` — fresh shell `popd` → rc=1 + stderr.
6. `dirs_default_collapses_home` — set HOME=$(pwd); `dirs` → `~`.
7. `dirs_v_numbered` — `pushd /tmp; pushd /var; dirs -v` → 3 numbered lines.
8. `dirs_c_clears` — `pushd /tmp; dirs -c; dirs` → only current dir.
9. `pushd_plus_n_rotates` — `pushd /tmp; pushd /var; pushd +2; pwd` → original dir (rotated such that the bottom of stack is now top).
10. `dirs_plus_index_prints_one` — `pushd /tmp; dirs +1` → original HOME's path or `~`.

### Smoke

`cargo test --all-targets` green (PTY flake tolerated).

## Implementation tasks

1. **Foundation + builtins + 10 unit tests** — modify
   `src/shell_state.rs` (add `dir_stack` field + init); modify
   `src/builtins.rs` (`pub(crate) fn builtin_cd`; add
   `parse_signed_index`, `dir_display`, `sync_stack_top`,
   `print_stack`, `builtin_pushd`, `builtin_popd`,
   `builtin_dirs`; dispatch arms; `BUILTIN_NAMES`; `mod
   dirstack_tests`).

2. **Integration tests** — create `tests/dirstack_integration.rs`
   with 10 scenarios.

3. **Docs** — M-78 entry; change-log; README v63 row.

## Acceptance criteria

- 10 unit tests pass.
- 10 integration tests pass.
- `cargo test --all-targets` green (PTY flake tolerated).
- `cargo clippy --all-targets -- -D warnings` clean.
- `pushd`, `popd`, `dirs` all regular (NOT in
  `is_special_builtin`).
- Full flag set works: `-c`, `-l`, `-p`, `-v`, `+N`, `-N`.
- Stack top always reflects current `$PWD` after sync.
- All existing tests still pass.
- M-78 doc entry added.
