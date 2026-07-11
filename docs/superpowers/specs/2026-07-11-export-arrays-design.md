# v282 — Fix #82 (`export name=(array)` errors) + #28 (exported array leaks `[0]` to child env)

**Issues:** [#82](https://github.com/jdstanhope/huck/issues/82) (bug, divergence, sev:low) and
[#28](https://github.com/jdstanhope/huck/issues/28) (bug, divergence, sev:low). The PR
`Closes #82` and `Closes #28`.

Two small, independent fixes in the "exported arrays" area. They touch different
files and do not interact.

## Problem

### #82 — `export a=(1 2 3)` errors instead of assigning + marking exported

bash accepts an array-literal assignment to `export`: it assigns the indexed
array and sets the export attribute (`declare -ax a`, rc 0). huck rejects it:

```
$ huck -c 'export a=(1 2 3); declare -p a'
huck: line 1: export: cannot export arrays          # rc 1, `a` not created
$ bash -c 'export a=(1 2 3); declare -p a'
declare -ax a=([0]="1" [1]="2" [2]="3")             # rc 0
```

The array-literal parse is already correct; only the `export` builtin's runtime
rejects the assignment.

### #28 — an exported array leaks its `[0]` element into a child's environment

bash never puts an array variable into a child's environment. huck exports the
array's scalar view (the `[0]`/`"0"` element) as a bogus scalar:

```
$ huck -c 'a=(x y z); export a; env huck -c "printenv a; echo rc=\$?"'
x                                                   # huck: child sees a=x
$ bash  -c 'a=(x y z); export a; env bash -c "printenv a; echo rc=\$?"'
                                                    # bash: unset, printenv rc 1
```

This is pre-existing and independent of #82 (it already affects `export b` on an
existing array). Fixing #82 makes `export a=(…)` reach the same exported state,
so both are fixed together here.

## Design

### Part 1 — #82: let array assignments flow through the normal assign+export path

In `builtin_export_decl` (`crates/huck-engine/src/builtins.rs`), the
`DeclArg::Assign(a)` arm currently rejects array-valued assignments:

```rust
            DeclArg::Assign(a) => {
                if assign_value_is_array(a) {
                    crate::sh_error_to!(shell, err, None, "export: cannot export arrays");
                    any_error = true;
                    continue;
                }
                if matches!(&a.target, crate::command::AssignTarget::Indexed { .. }) {
                    // … "not a valid identifier" for `export AA[4]=1` … (KEEP)
                }
                let name = a.target.name().to_string();
                // readonly check …
                if crate::executor::apply_one_assignment(a, shell, err).is_err() { … }
                if unexport { shell.unexport(&name); } else { shell.export(&name); }
            }
```

`apply_one_assignment` already handles array literals — indexed, associative, and
`+=` append — (it is the same path `declare -a`/`local` use for compound RHS).
So the fix is to **remove the array rejection** and let array assignments fall
through to the existing `apply_one_assignment` + `shell.export(name)` path:

- Delete the 5-line `if assign_value_is_array(a) { … cannot export arrays … }`
  block.
- Delete the now-unused helper `assign_value_is_array` (builtins.rs:1314-1322) —
  its only caller is that block, so leaving it triggers a `dead_code` warning.
- Update the `builtin_export_decl` doc comment (currently "Rejects array
  compound-RHS; otherwise mirrors the legacy `builtin_export` behavior …") to
  drop the "Rejects array compound-RHS" clause.

The `export AA[4]=1` → "not a valid identifier" (`AssignTarget::Indexed`) check
and the readonly check are unchanged. Result:

- `export a=(1 2 3)` → assigns the indexed array, sets the export attribute,
  rc 0; `declare -p a` → `declare -ax a=([0]="1" [1]="2" [2]="3")`.
- `export a+=(4 5)` appends (via `apply_one_assignment`'s existing append path).
- `export -a a=(…)` — the `-a` flag is already a no-op; unaffected.

### Part 2 — #28: omit array-typed variables from the exported environment

`exported_env` (`crates/huck-engine/src/shell_state.rs:2998`) is the single seam
that builds a child's environment (3 executor call sites all source from it). It
currently maps every exported var through `scalar_view()`:

```rust
    pub fn exported_env(&self) -> impl Iterator<Item = (&str, &str)> {
        self.vars
            .iter()
            .filter(|(_, v)| v.exported)
            .map(|(k, v)| (k.as_str(), v.value.scalar_view()))
    }
```

`scalar_view()` returns the `[0]` element for an `Indexed`/`Associative` value —
the leak. **Fix:** also filter to only `VarValue::Scalar` values, so arrays are
omitted from the child environment entirely (matching bash):

```rust
    pub fn exported_env(&self) -> impl Iterator<Item = (&str, &str)> {
        self.vars
            .iter()
            .filter(|(_, v)| v.exported)
            // bash never inherits array variables into a child's environment;
            // emit only true scalars (skip Indexed/Associative). See #28.
            .filter_map(|(k, v)| match &v.value {
                VarValue::Scalar(s) => Some((k.as_str(), s.as_str())),
                VarValue::Indexed(_) | VarValue::Associative(_) => None,
            })
    }
```

The export *attribute* (`v.exported`) is untouched, so `declare -ax a` still
displays and `export -p` still lists the array; only the child environment omits
it. (`scalar_view` stays; it is used elsewhere for display.)

## Testing

### Bash-diff harness — `tests/scripts/export_array_diff_check.sh` (new)

Byte-identical bash↔huck fragments (the gold-standard compat check), covering
both halves. Cases (each run through bash and `$HUCK_BIN`, stdout+stderr+rc
compared, with the program-name prefix normalized as in sibling harnesses):

- `export a=(1 2 3); declare -p a` → `declare -ax a=([0]="1" [1]="2" [2]="3")`, rc 0.
- `export a=(1 2 3); echo "rc=$?"` → rc 0 (no "cannot export arrays").
- `a=(x y); export a; declare -p a` (export-then-existing path, already worked) →
  `declare -ax a=…`.
- `export a+=(4 5)` on a pre-set `a=(1 2 3)` → appended array.
- **#28 half** — array NOT inherited: `export a=(x y z); printenv a; echo "rc=$?"`
  → empty (no `a` line), `rc=1` (matching bash). `printenv` is an ordinary
  external command that inherits the parent's environment, so the same fragment
  runs identically under both shells — no need to spawn the shell-under-test as
  the child. (Before the fix, huck's arm prints `x` / `rc=0` — the leak the
  harness catches.)
- **#28 control** — a scalar IS inherited: `export s=hi; printenv s; echo "rc=$?"`
  → `hi`, `rc=0` in both.

Note: each fragment is compared byte-identically between bash and `$HUCK_BIN`
(stdout+stderr+rc), with the leading program-name prefix normalized as in sibling
harnesses.

### Unit tests

- `crates/huck-engine/src/builtins.rs` test module: invoking `export` on an array
  assignment (`export a=(1 2 3)`) creates the `Indexed` value AND sets the export
  bit (rc 0), and does not emit "cannot export arrays".
- `crates/huck-engine/src/shell_state.rs` test module: `exported_env` omits an
  exported `Indexed` (and `Associative`) variable while still emitting an exported
  `Scalar`.

### Suites

- Per-crate: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`
  (single-threaded, box constraint).
- Full diff-check sweep: `tests/scripts/run_diff_checks.sh` stays green (now
  including the new harness).

## Out of scope

- No change to array *display* (`declare -p`/`${var@A}`) or to
  `exported_function_env` (exported functions, a separate seam).
- The child-inheritance of exported functions and other `export` edges
  (#65/#23/#67) are separate issues, untouched.

## Notes

- Both #82 and #28 are real (not intentional) divergences; the merged PR
  auto-closes both (`Closes #82`, `Closes #28`). No `docs/bash-divergences.md`
  entry needed.
- `VarValue` variants: `Scalar(String)`, `Indexed(BTreeMap<usize,String>)`,
  `Associative(Vec<(String,String)>)` (shell_state.rs).
