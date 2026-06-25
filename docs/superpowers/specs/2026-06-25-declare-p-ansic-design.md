# v220 — `declare -p` ANSI-C value quoting

## Status

Design approved 2026-06-25. Small, focused iteration: wire huck's existing
ANSI-C quoter into the `declare -p` value-rendering path so values containing
control characters render as `$'…'`, matching bash 5.2.21. Flips the `herestr`
bash-test-suite category to PASS.

## Background

After v219, the `herestr` category's **entire** remaining residual is a single
hunk: `declare -p` of an array element whose value contains a newline renders as
a double-quoted literal newline, where bash uses ANSI-C `$'…'` quoting.

```
bash: declare -a uu=([0]="" [1]="kghfjk" [2]="jkfzuk" [3]=$'i\n')
huck: declare -a uu=([0]="" [1]="kghfjk" [2]="jkfzuk" [3]="i
")
```

This was confirmed by measuring herestr's full residual via the authoritative
runner path (`THIS_SH` set): exactly this one divergence (3 diff lines, one
logical hunk). Fixing it flips herestr to PASS (PASS count 7→8). The category's
other historical blocker (a runtime `command not found:` from an unset
`THIS_SH`) is harness-masked — the runner exports `THIS_SH=$HUCK` — so it does
not gate the category.

### bash's `declare -p` value-quoting rule (measured)

bash double-quotes a value normally (escaping `"`, `$`, `` ` ``, `\`), but
switches the **whole value** to ANSI-C `$'…'` when it contains a control /
non-printable character:

- Named escapes: `\a` (0x07), `\b` (0x08), `\t` (0x09), `\n` (0x0a), `\v`
  (0x0b), `\f` (0x0c), `\r` (0x0d), `\E` (0x1b — bash uses `\E`, not `\e`).
- `\\` for backslash, `\'` for single-quote (inside `$'…'`).
- Other control bytes (`<0x20`, `0x7f`) → **3-digit octal** `\NNN` (`\001`,
  `\037`, `\177`).
- A lone backslash / `$` / `"` / space alone does NOT trigger `$'…'` — those stay
  in the `"…"` form.
- Valid printable UTF-8 is kept in `"…"` (e.g. `café` → `"café"`); only lone
  invalid high bytes get octal-escaped (locale/`mbrtowc`-aware) — see Out of
  Scope.

### What huck already has

`crate::param_expansion::ansi_c_quote(s) -> String` already produces this exact
format (named escapes incl. `\E`, 3-digit octal for `<0x20`/`0x7f`, `\\`/`\'`,
printable passthrough), and `declare_scalar_quote` (the *bare* `declare` / `set`
form, `builtins.rs:~895`) already calls it for control-bearing values — that
path matches bash. The gap is solely that `render_declare_value_part` (the
`declare -p` value renderer, `builtins.rs:875`) never consults it: all three of
its `VarValue` arms unconditionally emit `="{escape_double_quote_value(v)}"`.

Because Rust `String`s are always valid UTF-8, huck cannot hold the lone-invalid-
high-byte values where bash's locale-aware octal escaping would differ — so for
every value huck can represent, `ansi_c_quote` already matches bash.

## Goals

1. `declare -p` renders a scalar / indexed-element / associative-element value
   that contains a control character as `$'…'` (via the existing `ansi_c_quote`),
   matching bash 5.2.21; control-free values render exactly as today (`"…"`).
2. The `herestr` bash-test-suite category passes (runner diff = 0).
3. No regression: control-free `declare -p` output is byte-identical to before;
   the bare-`declare`/`set` path is untouched (already correct).

## Non-goals / Out of scope (documented)

- **C1 controls (U+0080–U+009F):** `char::is_control()` is true for these, so
  they would trigger the `$'…'` branch, but `ansi_c_quote`'s octal arm only
  covers `<0x20`/`0x7f` — a C1 char would pass through literally inside `$'…'`.
  This is a pre-existing edge in `ansi_c_quote`, not exercised by herestr, and
  matching bash there needs locale-aware printability. Left as-is; noted as a
  low-severity deferred edge.
- **The bare-`declare` / `set` scalar path** (`declare_scalar_quote`): already
  ANSI-C-aware and matching bash; not touched.
- **Consolidating** `ansi_c_quote` (huck-engine) with v219's `ansi_c_escape`
  (huck-syntax/generate.rs, which uses hex not octal): out of scope; v220 stays
  in the `declare -p` path.

## Design

### Component: a declare-p value quoter

Add to `builtins.rs` (next to `render_declare_value_part`):

```rust
/// Quote a value for `declare -p` display. bash double-quotes normally but
/// switches the whole value to ANSI-C `$'…'` when it contains a control
/// character (newline, tab, etc.) — same trigger as `declare_scalar_quote`,
/// so the `-p` and bare forms agree. Returns the FULL quoted token (`"…"` or
/// `$'…'`), without a leading `=`.
fn declare_p_value_quote(s: &str) -> String {
    if s.chars().any(|c| c.is_control()) {
        crate::param_expansion::ansi_c_quote(s)
    } else {
        format!("\"{}\"", crate::escape_double_quote_value(s))
    }
}
```

### Wiring: `render_declare_value_part` (`builtins.rs:875`)

Replace the three `="…"` renderings with `declare_p_value_quote`:

- **Scalar** (`builtins.rs:883`): `format!("={}", declare_p_value_quote(s))`
  (keep the unbound-nameref empty-value special case that returns `""`).
- **Indexed** (`builtins.rs:889`): `format!("[{k}]={}", declare_p_value_quote(v))`.
- **Associative** (`builtins.rs:898`):
  `format!("[{}]={}", quote_subscript_key(k), declare_p_value_quote(v))`.

No other call sites change. The bare-array path (`format_declare_bare_line`,
`builtins.rs:929`) reuses `render_declare_value_part`, so it is covered
automatically.

## Testing / Verification

- **Unit tests** (`builtins.rs` tests module) for `render_declare_value_part` /
  `format_declare_line`:
  - scalar with `\n` → the value renders bare as `$'…'` (e.g. value `i\n` →
    `declare -- v=$'i\n'`, NOT `v="..."`); also `\t`; and a `<0x20` control
    (octal, e.g. 0x01 → `$'\001'`).
  - indexed array with a newline element → `[3]=$'i\\n'`.
  - associative element with a control value → `[k]=$'…'`.
  - **no-regression:** a plain value (`hello`) and a value with `"`/`$`/space →
    unchanged `"…"` form.
  Capture each expected string from `bash -c '…; declare -p v' | cat -A` (system
  bash is 5.2.21).
- **`tests/scripts/declare_no_args_diff_check.sh`** (or a small new
  `declare_p_ansic_diff_check.sh`): add fragments defining vars/arrays with
  control-char values and assert byte-identical `declare -p` vs live bash.
- **herestr category PASS** — run
  `BASH_SOURCE_DIR=/tmp/bash-5.2.21 HUCK_BASH_TEST_HELPERS=/tmp/bash-test-helpers
  HUCK_BASH_TEST_CATEGORY=herestr bash tests/bash-test-suite/runner.sh`; expect 0
  diff. Headline success criterion.
- **Full suite** `cargo test --workspace` green (~3684); confirm no existing
  `declare -p` test that asserts a `"…"` form for a control-free value broke.

## Risks

- **Existing test churn:** any test asserting a `"…"` form for a value that
  actually contains a control char must be updated to `$'…'` (the old assertion
  encoded the bug). Control-free assertions are unaffected.
- **Trigger mismatch:** `is_control()` is the same predicate `declare_scalar_quote`
  already uses (and that path matches bash), so the `-p` and bare forms stay
  consistent. The C1 edge is the only known divergence (Out of Scope).
- herestr may reveal a second residual the measurement missed — if so, report it;
  the flip is the criterion, not a partial improvement.

## Divergence-doc bookkeeping

- On merge: DELETE the resolved `declare -p` ANSI-C value-quoting blocker from
  the herestr-scoped L-57 successor entry in `docs/bash-divergences.md`. If that
  was herestr's last listed blocker, the entry is removed (herestr resolved);
  add a low-severity `[deferred]` note for the C1-control octal edge in
  `ansi_c_quote` if worth tracking.
- Update `docs/bash-test-suite-baseline.md` (herestr → PASS; Summary counts) and
  the iteration memory.
