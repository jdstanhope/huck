# huck v54 — `readonly` builtin (M-71)

## Goal

Add bash's `readonly` builtin so a variable can be marked
read-only — any subsequent write (or `unset`) errors.

After v54:

- `readonly NAME=value` — set NAME to value, mark readonly.
- `readonly NAME` (NAME already set) — keep value, mark readonly.
- `readonly NAME` (NAME unset) — create with empty value, mark
  readonly (bash-compat).
- `readonly NAME1=v1 NAME2 NAME3=v3` — multiple per call.
- `readonly` / `readonly -p` — list all readonly vars in POSIX
  `readonly NAME='escaped'` format.
- Once readonly, every write path errors and the originating
  command/builtin returns non-zero:
  - Top-level `NAME=newval`.
  - Inline `NAME=newval cmd` (aborts the command).
  - For-loop iter var (`for NAME in a b c; do …; done`).
  - `${NAME:=default}` default-assignment.
  - Arithmetic assignment `$((NAME = 5))`.
  - `export NAME=newval`.
  - `local NAME` / `local NAME=v` in a function.
  - `unset NAME`.

New tracked divergence: **M-71: `readonly`**.

## Scope decisions (locked via AskUserQuestion)

1. **Full POSIX/bash variable form** — `readonly NAME=value`,
   `readonly NAME`, multiple per call, listing.
2. **Enforcement at ALL user-facing write paths** (the 8 listed
   above).
3. **Deferred**: `readonly -f` (function readonly), `readonly -a`
   (array — huck has no arrays), `readonly -A` (assoc array).
4. **Variable model**: add a `readonly: bool` flag to the
   `Variable` struct (mirrors `exported: bool`).
5. **Listing format**: POSIX `readonly NAME='escaped-value'` (we
   don't have `declare` yet, so bash's `declare -r NAME=value`
   form isn't available).

## Out of scope (deferred)

- `readonly -f`, `-a`, `-A`.
- Internal-mechanism writes (`cd` updating PWD/OLDPWD, signal-state
  updates) are explicitly EXEMPT from readonly enforcement in
  v54 — they go through `shell.set()` (unchecked) by design. A
  realistic divergence is `readonly PWD; cd /tmp` — bash would
  error; huck silently succeeds. Acceptable. Logged in M-71's
  Known limitations.

## Architecture

### 1. `Variable.readonly: bool` (`src/shell_state.rs`)

```rust
#[derive(Debug, Clone)]
pub struct Variable {
    pub value: String,
    pub exported: bool,
    pub readonly: bool,    // NEW
}
```

`Shell::new` doesn't construct Variables directly; the writer
methods (`set`, `export_set`) currently create `Variable { value,
exported: false }` and we change them to also default
`readonly: false`. All existing call sites continue to work.

### 2. New `Shell` methods (`src/shell_state.rs`)

```rust
/// True iff `name` is currently set AND marked readonly.
pub fn is_readonly(&self, name: &str) -> bool {
    self.vars.get(name).map(|v| v.readonly).unwrap_or(false)
}

/// Attempts to set `name` to `value`. Returns `Err(())` if `name`
/// is readonly; the caller is responsible for printing a
/// diagnostic ("huck: NAME: readonly variable"). Preserves the
/// existing `exported` flag on a successful set.
pub fn try_set(&mut self, name: &str, value: String) -> Result<(), ()> {
    if self.is_readonly(name) { return Err(()); }
    self.set(name, value);
    Ok(())
}

/// Attempts to remove `name` from the variable table. Returns
/// `Err(())` if `name` is readonly.
pub fn try_unset(&mut self, name: &str) -> Result<(), ()> {
    if self.is_readonly(name) { return Err(()); }
    self.unset(name);
    Ok(())
}

/// Marks `name` readonly. If `name` is unset, creates it with an
/// empty value (bash behavior: `readonly NAME` on an unset name
/// declares it readonly with value `""`). If `name` is already
/// set, preserves its value and exported flag.
pub fn mark_readonly(&mut self, name: &str) {
    if let Some(v) = self.vars.get_mut(name) {
        v.readonly = true;
    } else {
        self.vars.insert(name.to_string(), Variable {
            value: String::new(),
            exported: false,
            readonly: true,
        });
    }
}

/// Returns names of all currently-readonly vars, sorted.
pub fn readonly_names(&self) -> Vec<String> {
    let mut names: Vec<String> = self
        .vars
        .iter()
        .filter(|(_, v)| v.readonly)
        .map(|(k, _)| k.clone())
        .collect();
    names.sort();
    names
}
```

`set()` stays unchecked (used by internal mechanisms like `cd`'s
PWD/OLDPWD updates, signal-state, and existing test setup). User
write paths migrate to `try_set` / `try_unset`.

### 3. `builtin_readonly` (`src/builtins.rs`)

```rust
fn builtin_readonly(
    args: &[String],
    out: &mut dyn std::io::Write,
    shell: &mut Shell,
) -> ExecOutcome {
    // No args, or just `-p`: list. Any other `-X` flag → error.
    let mut want_list = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-p" => { want_list = true; i += 1; }
            "--" => { i += 1; break; }
            s if s.starts_with('-') && s.len() > 1 => {
                eprintln!("huck: readonly: {s}: invalid option");
                return ExecOutcome::Continue(2);
            }
            _ => break,
        }
    }
    let names = &args[i..];

    if names.is_empty() || want_list {
        // List all readonly vars.
        for name in shell.readonly_names() {
            let value = shell.lookup_var(&name).unwrap_or_default();
            let _ = writeln!(out, "readonly {name}='{}'",
                escape_alias_value(&value));
        }
        return ExecOutcome::Continue(0);
    }

    let mut exit = 0;
    for arg in names {
        let (name, value): (&str, Option<String>) = match arg.find('=') {
            Some(eq) => (&arg[..eq], Some(arg[eq+1..].to_string())),
            None => (arg.as_str(), None),
        };
        if !is_valid_name(name) {
            eprintln!("huck: readonly: `{arg}': not a valid identifier");
            exit = 1;
            continue;
        }
        if let Some(v) = value {
            // Attempt to set, then mark readonly. If already
            // readonly, error and continue (don't mark twice,
            // don't write).
            if shell.is_readonly(name) {
                eprintln!("huck: readonly: {name}: readonly variable");
                exit = 1;
                continue;
            }
            shell.set(name, v);
        }
        shell.mark_readonly(name);
    }
    ExecOutcome::Continue(exit)
}
```

`escape_alias_value` (defined for `builtin_alias` and reused by
v53's `builtin_command`) gives POSIX single-quote escaping.

### 4. Builtin-layer enforcement (`src/builtins.rs`)

#### `builtin_unset`

Current loop body essentially: `shell.unset(arg);`. Change to:

```rust
if shell.is_readonly(arg) {
    eprintln!("huck: unset: {arg}: readonly variable");
    exit_status = 1;
    continue;
}
shell.unset(arg);
```

#### `builtin_export`

For each `NAME=value` arg: check `is_readonly(name)` before
calling `shell.export_set`. If violated, print diagnostic, set
exit=1, continue.

For bare `NAME` (no `=value`): bash allows `export READONLY_NAME`
— it just flips the export flag without changing the value. We
match: skip the readonly check for the value-less form. (The
readonly variable becomes exported but its value is untouched.)

#### `builtin_local`

Inside a function, `local NAME=value` and `local NAME` both
need a readonly check: if NAME is readonly in the outer scope,
bash errors with status 1 and DOES NOT snapshot or set. Match.

### 5. Executor-layer enforcement

Five sites need readonly checks:

#### (a) `executor.rs:291` — for-loop iteration var

```rust
if shell.try_set(&clause.var, value).is_err() {
    eprintln!("huck: {}: readonly variable", clause.var);
    return ExecOutcome::Continue(1);   // abort loop
}
```

#### (b) `executor.rs:1358` — top-level `SimpleCommand::Assign`

```rust
SimpleCommand::Assign(items) => {
    for (name, value) in items {
        let v = expand_assignment(value, shell);
        if shell.try_set(name, v).is_err() {
            eprintln!("huck: {name}: readonly variable");
            return ExecOutcome::Continue(1);
        }
    }
    ExecOutcome::Continue(0)
}
```

#### (c) `executor.rs::apply_inline_assignments`

Change signature to:

```rust
fn apply_inline_assignments(
    assignments: &[(String, Word)],
    shell: &mut Shell,
) -> Result<AssignmentSnapshot, AssignmentSnapshot>
```

Loop body: before the `shell.export_set(name, value)`, check
`is_readonly(name)`. If violated: print
"huck: {name}: readonly variable", return `Err(snap)`. Caller
restores the partial snapshot via `restore_inline_assignments`
and returns `Continue(1)` (the command does not run).

All call sites must be updated:
- `run_double_bracket` (`executor.rs:422`)
- The main simple-command path (`executor.rs:691`)
- Any other internal callers (search to confirm).

#### (d) `param_expansion.rs:45` — `${var:=default}`

Replace `shell.set(name, v.clone())` with `try_set`. On Err: print
"huck: {name}: readonly variable" + signal a param-expansion
error (the existing expansion-error pathway). Likely the easiest
mirror is to emit the existing `expansion_error` mechanism — read
the surrounding context to choose the right signal. Acceptable
fallback: ignore the assignment but continue expansion (returns the
existing value) and print the diagnostic — also matches what bash
does for non-fatal `${var:=…}` failures in non-interactive scripts.

#### (e) `arith.rs:592` — arithmetic assignment

Replace `shell.set(name, value.to_string())` with a checked path.
On Err: print "huck: NAME: readonly variable" + propagate an
arithmetic error (the existing path that signals "expected
expression" / "division by zero" etc). The likely simplest fix is
to make the surrounding function return an error so the caller of
`$((NAME=5))` substitutes nothing and the simple command exits
non-zero.

### 6. Dispatch + BUILTIN_NAMES + is_special_builtin

- `"readonly"` added to `BUILTIN_NAMES`.
- `"readonly"` added to `is_special_builtin`'s matched set (POSIX
  classifies it as special).
- `"readonly" => builtin_readonly(args, out, shell)` arm in
  `run_builtin` near `"export"`.

## Behavior table

| Input | Behavior |
|---|---|
| `readonly` (no args) | list all readonly vars in `readonly NAME='value'` form |
| `readonly -p` | same as above |
| `readonly X` (X unset) | X="" + readonly; future writes error |
| `readonly X` (X="prev") | X="prev" + readonly |
| `readonly X=1 Y Z=3` | X="1"+ro, Y=""+ro, Z="3"+ro |
| `readonly 1foo=bar` | "not a valid identifier" + status 1 (and other args still processed) |
| `readonly X=newval` (X already ro) | "X: readonly variable" + status 1 (does NOT overwrite) |
| `X=newval` after `readonly X` | "X: readonly variable" + status 1 |
| `X=v cmd` after `readonly X` | "X: readonly variable" + status 1, cmd does NOT run |
| `for X in a b c; do …; done` after `readonly X` | "X: readonly variable" at first iter + status 1, body NOT run |
| `${X:=default}` after `readonly X` (X unset/empty) | "X: readonly variable" + expansion-error path |
| `$((X=5))` after `readonly X` | "X: readonly variable" + arith-error |
| `export X=newval` after `readonly X` | "X: readonly variable" + status 1 |
| `export X` after `readonly X` | exit 0 (just flips export flag) |
| `local X=v` in fn, where caller has `readonly X` | "local: X: readonly variable" + status 1 |
| `unset X` after `readonly X` | "X: readonly variable" + status 1 |
| `readonly X=v && echo done` (success) | "done" |

## Test plan

### Unit tests in `src/builtins.rs::mod readonly_tests` (10):

1. `readonly_with_value_sets_and_locks`.
2. `readonly_no_value_creates_empty_and_locks`.
3. `readonly_no_value_keeps_existing_value`.
4. `readonly_multi_arg_mixed_forms`.
5. `readonly_invalid_identifier_errors`.
6. `readonly_listing_no_args` — pre-set readonly X="v" + Y, then
   `readonly` lists both in sorted order with escaped quotes.
7. `readonly_dash_p_same_as_no_args`.
8. `readonly_overwrite_existing_readonly_errors` — `readonly X=1`
   twice with different values → second errors + X stays "1".
9. `unset_readonly_errors_status_1`.
10. `export_readonly_value_errors_but_bare_export_succeeds`.

### Unit tests in `src/executor.rs::mod tests` (6):

11. `top_level_assign_to_readonly_errors`.
12. `inline_assignment_to_readonly_aborts_command` — verify
    inline-assignment side effects don't apply AND the command
    doesn't run.
13. `for_loop_iter_var_readonly_aborts_at_first_iter`.
14. `param_expansion_default_assign_to_readonly_errors`.
15. `arith_assign_to_readonly_errors`.
16. `local_readonly_in_function_errors`.

### Integration tests in `tests/readonly_integration.rs` (6):

17. `readonly_basic_blocks_reassignment` — `readonly X=1\nX=2\necho ok\nexit\n`
    → stderr "readonly", stdout does not contain "ok" with `set -e`-style
    semantics? Actually no — huck doesn't have `set -e`. The script
    continues after the error. Assertion: stderr contains "readonly",
    stdout still has `ok` line (assignment failed but next cmd ran),
    and `echo $?` after the assignment is "1".
18. `readonly_lists_in_posix_format` — `readonly X='a b'\nreadonly\nexit\n`
    → stdout contains `readonly X='a b'`.
19. `readonly_blocks_unset` — `readonly X=1\nunset X\nrc=$?\necho rc=$rc\nexit\n`
    → "rc=1" + stderr "readonly".
20. `readonly_blocks_inline_assignment` — `readonly X=1\nX=2 echo hi\nrc=$?\necho rc=$rc\nexit\n`
    → "rc=1", and "hi" should NOT appear in stdout.
21. `readonly_blocks_for_loop` — `readonly X=outer\nfor X in a b c; do echo $X; done\nrc=$?\necho rc=$rc\nexit\n`
    → no "a"/"b"/"c" lines; "rc=1".
22. `readonly_with_single_quote_listing_escapes` — `readonly X="a'b"\nreadonly\nexit\n`
    → stdout has `readonly X='a'\''b'` (POSIX escape).

### Smoke

`cargo test --all-targets` green. PTY flake tolerated.

## Implementation tasks

1. **Foundation + builtins-layer enforcement** — `src/shell_state.rs`
   (add `readonly` field, helpers); `src/builtins.rs` (builtin_readonly,
   readonly checks in unset/export/local, BUILTIN_NAMES,
   is_special_builtin, dispatch, 10 unit tests in `mod readonly_tests`).

2. **Executor-layer enforcement + integration tests** — touch
   `src/executor.rs` (for-loop, top-level Assign,
   apply_inline_assignments signature + callers, + 6 executor tests),
   `src/param_expansion.rs` (`${var:=…}` check), `src/arith.rs`
   (assignment check); `tests/readonly_integration.rs` (6 tests).

3. **Docs** — `docs/bash-divergences.md` M-71 entry + change-log;
   `README.md` v54 row.

## Acceptance criteria

- All 16 unit tests pass (10 builtin + 6 executor).
- All 6 integration tests pass.
- `cargo test --all-targets` green (PTY flake tolerated).
- `cargo clippy --all-targets -- -D warnings` clean.
- `readonly` is special; in `is_special_builtin`.
- Pre-existing tests still pass after the `Variable` field addition.
- The 5 executor write sites + 3 builtin-layer write sites all
  reject writes to readonly vars with a clear diagnostic.
- Listing format: `readonly NAME='value'` with single-quote
  escaping.
