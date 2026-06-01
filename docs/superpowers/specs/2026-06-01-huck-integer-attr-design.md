# huck v65 — `declare -i` integer attribute (M-79 cont.)

## Goal

Add the `integer` variable attribute, finishing the `-i` row
that v64's M-79 listed as deferred. With this attribute on a
variable, subsequent assignments evaluate the RHS as an
arithmetic expression and store the integer result.

After v65:

- `declare -i X=2+3` → X="5".
- `declare -i X; X=10*5` → X="50".
- `declare -i X; X=abc` → X="0" (silent coercion on failure).
- `declare +i X` → removes the integer attribute.
- `declare -p X` (when X is integer-flagged) → `declare -i X="..."`.
- Affects all standard write paths: top-level assignment, inline
  prefix, for-loop iter, `${var:=...}`, `read`, arithmetic
  assignment, `declare -i`.
- Marking an existing variable with `-i` does NOT re-evaluate
  its current value (matches bash). Only future writes coerce.

This completes another row from M-79's deferred list.

## Scope decisions

**Match bash's silent-coercion model**: when the RHS fails to
parse/evaluate as arithmetic, store `"0"` silently (no error
message). This is what bash does for every code path EXCEPT
`declare -i NAME=value`, which bash treats specially with a
loud error. For simplicity, v65 silently coerces in ALL paths
including `declare -i NAME=value`. Documented bash divergence
in M-79 update.

## Out of scope (deferred)

- Loud error reporting when `declare -i NAME=value` can't parse
  the RHS (bash prints "syntax error: operand expected"). Could
  layer on later; not a behavior bug, just a missing
  diagnostic.
- `let` builtin (bash's arithmetic-evaluation-only builtin
  closely related to `-i`). Separate iteration.
- Compound-assignment operators (`X+=5`, `X*=2`) that bash
  routes through arith. v65's coverage is base-form `X=expr`
  via existing `try_set` channels.

## Architecture

### `Variable.integer` flag (`src/shell_state.rs`)

```rust
#[derive(Debug, Clone)]
pub struct Variable {
    pub value: String,
    pub exported: bool,
    pub readonly: bool,
    pub integer: bool,   // NEW (defaulted false in all existing literals)
}
```

All existing `Variable { … }` constructions need a `integer:
false` field added. Should be small (handful of literals).

### New `Shell` methods

```rust
/// True iff `name` is currently set AND marked integer.
pub fn is_integer(&self, name: &str) -> bool {
    self.vars.get(name).map(|v| v.integer).unwrap_or(false)
}

/// Sets the integer flag on `name`. If unset, creates an empty
/// integer-flagged Variable (mirrors mark_readonly's behavior).
/// Does NOT re-evaluate the existing value — matches bash:
/// future writes evaluate, current value is preserved.
pub fn mark_integer(&mut self, name: &str) {
    if let Some(v) = self.vars.get_mut(name) {
        v.integer = true;
    } else {
        self.vars.insert(name.to_string(), Variable {
            value: String::new(),
            exported: false,
            readonly: false,
            integer: true,
        });
    }
}

/// Removes the integer flag. No-op if name unset.
pub fn unmark_integer(&mut self, name: &str) {
    if let Some(v) = self.vars.get_mut(name) {
        v.integer = false;
    }
}
```

### Extending `try_set` (`src/shell_state.rs`)

This is the load-bearing change. v54's `try_set` checked
readonly. v65 extends it to evaluate via `arith` when the
target is integer-flagged:

```rust
pub fn try_set(&mut self, name: &str, value: String) -> Result<(), ()> {
    if self.is_readonly(name) {
        return Err(());
    }
    let final_value = if self.is_integer(name) {
        eval_integer_coerce(self, &value)
    } else {
        value
    };
    self.set(name, final_value);
    Ok(())
}

fn eval_integer_coerce(shell: &mut Shell, value: &str) -> String {
    match crate::arith::parse(value) {
        Ok(expr) => match crate::arith::eval(&expr, shell) {
            Ok(n) => n.to_string(),
            Err(_) => "0".to_string(),
        },
        Err(_) => "0".to_string(),
    }
}
```

The `set` underneath stays unchecked — internal mechanism
writes (cd updating PWD, signal-state, etc.) still bypass the
integer check. User-facing writes ALL go through `try_set` (per
v54), so they get integer-coercion for free.

Borrow-checker note: `arith::eval` takes `&mut Shell`. Inside
`try_set` we have `&mut self` and pass that to the helper, which
in turn passes it to `arith::eval`. The `value: String` is owned
so its parse-result (`ArithExpr`) doesn't borrow it. Sequenced
borrows: borrow self for eval → drop → borrow self for set.
Compiles cleanly.

### `builtin_declare` extension

In the flag parser, currently `-i`/`-l`/`-u`/`-a`/`-A`/`-n`/`-g`
all emit "not yet implemented". Remove `b'i'` from that arm and
add a real handler:

```rust
let mut want_integer = false;
let mut want_remove_integer = false;
// ... in the flag-cluster loop:
b'i' if minus => want_integer = true,
b'i' if plus => want_remove_integer = true,
// ... etc.
```

Per-name processing:

- `declare -i X` (no value): `snapshot_for_local_scope` →
  `shell.mark_integer(name)` → done. Existing value preserved.
- `declare -i X=val`: snapshot → mark_integer → `shell.try_set`
  routes through the integer-eval path → stored as the
  evaluated integer.
- `declare +i X`: snapshot → `shell.unmark_integer(name)`.
- `-i` combines with `-r` and `-x`: same pattern as
  readonly+export. Apply mark_integer alongside the other
  marks; value is set once via the readonly path or directly.

### `format_declare_line` update

Add `i` to the attribute string when the integer flag is set:

```rust
fn format_declare_line(name: &str, var: &Variable) -> String {
    let mut attrs = String::new();
    if var.readonly { attrs.push('r'); }
    if var.exported { attrs.push('x'); }
    if var.integer { attrs.push('i'); }
    // ...
}
```

So `declare -i X=42` produces `declare -i X="42"`; `declare -ir X=42`
produces `declare -ir X="42"` (alphabetical order: i before r
in bash? actually bash uses `-ir`, `-ix`, etc. — order is the
flags' letters but order of i/r/x in display isn't strictly
specified; we'll use r, x, i which is the order we add them).

Actually bash's display order for `declare -p`:
```
$ declare -ix X=5
$ declare -p X
declare -ix X="5"
```

Bash uses `i` then `x`. Let me check more:
```
$ declare -rix X=5
$ declare -p X
declare -irx X="5"
```

Bash puts them in `irx` order. So `i` first, then `r`, then `x`.

Update the formatter to match:

```rust
fn format_declare_line(name: &str, var: &Variable) -> String {
    let mut attrs = String::new();
    if var.integer { attrs.push('i'); }
    if var.readonly { attrs.push('r'); }
    if var.exported { attrs.push('x'); }
    // ...
}
```

(Reorders existing v64 logic — minor visible change but consistent with bash.)

## Behavior table

| Input | Behavior |
|---|---|
| `declare -i X=2+3` | X="5" + integer flag |
| `declare -i X; X=10*5` | X="50" |
| `declare -i X; X=abc` | X="0" (silent coerce) |
| `declare +i X` (was integer) | flag removed; value unchanged |
| `declare -i X=Y+5` (Y=3) | X="8" |
| `declare -p X` (X integer + value=5) | `declare -i X="5"` |
| `declare -p X` (X integer + readonly + value=5) | `declare -ir X="5"` |
| `read X` (X integer-flagged, input "2+3") | X="5" |
| `for X in 2+3 7-1; do done` (X integer) | iter values "5", "6" |
| `${X:=2+3}` (X integer + unset) | X="5"; expansion result "5" |
| `declare -i RO_X` (X readonly first) | mark_integer still works — readonly only blocks WRITES, not flag changes. (Hmm, actually bash errors here. Let me match bash.) |

Actually for the last row, bash's behavior:
```
$ readonly X=10
$ declare -i X
bash: declare: X: readonly variable
```

So bash errors when you try to add the integer attribute to a readonly var. Let me match: `declare -i NAME` when NAME is readonly → error + exit 1.

Same logic applies to `declare -x` on readonly when value isn't being set — wait, v64 actually allows `declare -x` to flip the export bit on a readonly. Bash behavior here is inconsistent across attributes:
- `declare -x` on readonly (no value): bash allows it.
- `declare -i` on readonly (no value): bash errors.
- `declare -r` on readonly (no value): bash silently idempotent.

So: integer flag transition on a readonly variable is denied. Match by adding a check in builtin_declare's `-i` no-value path.

Updated behavior table row:
| `declare -i X` (X is readonly) | "readonly variable" + exit 1 |

## Test plan

### Unit tests in `src/shell_state.rs::tests` (4 try_set tests)

Skip — most v54/v52 tests are in builtins.rs. Just add the try_set tests in builtins.rs::declare_tests or a new module.

### Unit tests in `src/builtins.rs::mod integer_attr_tests` (10 tests)

1. `try_set_non_integer_passes_through` — `Shell::new`, `try_set("X", "2+3")` → value stored as "2+3".
2. `try_set_integer_simple_arith` — `mark_integer("X")` then `try_set("X", "2+3")` → "5".
3. `try_set_integer_negative_result` — same, `"0-5"` → "-5".
4. `try_set_integer_invalid_silently_zero` — `"abc"` → "0".
5. `try_set_integer_with_var_ref` — `set("Y", "10")` + `mark_integer("X")` + `try_set("X", "Y*2")` → "20".
6. `try_set_readonly_checked_before_integer` — `mark_readonly("X")` + `mark_integer("X")` + `try_set("X", "5")` → Err (readonly wins). Value unchanged.
7. `declare_i_marks_and_evals` — `declare -i X=2+3` → X="5" + `is_integer("X")` true.
8. `declare_plus_i_unmarks` — `declare -i X=5; declare +i X` → integer flag false; value preserved "5".
9. `declare_i_existing_var_no_reeval` — `set("X", "2+3"); declare -i X` (no =) → X stays "2+3", `is_integer("X")` true.
10. `declare_i_on_readonly_errors` — `mark_readonly("X")` then `declare -i X` → exit 1 + stderr.

### Integration tests in `tests/declare_integer_integration.rs` (6 tests)

1. `integer_assign_evaluates` — `declare -i X=2+3; echo $X` → "5".
2. `integer_reassign_evaluates` — `declare -i X; X=10*5; echo $X` → "50".
3. `integer_garbage_becomes_zero` — `declare -i X=abc; echo $X` → "0".
4. `integer_p_format` — `declare -i X=42; declare -p X` → "declare -i X=\"42\"".
5. `plus_i_unmarks` — `declare -i X=10; declare +i X; X=2+3; echo $X` → "2+3".
6. `integer_in_for_loop` — `declare -i X; for X in 2+3 7-1; do echo $X; done` → "5\n6\n".

### Smoke

`cargo test --all-targets` green (PTY flake tolerated).

## Implementation tasks

1. **Foundation + try_set extension + declare wiring + 10 unit tests** — modify `src/shell_state.rs` (add `integer` field + 3 helpers + extend `try_set`); modify `src/builtins.rs` (remove `-i` from deferred list; add `-i` and `+i` paths; update `format_declare_line` for `irx` order; readonly-check for `declare -i X`; `mod integer_attr_tests`).

2. **Integration tests** — `tests/declare_integer_integration.rs` with 6 scenarios.

3. **Docs** — update M-79 with v65-ships-i note; add v65 change-log entry; README v65 row.

## Acceptance criteria

- 10 unit tests pass.
- 6 integration tests pass.
- `cargo test --all-targets` green (PTY flake tolerated).
- `cargo clippy --all-targets -- -D warnings` clean.
- All existing tests pass (especially the v54 try_set tests and
  v52 local_scopes tests — extending try_set could break
  something).
- The deferred list in M-79 no longer mentions `-i`; an
  "Updates" sentence covers v65.
