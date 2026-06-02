# huck v74 — Configurable IFS (M-05)

**Status:** design complete; ready for plan.
**Date:** 2026-06-02.
**Closes:** M-05 (`IFS not configurable`), high-priority Tier-2 entry deferred since the original 2026-05-23 audit.

## Goal

Replace huck's hardcoded ASCII-whitespace word-splitting (`split_ascii_whitespace`) with bash-compatible IFS-driven field splitting. After v74, setting `IFS` to any value reshapes how unquoted expansions split into argv fields, matching POSIX § 2.6.5 and bash 5.x behavior.

## Out of scope

- `IFS` propagation to subshells beyond the existing v23 inline-prefix mechanism (no new wiring; the existing snapshot/restore cycle already handles `IFS=: cmd`).
- IFS-aware quoting for `declare -p` / `set` output (the existing scalar-escape covers IFS values).
- Locale-aware splitting (POSIX only specifies byte-level; we match).

## Architecture

### `Shell::ifs()` helper (new)

A single accessor centralizes IFS lookup:

```rust
impl Shell {
    /// Returns the current value of `$IFS`. Unset → POSIX default `" \t\n"`.
    /// Empty → no field-splitting (caller short-circuits).
    pub fn ifs(&self) -> String {
        self.lookup_var("IFS").unwrap_or_else(|| " \t\n".to_string())
    }
}
```

This replaces the ~10 inline copies of `shell.lookup_var("IFS").unwrap_or_else(|| " \t\n".to_string())` already scattered across `src/expand.rs` and `src/param_expansion.rs`. The centralization is a small DRY win and makes the "unset vs empty" boundary explicit.

### `emit_split_fields` (rewritten)

Current implementation (`src/expand.rs:900`) uses `value.split_ascii_whitespace()` — fixed `[' ', '\t', '\n']`. The new version takes an `ifs: &str` parameter and implements POSIX § 2.6.5:

```rust
fn emit_split_fields(
    value: &str,
    ifs: &str,
    current: &mut Field,
    result: &mut Vec<Field>,
    has_emitted: &mut bool,
);
```

Semantics:

1. **IFS empty** → no split. The entire `value` is appended to `current`; no field boundaries.
2. **IFS non-empty** → partition the IFS bytes into two classes:
   - **Whitespace IFS** = subset of IFS bytes that are `b' '` / `b'\t'` / `b'\n'`.
   - **Non-whitespace IFS** = every other IFS byte.
3. Walk `value` byte by byte:
   - **Leading IFS-whitespace** is consumed and discarded (no leading empty field).
   - A run of consecutive **IFS-whitespace** bytes is a single separator.
   - A **non-whitespace IFS** byte is a separator. Adjacent IFS-whitespace bytes (before or after) are consumed with it as part of the same separator.
   - Two adjacent non-whitespace IFS bytes produce an empty field between them (`a::b` with `IFS=:` → `a`, ``, `b`).
   - **Trailing IFS-whitespace** is consumed and discarded.
   - **Trailing non-whitespace IFS** does NOT produce a trailing empty field (matches bash: `v="a:"; IFS=:; echo $v` → `a`, one field).

The classifier and walk are byte-oriented (matches bash; POSIX explicitly allows byte-level for IFS).

The implementation can crib from `src/builtins.rs::split_into_names` (the `read` builtin's IFS splitter from v55), which already implements this correctly. The two functions serve different output shapes (`emit_split_fields` produces a `Vec<Field>` for argv assembly; `split_into_names` assigns into named variables with last-name-merge), so they remain separate, but the classification logic is shared in spirit.

### `${*}` / `${a[*]}` / `${m[*]}` separator

The quoted-context join uses the first character of IFS:

- **IFS unset** → join with `" "` (single space).
- **IFS empty** → join with `""` (no separator; concatenate).
- **IFS non-empty** → join with the first byte (technically: first char) of IFS.

Current huck code does `ifs.chars().next().unwrap_or(' ').to_string()` at ~6 sites. This is wrong for the empty-IFS case (returns `' '` instead of `""`). Fix via a helper:

```rust
/// Returns the IFS join separator for `"$*"` / `"${a[*]}"` contexts.
/// Empty IFS → empty separator (concatenate). Otherwise → first char of IFS.
/// Caller already knows IFS came from `Shell::ifs()` so the unset case is
/// already substituted with the default `" \t\n"` (first char `' '`).
fn ifs_join_sep(ifs: &str) -> String {
    ifs.chars().next().map(|c| c.to_string()).unwrap_or_default()
}
```

Replace each call site's `ifs.chars().next().unwrap_or(' ').to_string()` pattern with `ifs_join_sep(&ifs)`.

### Call-site updates

Every site that currently looks up IFS or calls `emit_split_fields` is updated. Audit (from existing greps):

- `src/expand.rs` — the main expansion match (the big switch over `WordPart`):
  - **Unquoted Var** path: `emit_split_fields(value, &shell.ifs(), …)` (was: `value.split_ascii_whitespace()` directly via the existing helper).
  - **Unquoted CommandSub** path: same.
  - **Unquoted `$@`/`$*`** paths: same after the join.
  - **`${a[@]}` unquoted** and **`${a[*]}` unquoted**: same.
- `src/param_expansion.rs` — `${*}` / `${@}` / `${a[*]}` quoted joins. ~6 sites with the `chars().next().unwrap_or(' ')` pattern → `ifs_join_sep(&shell.ifs())`.
- `src/expand.rs::expand_array_param` / `expand_assoc_param` — the `(PM::None, SK::Star)`, `(PM::IndirectKeys, SK::Star)`, and slicing-unquoted-star arms. Same pattern.

### `builtin_read` (no change)

`split_into_names` in `src/builtins.rs:1700ish` already implements full POSIX IFS semantics correctly (added in v55). It reads `shell.lookup_var("IFS")` itself, so swapping to `shell.ifs()` is a one-line consistency improvement but not strictly required. Make the swap for DRY.

### `inline_assignments` interaction

`IFS=: cmd args` should split `args` with `IFS=:`. The existing v23 mechanism applies inline assignments BEFORE the command's args are expanded (via `apply_inline_assignments` in `src/executor.rs`), so word-splitting during expansion sees the temporary IFS value. No new wiring needed; verify with an integration test.

## Error handling

No new error paths. IFS values are byte-strings; any byte combination is valid. Setting IFS to something exotic (e.g., NUL byte) is allowed but practically useless — bash accepts it too.

## Testing

### Unit tests (in-source)

New `mod ifs_splitter_tests` at the bottom of `src/expand.rs` (~12 tests against `emit_split_fields` directly):

1. `default_ifs_collapses_whitespace_runs` — `IFS=" \t\n"`, value `"a  b\tc"` → 3 fields.
2. `colon_ifs_preserves_empty_between` — `IFS=":"`, `"a::b"` → `a`/``/`b`.
3. `colon_ifs_leading_produces_empty` — `IFS=":"`, `":a"` → ``/`a` (2 fields).
4. `colon_ifs_trailing_no_empty` — `IFS=":"`, `"a:"` → `a` (1 field).
5. `mixed_ifs_ws_collapses_around_nonws` — `IFS=" :"`, `"a : b"` → `a`/`b` (2 fields, colon + adjacent spaces are one separator).
6. `empty_ifs_no_split` — `IFS=""`, `"a b c"` → 1 field `a b c`.
7. `whitespace_only_value_yields_no_fields` — `IFS=" "`, `"   "` → 0 fields.
8. `mixed_consecutive_nonws_empty_field` — `IFS=":,"`, `"a:,b"` → `a`/``/`b` (3 fields).
9. `single_nonws_only_yields_empty_field` — `IFS=":"`, `":"` → 1 field (empty).
10. `non_ascii_ifs_byte` — `IFS="α"` (multibyte), value uses that byte → splits at byte boundaries; documented as byte-level behavior (matches bash).
11. `current_field_continuation` — `current` field already contains text; first IFS-split fragment continues it.
12. `no_emission_when_value_is_only_leading_ws` — `IFS=" "`, `"  "`, current empty → 0 fields emitted.

Plus extensions to existing modules:
- `expand_unquoted_*` tests get `with_colon_ifs` and `with_empty_ifs` variants (~4 tests).
- `array_expansion_tests`: `star_join_uses_first_ifs_char` (replace existing if any) + `star_join_empty_ifs_concatenates` (~2 new tests).
- `assoc_expansion_tests`: parallel pair for associative `${m[*]}` (~2 new tests).

### Integration tests (`tests/ifs_integration.rs`, new)

~8 binary-driven scripts mirroring the bash-diff harness fragments:

1. `default_ifs_for_loop_splits_on_whitespace`
2. `colon_ifs_for_loop_splits_on_colons`
3. `colon_ifs_preserves_empty_middle_field`
4. `colon_ifs_trailing_no_empty_field`
5. `empty_ifs_no_splitting`
6. `local_ifs_reverts_on_function_return`
7. `command_sub_splits_with_current_ifs`
8. `star_join_uses_first_ifs_char`

### Bash-diff harness

New file `tests/scripts/ifs_diff_check.sh` (parallel to `arrays_diff_check.sh`) with ~10 fragments:

```bash
fragments=(
    'v="a b c"; for x in $v; do echo $x; done'
    'IFS=:; v="a:b:c"; for x in $v; do echo $x; done'
    'IFS=:; v="a::b"; for x in $v; do echo "[$x]"; done'
    'IFS=:; v=":a"; for x in $v; do echo "[$x]"; done'
    'IFS=:; v="a:"; for x in $v; do echo "[$x]"; done'
    'IFS=" :"; v="a : b"; for x in $v; do echo $x; done'
    'IFS=; v="a b c"; for x in $v; do echo "[$x]"; done'
    'set -- a b c; IFS=,; echo "$*"'
    'set -- a b c; IFS=; echo "$*"'
    'IFS=:; for x in $(echo "a:b:c"); do echo $x; done'
)
```

All should produce byte-identical output to bash 5.2.21.

## Documentation

`docs/bash-divergences.md`:
- Update **M-05** entry: `[deferred] high` → `[fixed v74]`. Body describes the splitter rewrite, the `Shell::ifs()` helper, the empty-IFS-no-split rule, and the trailing-non-ws-IFS quirk.
- Add change-log entry dated 2026-06-02.

`README.md`: add row `| v74 | configurable IFS (M-05) |`.

## Risks

- **Blast radius**: every unquoted expansion site potentially changes behavior. **Mitigation**: the existing ~2090 tests should all still pass under default IFS (`" \t\n"`) — if they do, default-IFS behavior is unchanged. Any regression there means the new splitter has a bug in the default case.
- **POSIX edge cases**: the trailing-non-ws-IFS rule (`v="a:"` → 1 field) is non-obvious. **Mitigation**: pinned by both a unit test and a bash-diff fragment.
- **Per-stage IFS via inline prefix**: `IFS=: cmd $args` should use `IFS=:` when splitting `$args`. **Mitigation**: integration test for this case. The existing v23 snapshot/restore mechanism already orders things correctly.
- **Non-ASCII IFS bytes**: huck operates on `&str` (UTF-8) but IFS classification is byte-oriented. Multibyte IFS chars (e.g., `IFS=α`) split on the constituent bytes. This matches bash. **Mitigation**: unit test documents the byte-level behavior.
- **`split_into_names` (read) divergence**: it has its own IFS classifier. If we don't share, the two implementations could drift. **Mitigation**: comment in each pointing at the other; verify they agree on the trailing-non-ws-IFS rule.
