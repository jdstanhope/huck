# v210: whole-array `${var@OP}` attribute transforms (M-93)

## Goal

Implement the four whole-array `${var@OP}` operators currently rejected by
the lexer:

- `@A` — `declare`-style assignment string (round-trips through `eval` /
  `source`).
- `@K` — key/value pairs as a single quoted-internally string.
- `@k` — key/value pairs as a word list (each key and value a separate
  field on `[@]`).
- `@a` — attribute flag letters (`a` indexed, `A` assoc, `i` integer,
  `r` readonly, `x` exported, `l`/`u` case-folded, `n` nameref, `t`
  traced).

Resolves **M-93** (Tier 2) and shrinks **L-44** (Tier 4) to an
ordering-only residual.

## Background

v209 (commit `4b31f0d`) added the per-element modifier path in
`expand_array_param` / `expand_assoc_param` for the six already-supported
scalar `@OP` ops (`@P`/`@Q`/`@U`/`@L`/`@u`/`@E`) plus `Case`,
`RemovePrefix`, `RemoveSuffix`, `Substitute`. The predicate
`is_per_element_modifier` in `crates/huck-engine/src/expand.rs` matches
`Transform { .. }` blanket because today's `TransformOp` enum is
entirely per-element. The docstring on that predicate already calls out
that M-93's whole-array ops need a sub-discriminator when they land —
this iteration delivers it.

The lexer rejects `@A`/`@K`/`@k`/`@a` at
`crates/huck-syntax/src/lexer.rs:3470`, returning
`LexError::InvalidBraceModifier`. No expansion code runs for these
ops today.

## Scope

**In scope:**

- Four new `TransformOp` variants (`AssignDecl`, `KvString`, `KvWords`,
  `AttrFlags`) — lexer parses them, generator round-trips them.
- Scalar dispatch in `param_expansion.rs::expand_modifier_with_value`
  for `${scalar@A}`, `${scalar@K}`, `${scalar@k}`, `${scalar@a}`.
- Whole-array dispatch in `expand_array_param` and `expand_assoc_param`
  for `${arr[@]@OP}` / `${arr[*]@OP}`.
- Subscript-aware behavior matching bash:
  - `${arr@OP}` (no subscript) and `${arr[i]@OP}` (specific subscript)
    behave like a scalar of `arr[0]` or `arr[i]` but with a
    `declare -a`/`-A` prefix for the array-aware ops.
  - `${arr[@]@OP}` / `${arr[*]@OP}` produce the whole-array shape.
- L-44 (a) + (b) cleanup: bareword subscript keys when safe; trailing
  space before `)` on assoc body only (mirroring bash's inconsistency
  vs indexed body).
- New shared renderer `render_declare_body` consumed by `declare -p`,
  bare `declare`, and `${var@A}`.
- Bash-diff harness `tests/scripts/array_transforms_diff_check.sh`
  (~25 fragments) asserting byte-identical stdout vs `bash`.

**Out of scope:**

- L-44 (c) ordering — bash uses internal hash order, huck uses insertion
  order. Impractical to match; documented residual.
- Whole-array transforms outside the 4 new ops. The catchall after v210
  fires only for `AssignDefault` (`${a[@]:=word}`) and `ErrorIfUnset`
  (`${a[@]:?word}`).
- `${var@OP}` on namerefs through the nameref (the underlying target's
  type wins; the nameref attribute itself shows as `n` for `@a`).
- Locale-aware case in `@A` value escaping — uses Rust `char::is_control`
  same as v37 / v96 (matches bash in UTF-8 locales).

## Lexer additions

`crates/huck-syntax/src/lexer.rs:147` — extend `TransformOp`:

```rust
pub enum TransformOp {
    PromptExpand,    // @P
    Quote,           // @Q
    Upper,           // @U
    Lower,           // @L
    UpperFirst,      // @u
    EscapeExpand,    // @E
    // v210 additions:
    AssignDecl,      // @A — declare-style assignment string
    KvString,        // @K — k/v pairs as single quoted-internally string
    KvWords,         // @k — k/v pairs as word list
    AttrFlags,       // @a — attribute flag letters
}
```

Update the doc comment to drop "Array/attribute forms `@A`/`@K`/`@k`/`@a`
are deferred".

`crates/huck-syntax/src/lexer.rs:3461` — extend the parse match:

```rust
Some('A') => TransformOp::AssignDecl,
Some('K') => TransformOp::KvString,
Some('k') => TransformOp::KvWords,
Some('a') => TransformOp::AttrFlags,
```

`crates/huck-syntax/src/generate.rs:694` — extend the round-trip table
to emit `A`/`K`/`k`/`a` for the new variants.

Existing lexer unit tests (`lexer.rs:8059`) grow by 4 fixtures to assert
round-trip parse.

## Semantic dispatch table

Verified against bash 5.x. The output column is the exact bytes bash
produces; v210 must match.

| Op   | Subject                                  | Output                                                |
|------|------------------------------------------|-------------------------------------------------------|
| `@A` | unset variable                           | empty                                                 |
| `@A` | scalar no attrs `s=hello`                | `s='hello'`                                           |
| `@A` | attributed scalar `declare -x ev=42`     | `declare -x ev='42'`                                  |
| `@A` | scalar with control chars                | `s=$'…'` (ANSI-C `$'…'` form)                         |
| `@A` | scalar with `'` quote                    | `q='it'\''s'` (`'\''` rewrite)                        |
| `@A` | `${arr}` / `${arr[i]}` indexed           | `declare -a arr='value-of-[0]-or-[i]'`                |
| `@A` | `${arr[@]}` / `${arr[*]}` indexed        | `declare -a arr=([0]="x" [1]="y" [2]="z")` (no trailing space) |
| `@A` | `${m}` / `${m[k]}` assoc                 | `declare -A m` (no body — scalar lookup is empty)     |
| `@A` | `${m[@]}` / `${m[*]}` assoc              | `declare -A m=([k]="v1" [j]="v2" )` (trailing space!) |
| `@A` | empty indexed                            | `declare -a empty=()`                                 |
| `@A` | empty assoc                              | `declare -A empty=()`                                 |
| `@K` | `${arr[@]}` indexed                      | `0 "x" 1 "y" 2 "z"` (bareword keys, dquoted values)   |
| `@K` | `${m[@]}` assoc                          | `k "v1" j "v2" ` (trailing space)                     |
| `@K` | scalar / `${arr}` / `${arr[i]}`          | `'value'` (single-quoted; same shape as compact `@Q`) |
| `@K` | unset                                    | empty                                                 |
| `@k` | `${arr[@]}` indexed                      | word list `0 x 1 y 2 z` (6 fields)                    |
| `@k` | `${m[@]}` assoc                          | word list `k v1 j v2` (4 fields, insertion order)     |
| `@k` | scalar / `${arr}` / `${arr[i]}`          | `'value'` (same as `@K` scalar shape)                 |
| `@k` | unset                                    | empty                                                 |
| `@a` | scalar no attrs                          | empty                                                 |
| `@a` | indexed array                            | `a`                                                   |
| `@a` | assoc array                              | `A`                                                   |
| `@a` | `declare -i n=5`                         | `i`                                                   |
| `@a` | `declare -r r=42`                        | `r`                                                   |
| `@a` | `declare -irx mix=7`                     | `irx` (concatenated in canonical order)               |
| `@a` | unset                                    | empty                                                 |

**Canonical flag-letter order for `@a`**: `aAilnrtuxc` filtered to
present attributes — `a` (indexed), `A` (assoc), `i` (integer),
`l` (lower), `n` (nameref), `r` (readonly), `t` (trace), `u` (upper),
`x` (exported), `c` (capitalize, if huck supports it). Bash's order
under multi-attr verified: `declare -irx mix` → `irx` (i, r, x). The
order is alphabetical case-insensitive with capitals before lowercase
within ties; v210 mirrors bash's observed order via a fixed letter
table.

**Word-list vs Value discriminator**: `@k` on quoted `[@]` produces
`ExpansionResult::WordList`; everything else (`@A`, `@K`, `@a`, and
`@k` on `[*]` or unquoted) produces `ExpansionResult::Value`.

## Architecture

### New module: `crates/huck-engine/src/array_transforms.rs`

~150 LOC. Four entry points, each owning one op:

```rust
pub(crate) enum ScopeMode {
    /// `[@]` or `[*]` subscript — operate on the whole array's
    /// key/value pairs.
    Whole,
    /// Scalar variable, no subscript, or specific `[i]` — operate on
    /// a single value (the scalar, [0], or [i] respectively).
    /// Carries the resolved value to avoid re-lookup.
    ScalarOrElement(String),
}

pub(crate) fn assign_decl(name: &str, scope: ScopeMode, shell: &Shell) -> String;
pub(crate) fn kv_string(name: &str, scope: ScopeMode, shell: &Shell) -> String;
pub(crate) fn kv_words(name: &str, scope: ScopeMode, shell: &Shell) -> Vec<String>;
pub(crate) fn attr_flags(name: &str, shell: &Shell) -> String;
```

Each function reads `shell.var_kind(name)` (a new pub-crate helper on
`Shell` returning an enum `VarKind { Scalar, Indexed, Assoc, Unset }`,
plus attribute flags) to drive the dispatch table above.

`shell.var_kind` wraps the existing `Variable` lookup and returns
`(VarKind, AttrFlags)` where `AttrFlags` is a bitset of the per-var
flags (integer/readonly/exported/case/nameref/trace). The bitset
already exists on `Variable` in some form — v210 surfaces it via a
clean accessor.

### Predicate split in `crates/huck-engine/src/expand.rs`

Replace today's blanket `Transform { .. }` match:

```rust
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

fn is_per_element_transform_op(op: TransformOp) -> bool {
    use TransformOp::*;
    matches!(op, PromptExpand | Quote | Upper | Lower | UpperFirst | EscapeExpand)
}

fn is_whole_array_transform_op(op: TransformOp) -> bool {
    use TransformOp::*;
    matches!(op, AssignDecl | KvString | KvWords | AttrFlags)
}
```

Predicates `is_per_element_transform_op` and
`is_whole_array_transform_op` are exhaustive over today's
`TransformOp`. If a future iteration adds a new variant, both
predicates' `matches!` falls through to `false`, which means the
catchall fires — a safe degradation, not silent wrong behavior.

### Whole-array dispatch arm

Insert into `expand_array_param` AND `expand_assoc_param`, immediately
before the v71/v72 catchall (and after v209's per-element arm):

```rust
(PM::Transform { op }, sub) if is_whole_array_transform_op(*op) => {
    use TransformOp::*;
    let scope = if matches!(sub, SK::All | SK::Star) {
        ScopeMode::Whole
    } else {
        ScopeMode::ScalarOrElement(/* the resolved element value */)
    };
    match op {
        AssignDecl => ExpansionResult::Value(array_transforms::assign_decl(name, scope, shell)),
        KvString   => ExpansionResult::Value(array_transforms::kv_string(name, scope, shell)),
        KvWords    => {
            let words = array_transforms::kv_words(name, scope, shell);
            if matches!(sub, SK::All) && quoted {
                ExpansionResult::WordList(words)
            } else {
                let sep = ifs_join_sep(&shell.ifs());
                ExpansionResult::Value(words.join(&sep))
            }
        }
        AttrFlags  => ExpansionResult::Value(array_transforms::attr_flags(name, shell)),
        _ => unreachable!("guarded by is_whole_array_transform_op"),
    }
}
```

Mirrored exactly in `expand_assoc_param` — the only difference is the
underlying var-kind dispatch inside `array_transforms` (Indexed vs
Assoc), which the functions handle via `shell.var_kind`.

### Scalar dispatch in `param_expansion.rs`

Extend the `Transform { op }` arm at `param_expansion.rs:240` with 4
new sub-cases that ALSO call `array_transforms::*` with
`ScopeMode::ScalarOrElement(value)`. This is the path for `${s@A}`
where `s` is a plain scalar — the lookup yields the scalar value, the
new sub-case builds `s='hello'` (or `declare -X s='value'` if
attributed), and the result flows back as `ExpansionResult::Value`.

The scalar path also handles `${arr@A}` (no subscript) and
`${arr[0]@A}` (specific subscript) for indexed arrays — both produce
the `declare -a arr='value'` form, because `shell.var_kind(name)`
inside `assign_decl` sees Indexed and prepends `-a`.

### Catchall after v210

The v71/v72 catchall fires only for:

- `AssignDefault` (`${a[@]:=word}`) — bash also errors.
- `ErrorIfUnset` (`${a[@]:?word}`) — deferred follow-on tracked
  separately (not v210's scope).

`Transform` is now fully covered between v209's per-element arm and
v210's whole-array arm.

## L-44 shared renderer

### Today

`render_declare_value_part` and `render_assoc_value_part` (or similar —
the exact names live in `builtins.rs::declare_p_form` and friends)
render the body of `declare -p` and bare `declare`. They quote subscript
keys unconditionally (`["k"]`) and omit the trailing space before `)`
on assoc bodies — neither matches bash.

### v210

New shared module-private renderer in
`crates/huck-engine/src/declare_render.rs` (new file, ~80 LOC):

```rust
pub(crate) fn render_declare_body(
    name: &str,
    val: &VarValue,
    attrs: AttrFlags,
) -> String;

pub(crate) fn render_array_body(values_or_pairs: &ArrayBody) -> String;
```

Three sites consume the new renderer:

1. `declare -p` builtin (already existed) — switch to new renderer.
2. Bare `declare` (no args) listing (v190) — switch to new renderer.
3. `${var@A}` — call the new renderer with the appropriate prefix.

### Rules baked into the renderer

- **Subscript key quoting** (matches bash via probe):
  - Bareword if the key matches `^[A-Za-z0-9_-]+$` (letters, digits,
    underscore, dash — covers identifiers, integers including
    negative, and dashed identifiers like `key-1`).
  - Else **double-quoted** with `\$`/`\\`/`\"`/`` \` `` escapes
    (the same escape policy as values inside `(…)`). Bash uses
    double-quoting for special-char keys, not single-quoting.
- **Value quoting in array body**: always double-quoted (`"v1"`),
  bash's convention inside `(…)`.
- **Value quoting in scalar `name='value'` form**: always single-quoted
  with `'\''` rewrite.
- **Control chars**: route through `ansi_c_quote` (existing helper) for
  the `$'…'` form, regardless of context.
- **Trailing space before `)`**: ASSOC body has trailing space (`"v2" )`);
  INDEXED body does not (`"z")`). Mirrors bash's observed inconsistency.
- **Empty arrays**: `declare -a empty=()` / `declare -A empty=()` (no
  trailing space; no body).
- **Order**: insertion order (huck's invariant; bash hash order
  documented as remaining L-44 residual).

### L-44 entry shrink

The current L-44 entry in `docs/bash-divergences.md` covers three
facts: (a) subscript key quoting, (b) trailing space, (c) hash vs
insertion order. v210 resolves (a) and (b); (c) remains. Edit L-44 to
state the residual only. Tier 4 count unchanged (L-44 stays in the
list, just with less content).

## Edge cases + open behavioral questions

### Scalar `@K` and `@k` mismatch with @Q?

Bash output for `${s@K}` is `'hello'` (single-quoted). The
`shell_quote` helper used by `@Q` already produces this exact form for
plain ASCII strings. v210's scalar `@K`/`@k` cases will call into the
same `shell_quote` (or an equivalent — TBD), so the output coincides
with `@Q` for plain scalars. They diverge only when the underlying
value has control chars (`$'…'` form is identical) or when the subject
is an array under `[@]` (different shape entirely).

### Subscript on `@a`?

`${arr[@]@a}`, `${arr[i]@a}`, and `${arr@a}` all produce the same
output (the array-level attribute letters). The subscript is ignored
for `@a`. v210 honors this — `attr_flags` takes no `ScopeMode`
parameter.

### Namerefs?

If `name` is a nameref to `target`, then `${name@OP}` follows the
nameref (matches bash). The `attr_flags` for a nameref shows `n` only
when called on the nameref name directly via a non-following form,
which v210 does NOT distinguish in this iteration. This is consistent
with v160's nameref work — the nameref attribute is observed only
via `declare -p name` (which v210 renders), not via `${name@a}` (which
follows). Acceptable scope cut.

### `@A` on attributed empty assoc?

`declare -Ar m=()` → `${m[@]@A}` should produce
`declare -Ar m=()`. The `-A` prefix is the array kind; the `r` flag
appends. Order in the prefix letter run: `aAilnrtuxc` filter (so
`-Ar`, not `-rA`). v210 follows bash's exact letter run.

### `@A` value escaping consistency

Use the same `ansi_c_quote` machinery as `@Q` for control chars. v210
adds NO new escape logic; it composes the existing primitives.

## Testing strategy

### Unit tests (~12 new)

**Lexer** (`huck-syntax/src/lexer.rs::mod tests`):
- 4 fixtures asserting `${v@A}`/`${v@K}`/`${v@k}`/`${v@a}` parse to the
  correct `TransformOp` variant.
- 4 fixtures asserting `generate.rs` round-trips each new variant to
  the original source.

**Scalar dispatch** (`huck-engine/src/param_expansion.rs::mod tests`):
- `transform_assign_decl_on_scalar`: `${s@A}` for `s=hello` → `s='hello'`.
- `transform_assign_decl_on_attributed_scalar`: `${ev@A}` for
  `declare -x ev=42` → `declare -x ev='42'`.
- `transform_assign_decl_on_unset`: empty.
- `transform_attr_flags_on_attributed_scalar`: `${ev@a}` → `x`.

**Whole-array dispatch** (`huck-engine/src/expand.rs::mod tests`):
- `transform_assign_decl_on_indexed_at`: `${a[@]@A}` for `a=(x y z)` →
  `declare -a a=([0]="x" [1]="y" [2]="z")`.
- `transform_assign_decl_on_assoc_at`: `${m[@]@A}` for
  `declare -A m=([k]=v1 [j]=v2)` — order-agnostic comparison.
- `transform_kv_words_on_indexed_yields_wordlist`: `${a[@]@k}` →
  `WordList(["0", "x", "1", "y", "2", "z"])`.
- `transform_attr_flags_indexed_yields_a`: `${a@a}` → `Value("a")`.

### L-44 renderer unit tests (~3 new)

In the new `declare_render.rs::mod tests` (or wherever the renderer
lands):
- `bareword_assoc_key_when_identifier`: key `"foo"` → `[foo]`.
- `quoted_assoc_key_when_metachar`: key `"a b"` → `['a b']`.
- `trailing_space_on_assoc_body_not_indexed`: assoc has trailing
  space; indexed does not.

### Bash-diff harness `tests/scripts/array_transforms_diff_check.sh`

~25 fragments covering the dispatch table. Same shape as v209's
`array_modifiers_diff_check.sh` (66 LOC, byte-identical stdout
assertion).

**Coverage breakdown:**
- `@A` (9): scalar / attributed scalar / unset / indexed `[@]` / assoc
  `[@]` / empty indexed / empty assoc / control-char value (`$'…'`) /
  quote-in-value (`'\''`).
- `@K`/`@k` (6): indexed `[@]` / assoc `[@]` / scalar / for-loop
  word-list check on `@k`.
- `@a` (7): scalar-no-attrs / `-i` / `-r` / `-x` / `-irx` multi-attr /
  `-a` indexed / `-A` assoc.
- Subscript variants (3): `${a@A}` vs `${a[0]@A}` vs `${a[@]@A}`.

Assoc-order-sensitive fragments pipe through `sort` to dodge L-44(c).
Where bash would output a key-value pair sequence that depends on
internal hash order (e.g. `@K` on assoc), the fragment iterates with
a for-loop and pipes individual `<k>=<v>` lines through `sort`.

### Existing harness regression

Pre-existing harnesses that exercise `declare -p` or bare `declare`
output become byte-identical to bash after the L-44(a)+(b) fix. Where they previously captured huck's old quoted-key
form verbatim, update the expected output. Where the harness already
sorted output to dodge L-44(c), no change.

Run the FULL harness sweep at the end of the iteration to catch any
incidental regression.

## Documentation updates

`docs/bash-divergences.md`:
- DELETE the M-93 entry entirely (Tier 2 12 → 11).
- SHRINK the L-44 entry to state only the ordering residual.

`docs/architecture.md`:
- Tiny note pointing at `array_transforms.rs` from the
  "parameter expansion" section's where-to-add cheatsheet. ~1 sentence.

## Out-of-scope

- L-44 (c) ordering: bash internal hash order. Impractical to match.
- `ErrorIfUnset` and `AssignDefault` on `[@]`/`[*]` — already
  catchall-rejected; v210 pins this behavior with v209's existing
  regression tests.
- New `set -o` options.
- Performance optimization of `var_kind` lookup — the existing
  `Variable` lookup is already O(1) hash; v210 wraps it.

## Risks

1. **Bash quoting policy edge cases**: bash's exact bareword-vs-quoted
   subscript-key rule may have subtle edges not covered by the harness
   fragments. Mitigation: harness fragments cover identifier keys,
   integer keys, metachar keys, control-char keys. Add more if
   reviewer finds gaps.
2. **L-44(c) hash order surfacing in unexpected places**: a fragment
   that doesn't pipe through `sort` may flake if bash's hash order
   changes between versions. Mitigation: every assoc-order-sensitive
   fragment pipes through `sort`.
3. **`@A` empty-but-declared distinction**: bash distinguishes
   `declare -a a` (declared, no body) from `declare -a a=()` (declared,
   empty body). huck currently sets the var to empty `()` on bare
   `declare -a a` (L-46). v210 inherits L-46 — the output may show
   `declare -a a=()` where bash shows `declare -a a`. Acceptable;
   L-46 follow-on if it ever bites.
4. **Attribute letter order under future flags**: if huck ever adds an
   attribute flag bash doesn't have (or vice versa), the canonical
   letter table needs updating. Today's set is stable.

## Acceptance

- All harness fragments byte-identical to bash.
- Full suite green; clippy `-D warnings` clean; release builds.
- M-93 entry deleted from `docs/bash-divergences.md`.
- L-44 entry shrunk to ordering residual.
- No new public API on `Engine` / huck-engine crate root — this is a
  shell-semantics iteration, not embedding-arc.
