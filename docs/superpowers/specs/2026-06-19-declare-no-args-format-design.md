# v190: bare `declare`/`typeset` (no-args) output format — Design

**Status:** approved 2026-06-19
**Iteration:** v190
**Origin:** Found by the v188 coverage sweep (the untested `declare_list_all_vars`).
bash's bare `declare` (no options/args) lists variables as **`name=value`** then
all function definitions; huck routes BOTH bare `declare` and `declare -p` to
`declare_list_all_vars`, which always emits the `declare -p` form
(`declare -- x="1"`) and never lists functions.

## bash contract (verified)

Bare `declare` / `typeset` (no args) prints, in order:

1. **All variables, sorted by name**, as `name=value` — NO `declare -X` prefix,
   NO attribute flags:
   - **scalar / integer / exported / readonly / nameref** → `name=<Q>` where `<Q>`
     is bash's *minimal* shell-quoting of the value (the `set -x` / variable-listing
     style — NOT `${v@Q}`, which always quotes):

     | value | bare-declare output |
     |---|---|
     | `hello` | `name=hello` (bare) |
     | `` (empty) | `name=` (bare, NOT `''`) |
     | `a b` / `x;y` / `gl*ob` / `d$ollar` / `back\`tick` / `bang!x` / `lt<gt>` / `br[ack]` | `name='…'` (single-quoted) |
     | `qu'ote` | `name='qu'\''ote'` (`'` → `'\''`) |
     | `ti~lde` / `eq=ual` / `hash#x` | `name=ti~lde` etc. (bare — `~`,`=`,`#` are not metacharacters here) |
     | control char (tab) | `name=$'…\t…'` (ANSI-C `$'…'`) |

     Verified identical to bash's `set -x` (xtrace) quoting for every non-empty
     case; differs only for the empty value (xtrace shows `name=` too, but huck's
     `xtrace_quote("")` returns `''`, so a declare-specific quoter is needed).
   - **indexed array** → `name=([0]="v0" [1]="v1" …)` — byte-identical to huck's
     existing `declare -p` array rendering minus the `declare -a ` prefix
     (verified: huck's indexed `-p` already matches bash).
2. **All functions, sorted by name**, in the `name () { … }` form (same as
   `declare -f`). huck's function bodies use its normalized serialization (the
   pre-existing **M-121** divergence — `declare -f` is not byte-identical to
   bash's pretty-printer either), so bare `declare` is byte-identical to bash for
   VARIABLES but not for function BODIES.

`declare -p` (no args) is UNCHANGED — it already matches bash and must keep the
`declare -- name="value"` form (and must NOT list functions).

## Design

A builtins-only formatting change (`src/builtins.rs`); no AST/parser/expand change.

### 1. Thread the bare-vs-`-p` distinction

`declare_list_all_vars` gains a `bare: bool` parameter. Its two callers
(`builtin_declare` ~line 1241, `builtin_declare_decl` ~line 2177) pass
`bare = !print_mode` (the `-p` flag is already parsed as `print_mode`). When
`bare`, emit the new format + functions; when `-p`, keep the current behavior
(`format_declare_line`, no functions).

### 2. `format_declare_bare_line(name, var)` (new, beside `format_declare_line`)

- **scalar/integer/exported/readonly/nameref** → `format!("{name}={}",
  declare_scalar_quote(s))`. (An unbound nameref with empty value → `name=` —
  same empty handling.)
- **indexed/associative array** → `format!("{name}{}", render_declare_value_part(var))`,
  reusing the array RHS renderer (see §4).

### 3. `declare_scalar_quote(v)` (new) — bash's variable-listing quoting

```rust
fn declare_scalar_quote(v: &str) -> String {
    if v.is_empty() {
        return String::new();                       // name=  (bare empty)
    }
    if v.chars().any(|c| c.is_control()) {
        return crate::param_expansion::ansi_c_quote(v);   // $'…'
    }
    if crate::param_expansion::contains_shell_metas(v) {  // needs pub(crate)
        return format!("'{}'", escape_alias_value(v));    // '…' with ' -> '\''
    }
    v.to_string()                                   // bare
}
```
Reuses the existing primitives `ansi_c_quote` (already `pub(crate)`),
`escape_alias_value` (already `pub(crate)`), and `contains_shell_metas`
(`src/param_expansion.rs:296`, made `pub(crate)`). This is bash's
`sh_contains_shell_metas` + `sh_single_quote`, the same primitive bash uses for
both `set -x` and bare-declare/`set` variable listings.

### 4. Factor the array RHS renderer (DRY)

Extract the `value_part` match arm of `format_declare_line` into
`fn render_declare_value_part(var) -> String` returning the `=…` suffix
(`="value"` for scalar, `=([k]="v" …)` for arrays). `format_declare_line` keeps
using it for all types; `format_declare_bare_line` uses it ONLY for the array
arms (scalars take the minimal-quote path instead).

### 5. Function listing in bare mode

After the sorted variables, when `bare`, call the existing
`declare_list_functions(&[], false, out, shell)` (sorted, full `f () {…}` form).
`typeset` shares the path, so it is fixed identically.

## Verification

- **New bash-diff harness** `tests/scripts/declare_no_args_diff_check.sh`
  (byte-identical). To filter out the noisy inherited environment, each case sets
  specific `z*` test variables and greps `declare` output to `^z`:
  - scalar battery (bare/spaced/`;`/glob/`$`/backtick/`!`/`<>`/`[]`/quote/`~`/`=`/`#`/empty/tab)
    — the table above, byte-identical to bash.
  - indexed array (`za=(p "q r" "")`), integer (`declare -i zi=42`), exported
    (`export ze=x`), readonly (`readonly zr=c`).
  - **regression guard**: `declare -p z*` for the same vars stays byte-identical
    to bash (the `-p` path must not change).
  - `typeset` (no args) behaves identically to `declare`.
  - function listing tested STRUCTURALLY (a `declare`-with-a-function run asserts
    the `zf () ` header line appears) — bodies are not byte-compared (M-121).
- **Unit tests** for `declare_scalar_quote` (each battery row) and
  `format_declare_bare_line` (scalar, indexed array).
- **Up-front grep** `tests/` + `src/` for tests asserting bare `declare`
  currently emits the `declare --` form; update them to the new `name=value` form
  (verify vs bash). The `declare -p` tests must stay green unchanged.
- Full `cargo test` (0 failures); all harnesses + clippy green.

## Docs / close-out

Check `docs/bash-divergences.md` for an existing `declare` entry (likely none —
coverage-found). Record v190 in `project_huck_iterations.md` + `MEMORY.md`; update
the coverage-divergence note (declare-no-args RESOLVED; `bind -p` still pending).
Log the deferred follow-on: **associative-array `declare -p`/bare rendering** —
huck always double-quotes the key (`["k"]`) and omits bash's trailing space
(`[k]="1" )`); a separate pre-existing `-p`-path divergence inherited by bare.

## Scope boundary

In scope: the bare-vs-`-p` distinction; `format_declare_bare_line` +
`declare_scalar_quote` (scalars) + indexed-array reuse; function listing in bare
mode; `typeset` parity; keeping `declare -p` unchanged. **Not** in scope: the
associative-array key-quoting/trailing-space divergence (deferred, shared `-p`
renderer); M-121 function-body pretty-printing; the `set` builtin's listing; any
`declare`-with-names behavior.
