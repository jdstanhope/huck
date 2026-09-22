# v365 — declared-but-unset variables

**Date:** 2026-09-22
**Issues:** [#600](https://github.com/jdstanhope/huck/issues/600) (primary),
[#225](https://github.com/jdstanhope/huck/issues/225),
[#33](https://github.com/jdstanhope/huck/issues/33),
[#691](https://github.com/jdstanhope/huck/issues/691),
[#692](https://github.com/jdstanhope/huck/issues/692),
[#777](https://github.com/jdstanhope/huck/issues/777).
Hub 2A of the tech-debt roadmap ([#765](https://github.com/jdstanhope/huck/issues/765),
`docs/superpowers/plans/2026-09-17-tech-debt-hubs-roadmap.md`).

## The gap

bash's variable is a set of attributes plus an **optional** value
(`SHELL_VAR.value == NULL`). huck's `VarValue` is `Scalar | Indexed |
Associative` with no null variant, so a declaration that carries no value has
nowhere to put "no value" and materialises an empty one instead. Nine measured
rows diverge (#600), plus the same root seen from `readonly` (#225),
`declare -p` (#33), and `local` (#691) — and the export set, which cannot be
right without the null state (#692).

## The model

Measured against bash 5.2.21; every rule below is a probe, not an inference.

### Creation

`declare -a/-A/-i/-l/-u/-n/-r/-x NAME`, bare `declare NAME` and bare `local
NAME` create the entry with attributes and **no value**. So do the attribute
mutators reached by `readonly NAME` and `export NAME` on a name that does not
exist yet.

### Becoming set

Any assignment sets the value: `v=`, `v=x`, `y[2]=x`, `y+=(a)`, `n+=1`,
`printf -v`, `read`, a `${v:=default}` assignment-expansion. There is no other
way in. `unset` removes the entry outright (it does not return it to the unset
state — a later `declare -p` says `not found`). An attribute change leaves the
value alone: `declare -i n; declare +i n; declare -x n` is unset throughout.

### While unset

| read | answer |
| --- | --- |
| `$v`, `${v}`, `${v:-D}`, `${v-D}` | as if the name did not exist |
| `${v+set}` | empty |
| `[[ -v v ]]`, `test -v v` | false |
| `declare -p v` | the declaration with no `=` (`declare -a y`) |
| `${v@A}` | the same declaration text |
| `set`, `compgen -v` | the name is **not** listed |
| `export -p`, `readonly -p` | listed (attributes are real) but with no `=` |
| `set -u` + scalar read (`$n`) | `n: unbound variable`, rc 1 |
| `set -u` + `${y[@]}` / `${y[*]}` / `${#y[@]}` | empty / `0` — **no** error |
| a child's environment | contributes nothing |

### The shape is an attribute, not the value

`declare -a y` with no value is still an indexed array for conversion
purposes: `declare -a y; declare -A y` remains `cannot convert indexed to
associative`. The unset state therefore carries the shape.

### The export set: two different questions

Separating these was the design's one genuine discovery; they had to be
measured apart.

1. **`export -p` / `declare -x` listings** report the innermost **visible**
   binding, if it is exported. With an exported global `V=1`, inside
   `f(){ local +x V=2; }` `export -p` does not list `V` at all.
2. **A child's environment** takes, per name, the innermost binding that is
   exported **and has a value**. That same child sees `V=1`: the walk passes
   over the unexported local to the outer binding. Assigning `V=3` to the
   unexported local does not change that — the child still sees `V=1`.

A bare `local V` over an exported outer inherits the export attribute (#691)
but has no value, so rule 2 walks past it as well and the child again sees
`V=1`. `export V` inside the function exports the local, and the child then
sees the local's value.

## Representation

`VarValue` gains `Unset(Shape)`, where `Shape` is `Scalar | Indexed |
Associative`. This mirrors bash and makes the compiler enumerate every read
site. The alternative — keeping a materialised empty value behind an
`assigned: bool` — was rejected: it leaves reads untouched but makes a missed
**write** path silently report a live variable as unset, where the chosen
representation makes a missed write loud (the variable stays unset and
`declare -p` shows it).

## Surface

`lookup_var` already returns `Option<String>`, and `VarValue` is reachable
only through a few accessors, so the null state is invisible to ordinary
readers: all 63 `lookup_var` callers are unchanged, because an unset variable
answers `None` exactly as a nonexistent one does. The sites that must
distinguish "no entry" from "entry without value":

| Site | Change |
| --- | --- |
| `VarValue::Unset(Shape)`, `Variable::shape()` | new variant; `scalar_view` → `""` (its 12 callers unaffected) |
| `lookup_var`, `get` | `None` when unset — the edit that makes most readers correct for free |
| `is_set` (`[[ -v ]]`, `test -v`) | false when unset; element forms (`x[k]`) still consult the map |
| nounset | scalar read raises; `${y[@]}` / `${y[*]}` / `${#y[@]}` stay quiet |
| `format_declare_line`, `format_declare_bare_line`, `${v@A}` | no `=…` when unset |
| `set` / `compgen -v` listings | skip unset entries |
| `exported_env` | skip unset entries, then walk outward to the innermost exported **and valued** binding (#692) |
| `assign`, `set_indexed_element`, `extend_indexed`, `append_indexed_element`, `set_associative_element`, `append_associative_element`, `replace_indexed` | materialise the shape on first write, through one shared `set_value` helper |
| `declare` / `local` declaration paths | create `Unset(shape)`; a bare `local V` inherits only `exported` (#691); `local` accepts `-x` (#777) |
| `mark_readonly`, `mark_integer`, `set_case_fold`, nameref | create or update without materialising a value (#225, #33) |
| `unset` | unchanged — removes the entry |

`readonly r` with no value creates a readonly unset variable, so a later
`r=1` must still be refused. huck's readonly guard lives in `Shell::assign`,
which this iteration edits, so the harness pins it.

## Testing

**Gate:** a new `tests/scripts/declared_unset_diff_check.sh`, every row under
both `-c` and a script file:

- #600's nine rows, plus `declare -l` / `-n` / `-r` and bare `declare v`.
- The set/unset boundary in both directions: `${v+set}`, `${v-unset}`,
  `[[ -v v ]]`, `${#y[@]}`, `${y[*]}` before and after `v=`, `y[2]=x`,
  `y+=(a)`, `n+=1`.
- `set -u`: scalar raises; `${y[@]}` / `${y[*]}` / `${#y[@]}` do not.
- `declare -p`, `${v@A}`, `set`, `compgen -v`, `export -p`, `readonly -p`
  while unset.
- `unset` after a bare declare → `declare: y: not found`, rc 1.
- Shape survives with no value (`declare -a y; declare -A y` refuses).
- Attribute churn keeps it unset (`declare -i n; declare +i n; declare -x n`).
- `readonly r` with no value, then `r=1` → refused.
- Export set: `local +x V=2`, bare `local V`, the nested `local V=3` case,
  `V=3` after `local +x`, `export V` inside, and the row where `export -p`
  lists nothing while the child still sees the outer value.
- `local -x L=5` and `local -x` over an exported outer (#777).

**Second gate — the pinned tests.** 33 assertions across
`declare_integration.rs`, `declare_integer_integration.rs`, `builtin_vars.rs`,
`printf_v_array_integration.rs` and the `shell_state` unit modules currently
expect `declare -a y=()`, `declare -i n=""` or a materialised local. Each is a
pinned divergence and each must be **re-measured against bash and adjudicated
individually** — not bulk-replaced. An assertion that mentions `=()` while
testing something else stays untouched.

**Structural invariant.** A table-driven unit test runs every value mutator
against an `Unset` variable and asserts the variable is set afterwards with
the right shape. This is what turns a missed write path into a failure rather
than a wrong answer.

**Sweep, clippy (pinned 1.97.1) and the per-crate suites** run before the PR.

## Out of scope

- The array/assoc conversion table (#347, #697, #698, #734) — hub 2B. This
  iteration only makes `Unset` a shape that table can see.
- Byte-transparent values (#738) — hub 8, the second `VarValue` pass.
- `${v@a}` attribute queries — they read attributes, not values.

## Rollout

One `v365-declared-unset` branch, subagent per task, code-quality review
between tasks, whole-branch review before the PR. Task order: `Unset` variant
and accessors → declaration paths → readers → export set and `local -x` →
harness → test adjudication.
