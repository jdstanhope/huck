# v209: Per-element parameter-expansion modifiers on whole arrays — Design

**Status:** approved 2026-06-23
**Iteration:** v209
**Resolves:** M-127 (case modification on whole arrays). Broader sweep also
covers prefix/suffix/substitute/per-element-Transform modifiers that the same
v71 guard rejects.

## Goal

Apply per-element parameter-expansion modifiers across whole arrays in both
indexed and associative form — `${a[@]^^}`, `${a[@]#pat}`, `${a[@]/pat/repl}`,
`${a[@]@U}`, etc. — matching bash. Closes M-127 outright and shrinks the
v71/v72 "not supported on array" catchall to its remaining justified cases
(`@A`/`@K`/`@k`/`@a` ← M-93; `${a[@]:=word}` ← bash errors on this too).

## Decisions (from brainstorming)

1. **Broader sweep.** Cover Case, RemovePrefix, RemoveSuffix, Substitute, and
   per-element Transform ops (`@P`/`@Q`/`@U`/`@u`/`@L`/`@E`) — not just Case.
   `@A`/`@K`/`@k`/`@a` stay as M-93. `${a[@]:=word}` stays rejected (bash
   errors on it too). `${a[@]:?word}` stays rejected for v209 (rare; small
   follow-on if friction surfaces).
2. **Both indexed AND associative arrays.** Symmetric fix in
   `expand_array_param` and `expand_assoc_param`.
3. **Pattern args re-expanded per element.** Bash expands them once; we
   re-expand per element. Observably identical for idempotent patterns (the
   overwhelming common case). Documented; cheap follow-on if real friction.

## Observable behavior (post-v209)

These all work bash-compatibly:

```bash
# Case modification (M-127's literal scope)
a=(foo bar baz); echo "${a[@]^^}"          # → FOO BAR BAZ
a=(FOO BAR); echo "${a[@],,}"              # → foo bar
a=(hello world); echo "${a[@]^^[hl]}"      # → HeLLo worLd (pattern)
a=(foo bar); echo "${a[*]^^}"              # → FOO BAR (joined by IFS)
a=(); echo "[${a[@]^^}]"                   # → []  (empty)
declare -A m=([k]=foo [j]=bar); echo "${m[@]^^}"  # → FOO BAR (assoc)

# Prefix / suffix / substitute per element
a=(foo.txt bar.md baz.txt); echo "${a[@]%.*}"   # → foo bar baz
a=(foo.txt bar.md); echo "${a[@]##*.}"          # → txt md
a=(foo bar baz); echo "${a[@]/a/X}"             # → foo bXr bXz
a=(foo bar baz); echo "${a[@]//[ao]/X}"         # → fXX bXr bXz

# Per-element Transform (the @P/@Q/@U/@u/@L/@E subset)
a=(foo BAR baz); echo "${a[@]@U}"               # → FOO BAR BAZ
a=(foo BAR baz); echo "${a[@]@L}"               # → foo bar baz
a=(foo "bar baz"); printf "%s\n" "${a[@]@Q}"   # → 'foo' / 'bar baz'
```

These stay rejected — intentional v209 scope cuts:

```bash
${a[@]:=word}    # AssignDefault — bash errors too; we preserve our error
${a[@]:?word}    # ErrorIfUnset on whole array — rare; deferred
${a[@]@A}        # whole-array @A — M-93
${a[@]@K}        # assoc key/value @K — M-93
${a[@]@k}        # alt assoc key/value @k — M-93
${a[@]@a}        # attribute flags — M-93
```

## Quoting & joining (preserves existing `[@]`/`[*]` discipline)

| Form | Quoted (`"…"`) | Unquoted |
|---|---|---|
| `${a[@]^^}` (etc.) | `WordList` — one field per element | `Value` joined by first IFS char (consumer re-splits) |
| `${a[*]^^}` (etc.) | `Value` joined by first IFS char | Same `Value`-join |

## Internal architecture

### Files

- `crates/huck-engine/src/expand.rs` — add the per-element arm in both
  `expand_array_param` and `expand_assoc_param`. Add two predicates +
  one helper.
- `crates/huck-engine/src/param_expansion.rs` — no change. The existing
  `expand_modifier_with_value` is reused per-element.
- `tests/scripts/array_modifiers_diff_check.sh` — new bash-diff harness.
- `docs/bash-divergences.md` — DELETE the M-127 entry.

### New predicates + helper in `expand.rs`

```rust
/// Whether this modifier can sensibly apply per-element across a whole
/// array. The whole-array Transform ops (@A / @K / @k / @a) and
/// AssignDefault / ErrorIfUnset are excluded; the catchall continues to
/// reject them.
fn is_per_element_modifier(m: &ParamModifier) -> bool {
    use ParamModifier as PM;
    match m {
        PM::Case { .. }
        | PM::RemovePrefix { .. }
        | PM::RemoveSuffix { .. }
        | PM::Substitute { .. } => true,
        PM::Transform { op } => is_per_element_transform_op(*op),
        _ => false,
    }
}

/// @P / @Q / @U / @u / @L / @E are per-element. @A / @K / @k / @a are
/// whole-array (M-93 territory).
fn is_per_element_transform_op(op: TransformOp) -> bool {
    use TransformOp as TO;
    matches!(op, TO::Prompt | TO::Quote | TO::Upper | TO::TitleFirst | TO::Lower | TO::AnsiC)
}

/// Apply a scalar modifier to one element's value via the existing
/// expand_modifier_with_value scalar path. Wraps the element in
/// ParamLookup::Element so missing-element default semantics aren't
/// accidentally triggered (every element here has a concrete value).
fn scalar_apply_per_element(
    name: &str,
    modifier: &ParamModifier,
    element: &str,
    quoted: bool,
    shell: &mut Shell,
) -> String {
    use crate::param_expansion::{expand_modifier_with_value, ExpansionResult, ParamLookup};
    match expand_modifier_with_value(
        name,
        modifier,
        ParamLookup::Element(Some(element)),
        quoted,
        shell,
    ) {
        ExpansionResult::Value(s) => s,
        ExpansionResult::Empty => String::new(),
        _ => String::new(),
    }
}
```

(`TransformOp` variant names — `Prompt`, `Quote`, etc. — to be confirmed
against the actual `lexer::TransformOp` enum during implementation.)

### The new arm — symmetric for indexed and associative

Inserted in `expand_array_param`'s match BEFORE the catchall at line ~819:

```rust
// Per-element scalar modifier across a whole array. WordList for quoted
// [@]; joined Value for [*] or unquoted [@].
(modif, SK::All | SK::Star) if is_per_element_modifier(modif) => {
    let values = collect_values(shell);
    let transformed: Vec<String> = values
        .iter()
        .map(|v| scalar_apply_per_element(name, modif, v, quoted, shell))
        .collect();
    if matches!(subscript, SK::All) && quoted {
        ExpansionResult::WordList(transformed)
    } else {
        let ifs = shell.ifs();
        let sep = ifs_join_sep(&ifs);
        ExpansionResult::Value(transformed.join(&sep))
    }
}
```

Mirror this exactly in `expand_assoc_param` (line ~440), using the assoc
path's already-computed `values: Vec<String>` snapshot instead of
`collect_values`.

### Catchall — preserved with shrunken footprint

The existing `(other, SK::All | SK::Star) => with_err(... "not supported
on array in v71" ...)` arm stays. Post-v209 it only fires for:
- `AssignDefault` (matches bash's error behavior)
- `ErrorIfUnset` (deferred)
- `Transform { op: @A | @K | @k | @a }` (M-93)
- Any future modifier added without explicit per-element wiring

Wording: keep the existing message verbatim. (No revision to mention M-93
specifically — the message is a generic guard.)

### Pattern argument re-expansion

`expand_modifier_with_value` re-expands the pattern Word once per element.
For idempotent patterns (the common case) this is observably identical to
bash's once-per-expansion semantics. For side-effecting patterns (e.g.
`${a[@]/$(echo o)/X}`) we run N command substitutions where bash runs one.
Documented as a known low-impact divergence; logged for follow-on if real
friction surfaces.

## Semantics & edge cases

### Empty array
```bash
a=(); echo "[${a[@]^^}]"   # bash: []   huck: []
```
`collect_values` returns `Vec::new()`; the `.map` is a no-op; `WordList(vec![])`
produces zero fields.

### Single-element array
```bash
a=(foo); echo "${a[@]^^}"   # → FOO
```
Single value passes through; one-field WordList.

### Sparse indexed arrays
```bash
a=([0]=foo [5]=bar [10]=baz); echo "${a[@]^^}"   # → FOO BAR BAZ
```
`collect_values` uses BTreeMap's subscript-ascending order. Indices aren't
preserved in output (matches bash).

### Empty element
```bash
a=(foo "" bar); printf "[%s]\n" "${a[@]^^}"   # → [FOO] [] [BAR]
```
Empty string passes through `case_modify` etc. as empty; preserved as a field.

### Associative iteration order
Pre-existing L-44 intentional divergence — huck uses insertion order; bash
uses internal hash order. v209 doesn't change this.

### `set -u`
A whole-array expansion under `set -u` doesn't fault on unset elements
(arrays are "set" iff they have ≥1 element; element-level unset doesn't
matter for the array form). No change to existing `set -u` semantics.

### Pattern with command substitution
`${a[@]/$(slow_cmd)/X}` runs `slow_cmd` N times instead of once. Pure
side-effect divergence; identical output. Logged.

### Nameref to array
`declare -n r=a; echo "${r[@]^^}"` — the existing nameref resolution at
the top of `expand_array_param` (lines 620-632) runs before our new arm.
The resolved name flows into `collect_values` → into the per-element arm.
Bash-correct.

### Pre-existing `pending_fatal_pe_error` guard
Sits before the new arm. A fatal PE mid-iteration short-circuits remaining
elements. Matches existing fatal-PE behavior across the expander.

## Testing & verification

### Bash-diff harness — `tests/scripts/array_modifiers_diff_check.sh`

~25 fragments through bash + huck with byte-identical assertion. Coverage:

**Case (8)** — M-127 literal:
- `a=(foo bar baz); echo "${a[@]^^}"`
- `a=(foo bar baz); echo "${a[@]^}"`
- `a=(FOO BAR); echo "${a[@],,}"`
- `a=(FOO BAR); echo "${a[@],}"`
- `a=(hello world); echo "${a[@]^^[hl]}"` — pattern arg
- `a=(foo bar); echo "${a[*]^^}"` — star
- `a=(); echo "[${a[@]^^}]"` — empty
- `declare -A m=([k]=foo [j]=bar); echo "${m[@]^^}"` — assoc

**Prefix / suffix / substitute (6)**:
- `a=(foo.txt bar.md baz.txt); echo "${a[@]%.*}"`
- `a=(foo.txt bar.md); echo "${a[@]##*.}"`
- `a=(foo bar baz); echo "${a[@]/a/X}"`
- `a=(foo bar baz); echo "${a[@]//[ao]/X}"`
- `a=(foo bar); echo "${a[*]/o/_}"`
- `declare -A m=([k]=foo.txt [j]=bar.md); echo "${m[@]%.*}"`

**Per-element Transform (5)**:
- `a=(foo BAR baz); echo "${a[@]@U}"`
- `a=(foo BAR baz); echo "${a[@]@L}"`
- `a=(foo BAR baz); echo "${a[@]@u}"`
- `a=(foo "bar baz"); printf "%s\n" "${a[@]@Q}"`
- `declare -A m=([k]=Foo [j]=Bar); echo "${m[@]@L}"`

**Edge cases (4)**:
- `a=([0]=foo [5]=bar [10]=baz); echo "${a[@]^^}"` — sparse
- `a=(foo "" bar); printf "[%s]\n" "${a[@]^^}"` — empty element
- `a=(foo); echo "${a[@]^^}"` — single
- `a=(foo bar); for x in "${a[@]^^}"; do echo "<$x>"; done` — field discipline

`@P` excluded from the byte-diff (machine-dependent prompt expansion).
ANSI-C `$'\xff'` escape excluded (L-11 byte-vs-char divergence).

### Unit tests in `expand.rs::mod tests`

Six focused tests covering wiring rather than bash parity:
- `case_modifier_on_indexed_array_at` — `${a[@]^^}` returns WordList with N elements.
- `case_modifier_on_indexed_array_star` — `${a[*]^^}` returns joined Value.
- `case_modifier_on_associative_array` — `${m[@]^^}` works on assoc.
- `remove_prefix_per_element_indexed` — `${a[@]#fo}` strips per element.
- `substitute_per_element_assoc` — `${m[@]/o/X}` substitutes per assoc value.
- `transform_at_p_on_array_does_not_error` — `${a[@]@P}` returns N fields with no error (no byte assertion — `@P` is machine-dependent).

### Explicit rejection regression tests

To pin the scope cuts and catch a regression if the catchall is
over-generalized later:
- `${a[@]:=word}` → still errors.
- `${a[@]:?word}` → still errors (current behavior preserved).
- `${a[@]@A}` → still errors (M-93).

In `engine.rs`-style unit tests OR in the bash-diff harness with `_exit_only`
comparison (we only check exit code, not stderr wording).

### CLI byte-identical gate

`Engine::run` / `Engine::capture` / etc. paths unchanged. All 128 existing
harnesses + the new array_modifiers harness pass. Headless smoke identical.

### Workspace gates

- `cargo test --workspace --quiet` — green.
- `cargo test --workspace --doc --quiet` — green.
- `cargo clippy --workspace --all-targets -- -D warnings` — clean.
- `cargo build --release --workspace` — clean.

## Risks & mitigations

- **The `Transform { op }` match pattern with `if is_per_element_transform_op(*op)`
  may need a `TransformOp: Copy` derive.** Mitigate: confirm during
  implementation; if not Copy, dereference via match-guard differently. Trivial
  fix.
- **Pattern args re-expanded per element** — N command substitutions where
  bash runs one. Documented; cheap follow-on if real friction.
- **`@Q` quoting may differ in edge cases from bash** — pre-existing
  L-17(c) char-vs-byte divergence for non-ASCII bytes. v209 doesn't change
  it; the harness fragments use ASCII content.
- **Associative iteration order** — pre-existing L-44 intentional divergence.
  Assoc harness fragments use single-pair tests or assert order-agnostic to
  avoid coupling tests to bash's hash order.

## Out of scope (deferred)

- `${a[@]@A}` / `${a[@]@K}` / `${a[@]@k}` / `${a[@]@a}` — M-93.
- `${a[@]:?word}` — rare; small follow-on.
- `${a[@]:=word}` — bash errors; we preserve the error.
- Pattern args single-evaluation across element iterations — low-impact.
- Associative iteration order match with bash — L-44 intentional.
- Public API surface changes — none in v209.

## Task decomposition (for the plan)

1. **Add `is_per_element_modifier` + `is_per_element_transform_op` predicates
   + `scalar_apply_per_element` helper** in `expand.rs`. Pure additions; no
   call sites yet. Suite green.
2. **Wire the per-element arm into `expand_array_param`** before the existing
   catchall. Targeted unit tests (Case + one prefix/suffix on indexed).
3. **Wire the symmetric arm into `expand_assoc_param`.** Targeted unit tests
   (Case + Substitute on assoc).
4. **Add the bash-diff harness** `tests/scripts/array_modifiers_diff_check.sh`
   with ~24 fragments. All PASS.
5. **Add explicit rejection regression tests** for `${a[@]:=word}`,
   `${a[@]:?word}`, `${a[@]@A}` so the catchall's footprint is documented.
6. **Update `docs/bash-divergences.md`** — DELETE the M-127 entry (per the
   current-divergences-only policy). Add brief `[deferred] low` entry for
   `${a[@]:?word}` if we want to track it; otherwise leave it implicit.

6 tasks. Comparable to v208 in scope — concentrated in `expand.rs`, no new
modules, no platform code, no public API change.
