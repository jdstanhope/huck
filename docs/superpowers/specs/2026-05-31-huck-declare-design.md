# huck v64 — `declare` / `typeset` (M-79)

## Goal

Add bash's `declare` (with `typeset` as alias) builtin —
Tier A scope. Wires bash's flag-driven variable attributes to
huck's existing primitives:

- `-r` → v54's readonly flag (`mark_readonly`, `try_set`).
- `-x` → existing exported flag (`export`, `export_set`).
- `+x` → new `unexport` method (just flips the flag).
- `+r` → error (matches bash: readonly cannot be removed).
- `-f` / `-F` → list functions by name (bodies deferred).
- `-p` → print declarations of named (or all) vars.

After v64:

- `declare NAME=val` works like a bare assignment (or `local`
  inside a function).
- `declare -r NAME=val` is `readonly NAME=val`.
- `declare -x NAME=val` is `export NAME=val`.
- `declare +x NAME` un-exports.
- `declare -rx NAME=val` combines.
- `declare -p [NAME ...]` prints `declare ATTR NAME="value"`
  lines.
- Bare `declare` lists all variables (sorted) in the same form.
- `declare -F` lists function names (one per line as
  `declare -f NAME`).
- `declare` and `typeset` dispatch to the same builtin.
- Inside a function, default (no `-g`) scope is local — uses
  v52's `local_scopes` snapshot mechanism so attribute changes
  unwind on function exit.

New tracked divergence: **M-79: `declare` / `typeset`**.

## Scope decisions (locked via AskUserQuestion)

**Tier A** — wire to existing infrastructure only. Defer all
attributes that require new Variable-model surface:
- `-i` (integer coercion on assignment).
- `-l` (lowercase on assignment).
- `-u` (uppercase on assignment).
- `-a` (indexed array; huck has no arrays).
- `-A` (associative array; huck has no arrays).
- `-n` (nameref).
- `-g` (force global from inside function).

For now any of these deferred flags → "not yet implemented" + status 1.

## Out of scope (deferred)

- Function-body printing in `-f` output. Bash prints full
  function definitions; huck just prints names with the
  `declare -f NAME` prefix (matches `-F`'s output) since we
  don't have AST pretty-printing.
- All the `-i`/`-l`/`-u`/`-a`/`-A`/`-n`/`-g` attributes (see
  above).

## Architecture

### `Shell::unexport` (`src/shell_state.rs`)

```rust
/// Flips the `exported` flag off on an existing variable. No-op
/// if the variable doesn't exist. Used by `declare +x NAME`.
pub fn unexport(&mut self, name: &str) {
    if let Some(v) = self.vars.get_mut(name) {
        v.exported = false;
    }
}
```

### Local-scope snapshot helper (`src/builtins.rs`)

To match bash's "declare inside a function = local" semantics,
factor out the per-frame idempotent snapshot pattern used by
v52's `builtin_local`:

```rust
/// If we're inside a function call AND `name` hasn't been
/// snapshotted in the current local frame yet, snapshot the
/// current Variable (or None if unset). The unwinding in
/// `call_function` will restore it on function exit. No-op when
/// the local-scopes stack is empty (outside any function).
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

This is essentially the same logic v52's `builtin_local` does
inline. The factored helper can be reused there too if desired
(out of scope for v64).

### `builtin_declare`

```rust
fn builtin_declare(
    args: &[String],
    out: &mut dyn std::io::Write,
    shell: &mut Shell,
) -> ExecOutcome {
    let mut want_readonly = false;
    let mut want_export = false;
    let mut want_remove_export = false;
    let mut function_mode = false;
    let mut function_names_only = false;
    let mut print_mode = false;

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            i += 1;
            break;
        }
        // Bash distinguishes `-X` set-attribute from `+X`
        // remove-attribute. Walk each prefix's byte set.
        let plus = arg.starts_with('+');
        let minus = arg.starts_with('-');
        if !(plus || minus) || arg.len() < 2 {
            break;
        }
        for &c in &arg.as_bytes()[1..] {
            match c {
                b'r' if minus => want_readonly = true,
                b'r' if plus => {
                    eprintln!(
                        "huck: declare: +r: readonly attribute cannot be removed"
                    );
                    return ExecOutcome::Continue(1);
                }
                b'x' if minus => want_export = true,
                b'x' if plus => want_remove_export = true,
                b'f' if minus => function_mode = true,
                b'F' if minus => {
                    function_mode = true;
                    function_names_only = true;
                }
                b'p' if minus => print_mode = true,
                b'i' | b'l' | b'u' | b'a' | b'A' | b'n' | b'g' if minus => {
                    eprintln!(
                        "huck: declare: -{}: not yet implemented in this version",
                        c as char
                    );
                    return ExecOutcome::Continue(1);
                }
                other => {
                    let sign = if plus { '+' } else { '-' };
                    eprintln!("huck: declare: {sign}{}: invalid option", other as char);
                    return ExecOutcome::Continue(2);
                }
            }
        }
        i += 1;
    }
    let names = &args[i..];

    // Function-mode listing.
    if function_mode {
        return declare_list_functions(names, function_names_only, out, shell);
    }

    // Bare or -p with no names: list everything.
    if names.is_empty() {
        return declare_list_all_vars(out, shell);
    }

    // Per-name processing.
    let mut exit: i32 = 0;
    for arg in names {
        let (name, value): (&str, Option<String>) = match arg.find('=') {
            Some(eq) => (&arg[..eq], Some(arg[eq + 1..].to_string())),
            None => (arg.as_str(), None),
        };
        if !is_valid_name(name) {
            eprintln!("huck: declare: `{arg}': not a valid identifier");
            exit = 1;
            continue;
        }

        if print_mode {
            match shell.snapshot_var(name) {
                Some(var) => {
                    let _ = writeln!(out, "{}", format_declare_line(name, &var));
                }
                None => {
                    eprintln!("huck: declare: {name}: not found");
                    exit = 1;
                }
            }
            continue;
        }

        // For any mutation (readonly/export/un-export/plain set),
        // first record the pre-state for function-scope unwinding.
        snapshot_for_local_scope(shell, name);

        if want_readonly {
            if let Some(v) = value.as_ref() {
                if shell.is_readonly(name) {
                    eprintln!("huck: declare: {name}: readonly variable");
                    exit = 1;
                    continue;
                }
                shell.set(name, v.clone());
            }
            shell.mark_readonly(name);
            // -r and -x can combine. Fall through to handle -x too
            // if requested.
        }

        if want_export {
            // -x with value: error if name is readonly (matches
            // v54's `export` builtin).
            if value.is_some() && shell.is_readonly(name) && !want_readonly {
                eprintln!("huck: declare: {name}: readonly variable");
                exit = 1;
                continue;
            }
            match (&value, want_readonly) {
                (Some(v), false) => shell.export_set(name, v.clone()),
                (_, true) => {
                    // Already set via the -r branch; just flip the
                    // export bit too without value-mutation.
                    shell.export(name);
                }
                (None, false) => shell.export(name),
            }
            continue;
        }

        if want_readonly {
            // Already handled above; nothing else to do.
            continue;
        }

        if want_remove_export {
            shell.unexport(name);
            continue;
        }

        // Bare `declare NAME=val` (or just `declare NAME`).
        match value {
            Some(v) => {
                if shell.try_set(name, v).is_err() {
                    eprintln!("huck: declare: {name}: readonly variable");
                    exit = 1;
                }
            }
            None => {
                // `declare NAME` (no value): inside a function the
                // snapshot above is enough — when the function exits
                // the variable is restored (or unset). Outside,
                // bash just declares the name but doesn't create
                // (`declare X` with X unset: X stays unset, exit 0).
                // No action needed beyond the snapshot.
            }
        }
    }
    ExecOutcome::Continue(exit)
}
```

### `declare_list_all_vars`

```rust
fn declare_list_all_vars(
    out: &mut dyn std::io::Write,
    shell: &Shell,
) -> ExecOutcome {
    let mut names: Vec<&String> = shell.vars.keys().collect();
    names.sort();
    for name in names {
        let var = &shell.vars[name];
        let _ = writeln!(out, "{}", format_declare_line(name, var));
    }
    ExecOutcome::Continue(0)
}
```

(Requires `shell.vars` to be accessible. Since v52 made `Variable` `pub` but `vars` is private, we either need a public iterator method on `Shell` or to add one. Adding `pub fn iter_vars(&self) -> impl Iterator<Item = (&String, &Variable)>` is cleaner.)

### `declare_list_functions`

```rust
fn declare_list_functions(
    names: &[String],
    _names_only: bool, // -F vs -f: both names-only in v64; body deferred
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

### `format_declare_line` + escaper

```rust
fn format_declare_line(name: &str, var: &Variable) -> String {
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
```

### Dispatch + `BUILTIN_NAMES`

Append `"declare"` and `"typeset"` to `BUILTIN_NAMES`. Neither
in `is_special_builtin` (bash-specific, regular).

Dispatch:

```rust
"declare" | "typeset" => builtin_declare(args, out, shell),
```

### `Shell::iter_vars` helper (if needed)

```rust
pub fn iter_vars(&self) -> impl Iterator<Item = (&String, &Variable)> {
    self.vars.iter()
}
```

Or just make `vars` field `pub(crate)`. Either works.

## Behavior table

| Input | Behavior |
|---|---|
| `declare` (no args, no flags) | List all vars sorted, `declare ATTR NAME="value"` form |
| `declare X=hi` | Set X="hi"; inside function, local-scoped + restored on return |
| `declare -r X=hi` | Set X="hi" + mark readonly |
| `declare -x X=hi` | Set X="hi" + mark exported |
| `declare -rx X=hi` | Set X="hi" + readonly + exported |
| `declare +x X` | Un-export X (X must exist; no-op if not) |
| `declare +r X` | "readonly attribute cannot be removed" + exit 1 |
| `declare -p X` | Print `declare ATTRS X="value"` (or "not found" + exit 1) |
| `declare -p` (no names) | Same as bare `declare` (list all) |
| `declare -F` | List all functions as `declare -f NAME` per line |
| `declare -F myfn` | `declare -f myfn` if exists, else "not found" + exit 1 |
| `declare -f` | Same as `-F` for v64 (bodies deferred) |
| `declare 1foo=val` | "not a valid identifier" + exit 1 |
| `declare -i X` | "not yet implemented in this version" + exit 1 |
| `declare -X` (unknown) | "invalid option" + exit 2 |
| `typeset -rx X=v` | Same as `declare -rx X=v` (alias) |
| Inside function: `declare X=val` | Like `local X=val`; X restored on return |
| Inside function: `declare -r X=val` | Local X = val, readonly; both unwound on return |

## Test plan

### Unit tests in `src/builtins.rs::mod declare_tests` (14 tests)

Outside function:

1. `declare_bare_sets_var` — `declare X=hi` → X="hi".
2. `declare_r_sets_and_locks` — `declare -r X=hi` → X="hi" + readonly.
3. `declare_x_sets_and_exports` — `declare -x X=hi` → X="hi" + exported.
4. `declare_rx_combines` — `declare -rx X=hi` → X="hi" + readonly + exported.
5. `declare_plus_x_unexports` — pre-set + export X; `declare +x X` → X still exists with same value, but exported=false.
6. `declare_plus_r_errors` — `declare +r X` → exit 1 + stderr.
7. `declare_p_prints_known_var` — pre-set X="hi"; `declare -p X` → stdout starts with `declare -- X="hi"`.
8. `declare_p_missing_errors` — `declare -p __no_such__` → exit 1 + stderr.
9. `declare_f_lists_functions` — register `fn1`, `fn2`; `declare -f` → "declare -f fn1\ndeclare -f fn2\n" sorted.
10. `declare_F_named_function_found` — `declare -F fn1` → "declare -f fn1\n", exit 0.
11. `declare_F_named_function_missing` — `declare -F fn_none` → exit 1.
12. `declare_invalid_identifier` — `declare 1foo=bar` → exit 1.
13. `declare_typeset_alias` — `typeset -r X=hi` → X="hi" + readonly.
14. `declare_deferred_flag_errors` — `declare -i X=5` → exit 1 + stderr "not yet implemented".

(Per-frame snapshot tested implicitly via the existing v52 local tests since our integration is through `snapshot_for_local_scope`; could add an executor-level test for "declare inside function = local" but covered by integration tests instead.)

### Integration tests in `tests/declare_integration.rs` (8 tests)

1. `declare_bare_assigns` — `declare X=hi; echo $X` → "hi".
2. `declare_p_prints_decl` — `X=hi; declare -p X` → "declare -- X=\"hi\"".
3. `declare_r_is_readonly` — `declare -r X=hi; X=new; echo "rc=$?"` → rc=1 (readonly).
4. `declare_x_exports` — `declare -x X=hi; env | grep X=hi` → finds it (after env is implemented... wait, no `env` builtin. Use `printf` or just check the var is exported via downstream child).
5. `declare_plus_x_unexports` — `declare -x X=hi; declare +x X; declare -p X` → output has no `x` attr.
6. `declare_inside_function_is_local` — `f() { declare X=in; }; f; echo "[$X]"` → "[]" (X unset after function).
7. `declare_F_lists_functions` — `f() { :; }; declare -F` → contains "declare -f f".
8. `typeset_alias_works` — `typeset -r X=hi; X=new; echo "rc=$?"` → rc=1.

For test 4 (export check), instead of relying on a child process, just verify via `declare -p X` after `declare -x`:
- `declare -x X=hi; declare -p X` → "declare -x X=\"hi\"" (the `x` attr is in the output).

That's cleaner — no dependency on child-process env inspection.

Revised test 4:

4. `declare_x_is_exported` — `declare -x X=hi; declare -p X` → stdout has `declare -x X="hi"`.

### Smoke

`cargo test --all-targets` green (PTY flake tolerated).

## Implementation tasks

1. **Foundation + builtin + 14 unit tests** — `src/shell_state.rs`
   (add `unexport` method; `iter_vars` if needed); `src/builtins.rs`
   (`snapshot_for_local_scope`, `format_declare_line`,
   `escape_double_quote_value`, `declare_list_all_vars`,
   `declare_list_functions`, `builtin_declare`; dispatch arms
   for both `"declare"` and `"typeset"`; `mod declare_tests`).

2. **Integration tests** — `tests/declare_integration.rs` with
   8 scenarios.

3. **Docs** — M-79 entry; change-log; README v64 row.

## Acceptance criteria

- 14 unit tests pass.
- 8 integration tests pass.
- `cargo test --all-targets` green (PTY flake tolerated).
- `cargo clippy --all-targets -- -D warnings` clean.
- `declare` and `typeset` both regular (NOT in `is_special_builtin`).
- Tier A flags work: `-r`, `-x`, `+x`, `-f`, `-F`, `-p`,
  cluster combinations.
- Deferred flags (`-i`/`-l`/`-u`/`-a`/`-A`/`-n`/`-g`) produce
  "not yet implemented" + exit 1.
- Inside a function, default scope is local (via snapshot
  reuse).
- M-79 doc entry added.
