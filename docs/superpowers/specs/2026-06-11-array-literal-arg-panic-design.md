# huck v136 — fix the array-literal-as-argument panic (M-114) Design

**Status:** approved design, ready for implementation plan.
**Implements:** stop the panic when an array-literal / assignment-prefix word
reaches `expand()` as a command ARGUMENT (e.g. `eval x=(a b)`). Reconstruct it to
its `name…=…` source text so `eval` re-parses it — matching bash for all forms.
M-114 is huck's last open Tier-1 bug.
**Branch (impl):** `v136-array-literal-arg-panic`.

## Background — measured root cause

The lexer treats `name=(…)` (and `arr+=(…)`, `a[i]=v`, `a[i]+=z`) as an
assignment-shaped word with parser-internal `WordPart::AssignPrefix` / `ArrayLiteral`
parts. For a LEADING assignment or a declaration-builtin argument, `try_split_
assignment` consumes them. But as an argument to a NON-declaration command they
reach `expand()` / `expand_assignment()`, which `unreachable!`-panic:

```
$ huck -c 'eval x=(a b)'
thread 'main' panicked at src/expand.rs:984:17:
internal error: WordPart::ArrayLiteral is parser-internal and must not reach expand()…
```

Four panic sites: `AssignPrefix` + `ArrayLiteral` arms in BOTH `expand()`
(expand.rs:~983) and `expand_assignment()` (expand.rs:~1098). The panic class:

| form | word parts | bash via `eval` |
|---|---|---|
| `eval x=(a b)` | `[Literal "x=", ArrayLiteral([a,b])]` | `x=([0]=a [1]=b)` |
| `eval arr+=(x y)` | `[AssignPrefix{arr,+}, ArrayLiteral]` | `arr=([0]=x [1]=y)` |
| `eval a[1]=v` | `[AssignPrefix{a[1]}, Literal "v"]` | `a=([1]=v)` |
| `eval a[2]+=z` | `[AssignPrefix{a[2],+}, Literal "z"]` | `a[2]` += z |

**bash's actual rule:** `name=(…)` as an argument is accepted ONLY for declaration
builtins (`declare`/`local`/`typeset`/`readonly`/`export`) and `eval`; every other
command (`echo`/`true`/`:`/`command`/user functions) is a PARSE-TIME syntax error.
The escaped form `eval $2=\(…\)` (real `_upvars`) and the quoted `eval "x=(a b)"`
already work (no array-literal lexing) — only the UNESCAPED literal panics.

## Architecture — reconstruct the word to text instead of panicking

The fix matches bash for the eval/declaration cases (eval re-parses the
reconstructed text) and removes the crash everywhere. In both `expand()` and
`expand_assignment()`, the per-part loop's `AssignPrefix` and `ArrayLiteral` arms
become reconstruction (not `unreachable!`):

### `AssignPrefix { target, append }` → push the LHS text
```rust
let lhs = render_assign_target(target, shell);          // "name" or "name[<sub>]"
out.push_str(&lhs);
out.push_str(if append { "+=" } else { "=" });
```
where `render_assign_target`:
- `AssignTarget::Bare(name)` → `name`
- `AssignTarget::Indexed { name, subscript }` → `name` + `[` + `expand_assignment(subscript, shell)` + `]`

### `ArrayLiteral(elems)` → push `(` + rendered elements + `)`
Extract a shared helper (also used by v130's xtrace render, de-duping):
```rust
/// Reconstruct an array literal to re-parseable `(e1 e2 [k]=v …)` text.
fn reconstruct_array_literal(elems: &[ArrayLiteralElement], shell: &mut Shell) -> String
```
Per element (`ArrayLiteralElement { subscript: Option<Word>, value: Word }`):
- positional (`subscript == None`) → `render_elem_value(value, shell)`
- subscripted → `[` + `expand_assignment(subscript, shell)` + `]=` + `render_elem_value(value, shell)`

`render_elem_value(v, shell)`:
- if `word_literal_text(v)` is `Some(t)` (the canonical `a`/`b` literal case) → `t`
  verbatim (no quoting — exact for the common form)
- else → `xtrace_quote(expand_assignment(v, shell))` — the re-parse-safe quoter
  (quote-when-meta), so a spaced value (`"a b"`) survives as ONE element

The whole word then expands to `name…=(…)` text; for `eval`/`declare`/`local` that
text re-parses into the right array/element assignment, matching bash.

### v130 de-dup
`src/executor.rs` already has an inline array-literal render in the xtrace block
(`array_literal_elements` + a per-element loop, ~executor.rs:2920+). Route it
through the new shared `reconstruct_array_literal` so there is ONE renderer.
(If the xtrace render lives in executor and the new helper in expand.rs, put the
shared helper where both can call it — `expand.rs` `pub(crate)` is fine; the
xtrace code already calls into `expand`/`param_expansion`.)

## Scope & must-not-regress
- **Declaration commands unchanged.** `declare a=(x y)` / `local`/`export`/`readonly`
  still route through `try_split_assignment` → `decl_args` (M-117 path); they never
  reach the reconstruction arms. Verified by their existing tests staying green.
- **Leading assignments unchanged.** `arr=(a b)` as a leading command (or inline
  prefix) is consumed by `try_split_assignment`; not affected.
- **`expand_assignment` reconstruction** uses the SAME helpers; an array literal in
  an `expand_assignment` context (rare) reconstructs identically.
- **No recursion hazard:** element VALUES are ordinary Words (no nested
  `ArrayLiteral`), so `expand_assignment(value)` can't re-enter the ArrayLiteral arm.

## Documented divergences (NEW low-impact entries — replace the crash)
- **Non-eval/non-declaration command with an array-literal arg** (`echo x=(a b)`):
  bash is a PARSE-TIME syntax error; huck produces the reconstructed string argument
  (`echo` prints `x=(a b)`). Matching bash's parse error needs command-context-aware
  lexing (out of scope). → new `[intentional]`/`[low]` entry.
- **Element word-splitting of `$`-expansions** inside a reconstructed array-literal
  arg (`eval x=($v)`): best-effort; overlaps the already-deferred **M-102**
  (array-literal element word-splitting). The canonical literal case is exact. →
  note on the M-102 entry (no new entry needed) or a brief `[low]` note.

## Files & responsibilities

| File | Change |
|------|--------|
| `src/expand.rs` | Replace the 4 `unreachable!` arms (`AssignPrefix`+`ArrayLiteral` in `expand` and `expand_assignment`) with reconstruction; add `render_assign_target` + `reconstruct_array_literal` + `render_elem_value` (or place the array helper where the xtrace caller can reuse it). |
| `src/executor.rs` | Route the v130 xtrace array-literal render through the shared `reconstruct_array_literal` (de-dup). |
| `tests/array_literal_arg_integration.rs` (NEW) | The 4 eval forms + escaped/quoted forms + non-panic for `echo` + declaration-array regression. |
| `tests/scripts/array_literal_arg_diff_check.sh` (NEW) | Bash-diff over the eval-form cases. |
| `docs/bash-divergences.md` | DELETE M-114 (Tier-1 1→0); add the non-eval-syntax-error `[intentional]`/`[low]` divergence; note the element-splitting overlap with M-102. |

## Testing

1. **Integration `#[test]`s** (`tests/array_literal_arg_integration.rs`) — run via
   huck, assert exact output (compare each to bash):
   - `eval x=(a b); declare -p x` → `declare -a x=([0]="a" [1]="b")`
   - `eval arr+=(x y); declare -p arr` → matches bash
   - `eval a[1]=v; declare -p a` → `declare -a a=([1]="v")`
   - `eval a[2]+=z; a[2]=Q; a[2]+=z; echo ${a[2]}` → matches bash (append)
   - subscript/quoted-value: `eval 'x=([3]="a b" c)'; declare -p x` AND the unescaped
     `eval x=([3]="a b" c)` → matches bash (the quoted element stays one element)
   - escaped still works: `f(){ eval $1=\(p q\); }; f arr; declare -p arr` → matches
   - quoted still works: `eval "x=(a b)"; declare -p x` → matches
   - declaration regression: `declare d=(a b); declare -p d`; `local` in a function
   - **no panic**: `echo x=(a b)` exits without a panic (assert rc != 101 / no
     "panicked" on stderr); its stdout is the reconstructed `x=(a b)` (documented
     divergence vs bash's syntax error — assert huck's behavior, note bash differs)
2. **Bash-diff harness** `tests/scripts/array_literal_arg_diff_check.sh` — the
   eval-form + declaration cases, byte-identical bash↔huck. (Do NOT include the
   `echo x=(a b)` non-eval case — that's the documented syntax-error divergence;
   note it in a comment.)
3. **Full regression:** entire suite + ALL harnesses green — ESPECIALLY array,
   `declare`/`local`, inline-assignment, and the v130 xtrace tests (the xtrace
   render now routes through the shared helper — its output must be unchanged);
   clippy clean.

## Edge cases & notes
- **`a[i]=v` where the subscript is arithmetic** (`eval a[1+1]=v`): the `[sub]`
  reconstruction expands the subscript via `expand_assignment` (so `$`-vars resolve),
  and eval re-parses `a[2]=v` arith on its side — matches bash for the literal/`$var`
  case; a command-sub subscript double-evaluates (rare, note it).
- **Empty array literal** `eval x=()` → reconstruct `()` → `x=()` → empty array.
- **`xtrace_quote` on an element** keeps re-parse safety: `"a b"` → `'a b'`, `""` →
  `''`. The literal fast-path (`word_literal_text`) avoids quoting the common
  `a`/`b`/`c` case so the output stays bash-clean.
- The reconstruction is for the EXPANSION layer; it does not (and need not) replicate
  bash's parse-time gating — eval/declare consume the text correctly; the rare
  non-eval case is the documented divergence.
