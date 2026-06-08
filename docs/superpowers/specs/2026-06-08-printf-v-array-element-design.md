# huck v113 — `printf -v` array-element target (M-109) Design

**Status:** approved design, ready for implementation plan.
**Implements:** `printf -v NAME[SUBSCRIPT] …` — writing `printf`'s formatted
output into an array element (new **M-109**, Tier-2). The next gap surfaced by
`mise ` + `<TAB>` after v112 gave huck the arithmetic comma operator.
**Why now:** bash_completion's `__reassemble_comp_words_by_ref` builds its
`words` array with `printf -v "$2[i]" %s "${COMP_WORDS[i]}"` (line 291). huck's
`printf -v` validates its target with `is_valid_name`, which rejects
`words[0]` → `huck: printf: `words[0]': not a valid identifier`. The `words`
array never gets built, so `words`/`cword` stay unpopulated and the downstream
`_upvars -v … "$cword" …` (line 339) sees mangled args → `bash_completion: :
cword: invalid option` / `: : invalid option`. **All three reported errors are
one root cause** — supporting the array-element target clears them.
**Branch (impl):** `v113-printf-v-array-element`.

## Background — the contract (verified against bash)

| `printf -v` target | bash result |
|---|---|
| `printf -v "x[2]" %s hi` (x unset) | creates indexed array → `declare -a x=([2]="hi")` |
| `words=(); printf -v "words[0]" %s a; printf -v "words[1]" %s b` | `words=(a b)` |
| `printf -v "a[j+1]" %s X` (arith subscript) | element `j+1` set |
| `declare -A m; printf -v "m[key]" %s V` | `m[key]=V` (string key) |
| `printf -v plain %s hello` | `plain=hello` (unchanged — plain name) |

So a `NAME[SUBSCRIPT]` target writes the formatted result into that element with
**exactly the same subscript semantics as a normal `NAME[SUBSCRIPT]=value`
assignment**: arith-evaluated subscript for indexed/unset arrays, string key for
associatives, scalar→array promotion, readonly enforcement.

## Architecture — reuse `apply_one_assignment` (NOT a parallel write path)

huck already has the entire element-assignment machinery in
`apply_one_assignment` (`src/executor.rs`), which takes an
`Assignment { target: AssignTarget, value: Word, append: bool }` and, for an
`AssignTarget::Indexed { name, subscript: Word }` with a scalar value, evaluates
the subscript (arith for indexed, via the associative-dispatch prelude for
assoc), promotes/creates the array, and enforces readonly. `printf -v` should
route an element target through this rather than re-implementing the dispatch.

### Component 1 — accept a `name[sub]` `-v` target (`src/builtins.rs`)
`builtin_printf`'s `-v` flag handling (`~:2664-2668`) currently does:
```rust
if !is_valid_name(&args[i]) {
    eprintln!("huck: printf: `{}': not a valid identifier", args[i]);
    return ExecOutcome::Continue(1);
}
v_var = Some(args[i].clone());
```
Change the validation so the target is accepted when it is **either** a plain
valid identifier **or** a `name[subscript]` form whose name part is a valid
identifier:
```rust
let target = &args[i];
let valid = is_valid_name(target)
    || crate::expand::split_name_subscript(target)
        .map(|(name, sub)| is_valid_name(&name) && !sub.is_empty())
        .unwrap_or(false);
if !valid {
    eprintln!("huck: printf: `{target}': not a valid identifier");
    return ExecOutcome::Continue(1);
}
v_var = Some(target.clone());
```
- Promote `split_name_subscript` (`src/expand.rs:519`) from private `fn` to
  `pub(crate) fn` so `builtins.rs` can call it. It already returns
  `Some((name, sub))` only when the string ends with `]` and contains `[` with a
  non-empty name; we additionally require a **non-empty** `sub` (reject `x[]`,
  which bash also rejects) and a valid name part.

### Component 2 — route the element target through `apply_one_assignment` (`src/builtins.rs`)
The write site (`~:2755`) is currently:
```rust
if let Some(var) = v_var {
    let s = String::from_utf8_lossy(&buf).into_owned();
    if shell.try_set(&var, s).is_err() {
        eprintln!("huck: printf: {var}: readonly variable");
        return ExecOutcome::Continue(1);
    }
}
```
Change it to branch on whether `var` is a subscripted target:
```rust
if let Some(var) = v_var {
    let s = String::from_utf8_lossy(&buf).into_owned();
    if let Some((name, sub)) = crate::expand::split_name_subscript(&var) {
        // Array-element target: write via the same path as `name[sub]=value`,
        // so the subscript is arith-evaluated (indexed) / string-keyed (assoc),
        // the array is created/promoted, and readonly is enforced.
        use crate::command::{Assignment, AssignTarget};
        use crate::lexer::{Word, WordPart};
        let assignment = Assignment {
            target: AssignTarget::Indexed {
                name,
                subscript: Word(vec![WordPart::Literal { text: sub, quoted: false }]),
            },
            value: Word(vec![WordPart::Literal { text: s, quoted: true }]),
            append: false,
        };
        if crate::executor::apply_one_assignment(&assignment, shell).is_err() {
            // apply_one_assignment already printed the specific diagnostic
            // (readonly / type mismatch / bad subscript).
            return ExecOutcome::Continue(1);
        }
    } else if shell.try_set(&var, s).is_err() {
        eprintln!("huck: printf: {var}: readonly variable");
        return ExecOutcome::Continue(1);
    }
}
```
- The subscript `Word` is the raw `sub` text as a single unquoted literal — the
  Indexed assignment path expands+arith-evaluates it (so `words[i]` /
  `words[j+1]` evaluate `i` / `j+1` exactly as `words[i]=v` would).
- The value `Word` is the formatted bytes as a single **quoted** literal so it is
  assigned verbatim (no re-expansion / word-splitting).
- Confirm the exact field names/shape of `Assignment`, `AssignTarget::Indexed`,
  `Word`, and `WordPart::Literal` against the source at implementation time
  (`grep` the definitions); the structure above matches `src/command.rs`’s
  `AssignTarget { Bare(String), Indexed { name: String, subscript: Word } }` and
  `WordPart::Literal { text, quoted }`.

## Scoped out / notes
- **Exotic targets** (`printf -v 'arr[@]'`, `printf -v 'arr[*]'`): bash errors;
  huck routes them through `apply_one_assignment`'s Indexed path, which
  arith-evaluates `@`/`*` and errors too — acceptable (both fail, message text
  may differ).
- **Readonly/type-mismatch message text** for the element case comes from
  `apply_one_assignment` (`huck: NAME: readonly variable` / type-mismatch),
  not the `printf:`-prefixed form — a trivial stderr divergence; log a low `L-`
  note if it differs from bash's `printf:`-prefixed message. The load-bearing
  behavior (rc ≠ 0, no write) matches.
- **Empty subscript** `printf -v 'x[]'`: rejected by the `!sub.is_empty()` guard
  → `not a valid identifier` (bash also errors).

## Must-not-regress
- Plain `printf -v NAME …` (scalar) — still `try_set`, including its
  `printf: NAME: readonly variable` message and integer-coerce via `try_set`.
- `printf` without `-v` (writes to stdout / the redirected sink).
- All existing `printf` format behavior (`%s`/`%d`/`%q`/`%b`/width/precision/
  recycling), `--`, unknown-flag rejection.
- Existing `arr[i]=v` / `declare -a` / associative-array behavior (untouched;
  only a new caller of `apply_one_assignment`).

## Files & responsibilities

| File | Change |
|------|--------|
| `src/expand.rs` | promote `split_name_subscript` to `pub(crate)` |
| `src/builtins.rs` | accept `name[sub]` `-v` target; route element writes through `apply_one_assignment` |
| `tests/printf_v_array_integration.rs` | NEW — element-target cases |
| `tests/scripts/printf_v_array_diff_check.sh` | NEW — 37th bash-diff harness |
| `docs/bash-divergences.md`, `README.md` | M-109 `[fixed v113]`; changelog; README row; Tier-2 count |

## Testing

1. **Unit** (`src/builtins.rs` or `src/expand.rs`): `split_name_subscript`
   already has coverage; add a printf-target validation case if a helper is
   introduced (the inline check above needs no new unit test beyond the
   integration cases).
2. **Integration** (`tests/printf_v_array_integration.rs`, binary-driven):
   - `words=(); printf -v "words[0]" %s a; printf -v "words[1]" %s b; echo "${words[0]}/${words[1]}"` → `a/b`.
   - arith subscript: `j=2; printf -v "x[j+1]" %s X; declare -p x` → element 3 = X.
   - unset-var promotion: `printf -v "y[2]" %s hi; declare -p y` → `declare -a y=([2]="hi")`.
   - associative: `declare -A m; printf -v "m[key]" %s V; echo "${m[key]}"` → `V`.
   - plain-name regression: `printf -v plain %s hello; echo "$plain"` → `hello`.
   - the `__reassemble`-style shape: build a `words` array element-by-element
     with `printf -v "words[$i]"` in a loop, assert it populates.
   Verify each against the system bash first.
3. **37th bash-diff harness** `tests/scripts/printf_v_array_diff_check.sh` —
   byte-identical fragments for the contract cases (using `declare -p` /
   `echo "${arr[k]}"` readouts so output is deterministic).
4. **Regression**: full suite (2789+), all 37 harnesses, clippy clean. Pay
   attention to the existing `printf` and array tests.
5. **Payoff**: `mise ` + `<TAB>` (or the loop shape
   `COMP_WORDS=(mise ""); words=(); for ((i=0;i<${#COMP_WORDS[@]};i++)); do printf -v "words[i]" %s "${COMP_WORDS[i]}"; done; declare -p words`)
   no longer prints `printf: 'words[0]': not a valid identifier`, and the
   downstream `_upvars` `invalid option` cascade is gone. Report before/after.

## Edge cases & notes
- The `subscript` literal is unquoted so the Indexed path arith-evaluates it;
  the value literal is quoted so it is stored verbatim (matches bash: the
  formatted string is assigned literally, not re-split).
- A subscript referencing an unset variable evaluates to 0 (huck arith
  convention), matching bash for `printf -v "x[unset]"` → `x[0]`.
- `apply_one_assignment` is already `pub(crate)`, callable from `builtins.rs`.
