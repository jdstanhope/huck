# huck v33 — `${var:offset}` / `${var:offset:length}` Substring Expansion Design

**Goal:** Close M-16 by implementing bash's substring parameter expansion:
`${var:offset}` (from `offset` to end) and `${var:offset:length}` (`length`
chars starting at `offset`).

**Why:** Listed as **high impact** in `docs/bash-divergences.md` Tier 2. Real
bash scripts use substring expansion for path slicing, fixed-width parsing,
and string truncation. Currently huck fails with
`LexError::InvalidBraceModifier(":N")` at lex time, aborting the script.

## Forms

| Syntax | Meaning |
|---|---|
| `${var:offset}` | Chars from `offset` to end of `$var`. |
| `${var:offset:length}` | Up to `length` chars starting at `offset`. |
| `${1:off}` / `${1:off:len}` | Same on a positional parameter (`$1`, `$2`, …). |
| `${var: -3}` | Negative offset (space required to disambiguate from `:-`). |
| `${var:0:-1}` | Negative length — counts from end of string. |

`offset` and `length` are **arithmetic expressions** — evaluated via the
existing `arith::eval` from v22. They can reference variables (`${s:$n}`),
use operators (`${s:1:$((n*2))}`), include unary minus, and parenthesize.

## Semantics

Char-counting (Unicode codepoints) throughout — consistent with the existing
`${#var}` divergence documented as L-04.

Let `chars = $var` collected as a `Vec<char>` and `strlen = chars.len()`.

**Effective offset:**
- `offset >= 0` → `eff_off = min(offset, strlen)`
- `offset < 0` → `eff_off = max(strlen + offset, 0)`

**Effective length:**
- `length` absent → `eff_len = strlen - eff_off`
- `length = n, n >= 0` → `eff_len = min(n, strlen - eff_off)`
- `length = n, n < 0` → `eff_len = strlen + n - eff_off`. If the result is
  negative, raise `substring expression < 0` (matches bash error string).

**Slice:** `chars[eff_off .. eff_off + eff_len]`, re-collected into a `String`.

**Edge-case table** (let `s = "abc"`, `strlen = 3`):

| Expression | Result |
|---|---|
| `${s:0}` | `abc` |
| `${s:1}` | `bc` |
| `${s:3}` | empty (offset == strlen) |
| `${s:5}` | empty (offset > strlen, clamped) |
| `${s: -1}` | `c` |
| `${s: -3}` | `abc` |
| `${s: -5}` | `abc` (eff_off clamps to 0; eff_len = strlen - 0 = 3) |
| `${s:1:5}` | `bc` (length clamps to remaining 2) |
| `${s:1:-1}` | `b` (eff_len = 3 + (-1) - 1 = 1) |
| `${s:0:-3}` | empty (eff_len = 3 + (-3) - 0 = 0) |
| `${s:0:-4}` | error: `substring expression < 0` |
| `${s:nonsense}` | empty (arith error: print + `$?=1`) |

## Disambiguation rule

After `:`, the **literal next char** decides the dispatch:

| Next char | Modifier |
|---|---|
| `-` | UseDefault (`:-`) — unchanged |
| `=` | AssignDefault (`:=`) — unchanged |
| `?` | ErrorIfUnset (`:?`) — unchanged |
| `+` | UseAlternate (`:+`) — unchanged |
| anything else (digit, space, `(`, `$`, …) | **Substring** (new) |

So `${var:-3}` stays UseDefault with default `"3"`; `${var: -3}` (space) is
substring with offset = -3. This matches bash exactly. No lookahead past
whitespace.

## Scope (in)

- Both forms: `${var:off}` and `${var:off:len}`.
- Scalar named variables: `${name:off:len}`.
- Positional parameters: `${1:off:len}` — by extending the digit-only lexer
  branch to dispatch modifiers in the same way as the named-var branch.
- Negative offsets and lengths per the table.
- Full arithmetic in offset/length: variable refs, all operators v22's
  `arith::eval` supports, parentheses.
- Quoted contexts: `"${var:1:3}"` produces a single field, no word-splitting
  on the slice (mirrors v32 substitute and all other `${…}` modifiers).
- Pipeline stages, inside `${}` operand contexts, inside heredoc bodies — all
  routed through the same `ParamExpansion` codepath, so they fall out for
  free.

## Scope (out — explicitly deferred)

- **Array slicing**: `${@:off:len}` and `${*:off:len}`. Require routing
  `@`/`*` through the brace lexer (currently they go through a separate
  `WordPart::AllArgs` path) AND array-aware semantics that emit multiple
  fields. Roughly doubles the iteration. Track in `docs/bash-divergences.md`
  as a follow-up under M-16's note, or as a new ID.
- **Byte-counting**: bash counts bytes; huck uses chars. Intentional —
  consistent with L-04 and avoids splitting multi-byte codepoints. Add a
  sub-bullet under L-04.
- **Silent clamping of negative computed length**: bash errors; we error too.

## AST

New variant in `ParamModifier` (`src/lexer.rs`, alongside `Substitute`):

```rust
ParamModifier::Substring {
    offset: Word,
    length: Option<Word>,
}
```

Operands are `Word`s (not pre-parsed `i64`s) so they carry variable refs,
escapes, nested `${…}`, etc. — same shape as v32's `Substitute`. Arithmetic
evaluation happens at expand time.

## Lexer

Three edits to `src/lexer.rs`:

1. **Replace** the `:` dispatch arm in `parse_braced_param`
   (currently `lexer.rs:1079-1090`):
   ```rust
   Some(':') => {
       match chars.peek().copied() {
           Some('-') => { chars.next(); /* existing UseDefault */ }
           Some('=') => { chars.next(); /* existing AssignDefault */ }
           Some('?') => { chars.next(); /* existing ErrorIfUnset */ }
           Some('+') => { chars.next(); /* existing UseAlternate */ }
           Some(_) => {
               let (offset, length) = scan_substring_operands(chars)?;
               parts.push(WordPart::ParamExpansion {
                   name,
                   modifier: ParamModifier::Substring { offset, length },
                   quoted,
               });
           }
           None => return Err(LexError::UnterminatedBrace),
       }
   }
   ```

2. **New helper** `scan_substring_operands`. Modeled on
   `scan_substitution_operand` (v32's review-fix shape): delegate to
   `scan_braced_operand` to collect the raw body (depth- and quote-aware),
   then split at the first depth-zero `:`. If no `:`, the whole body is the
   offset operand and `length = None`.

3. **Extend the digit-only branch** at `lexer.rs:1051-1067`. After reading
   the digit name, peek the next char:
   - `}` → today's `Var { name, quoted }` path (regression guard).
   - `:` (or any other modifier-start char) → fall through to the same
     modifier dispatch as named vars, producing `ParamExpansion { name,
     modifier: Substring { … }, quoted }`.
   The fall-through is mechanical: refactor the existing modifier dispatch
   into a small function that both branches call.

## Evaluator

`src/param_expansion.rs`:

1. **New `substring` helper**:
   ```rust
   fn substring(value: &str, offset: i64, length: Option<i64>)
       -> Result<String, &'static str>;
   ```
   Pure function implementing the semantics table. Returns
   `Err("substring expression < 0")` for the one error path; everything else
   returns `Ok(...)`.

2. **New arm** in `expand_modifier`:
   ```rust
   ParamModifier::Substring { offset, length } => {
       let v = shell.get(name).unwrap_or("").to_string();
       let off = match eval_arith_word(offset, shell) {
           Ok(n) => n,
           Err(()) => return ExpansionResult::Empty,
       };
       let len = match length {
           Some(w) => match eval_arith_word(w, shell) {
               Ok(n) => Some(n),
               Err(()) => return ExpansionResult::Empty,
           },
           None => None,
       };
       match substring(&v, off, len) {
           Ok(s) => ExpansionResult::Value(s),
           Err(msg) => {
               eprintln!("huck: {}: {}", name, msg);
               shell.set_last_status(1);
               ExpansionResult::Empty
           }
       }
   }
   ```

3. **`eval_arith_word`** is a small private wrapper that takes a `Word` and
   produces an `i64` for the arm. Implementation strategy: replicate the
   shape of `WordPart::Arith` handling in `src/expand.rs:187-200` — call
   `arith::eval(word, shell)`; on `Err`, print `huck: arithmetic: <msg>`
   and set `$? = 1`. (Task 4 verifies `arith::eval`'s actual public
   signature and adapts the wrapper if it takes a string rather than a
   `Word`; in that case the wrapper does `expand_assignment(word, shell)`
   first.)

**Positional-name value lookup**: Task 4 verifies whether `shell.get("1")`
already reads positionals. If not, the arm adds a small helper
`get_param_value(name, shell)` that for digit-only names returns
`shell.positional_args.get(N-1)` and for everything else falls back to
`shell.get(name)`. The shape mirrors how `$1` is already handled in
`src/expand.rs` `WordPart::Var` for digit-only names.

## Error handling

| Condition | Behavior |
|---|---|
| Unset / empty `$var` | Result = empty string (consistent with other modifiers). |
| Negative computed length | `huck: NAME: substring expression < 0`, `$? = 1`, ExpansionResult::Empty. |
| Bad arith in offset/length | Existing `huck: arithmetic: …` message from `arith::eval`, `$? = 1`, ExpansionResult::Empty. |
| Unterminated `}` | `LexError::UnterminatedBrace` at lex time. |
| Empty operand: `${var::3}` | Offset operand is empty → `arith::eval` returns an error on the empty input → standard bad-arith path (print + `$?=1` + Empty). |
| Extra colon: `${var:1:2:3}` | `scan_substring_operands` splits only on the FIRST `:`, so length operand is `2:3`. `arith::eval` rejects it as a syntax error → bad-arith path. Divergence from bash (which rejects with a dedicated message); acceptable. |

## Testing

**Lexer unit tests** (`src/lexer.rs` tests module) — ~7 tests:
- `brace_substring_simple` — `${var:1}` parses as `Substring { offset, length: None }`.
- `brace_substring_with_length` — `${var:1:3}` → `length = Some(...)`.
- `brace_substring_negative_offset_with_space` — `${var: -3}` → `Substring`.
- `brace_substring_no_space_is_use_default` — `${var:-3}` stays `UseDefault` (regression guard).
- `brace_substring_positional` — `${1:0:3}` → `ParamExpansion` (not `Var`).
- `brace_substring_nested_braced_var_in_operand` — `${var:${start}:${len}}` parses with depth-aware split.
- `brace_substring_unterminated_is_error`.

**Evaluator unit tests** (`src/param_expansion.rs` tests module) — ~12 tests:
- `substring()` helper exhaustive table from the Edge-case section.
- `expand_modifier_substring_*` — through-the-arm tests for scalar var, unset var, negative-computed-length error path, positional lookup.

**Integration tests** (new file `tests/param_substring_integration.rs`)
— ~7 tests:
- One per "interesting" row from the edge-case table.
- `subst_substring_positional_in_function` — `f() { echo "${1:0:3}"; }; f hello` → `hel`.
- `subst_substring_var_ref_in_offset` — `s=hello; n=2; echo ${s:$n}` → `llo`.
- `subst_substring_arith_in_length` — `${s:1:$((n+1))}`.
- `subst_substring_unicode` — `s=café; echo ${s:1:2}` → `af`.
- `subst_substring_inside_quotes_single_field` — preserves quoted-no-split.
- `subst_substring_in_pipeline_stage` — regression for v25 pipeline interaction.

**Total new tests:** ~26. Baseline goes from 1230 to ~1256.

## Documentation

- `docs/bash-divergences.md`:
  - M-16 status → `[fixed v33]` with a one-line summary (forms + char-counting + arith operands + bash 5.x edge-case alignment).
  - L-04 sub-bullet noting substring offset/length share the char-counting divergence.
  - Changelog row.
- `README.md`: new v33 row in the status table.

## Out of scope (recap)

- `${@:off:len}` / `${*:off:len}` array slicing — would require routing
  `@`/`*` through brace dispatch and emitting multiple fields. Defer to a
  future iteration (rename M-16 to "scalar substring done; array
  substring still open", or open a new ID).
- Byte-counting — intentional, see L-04.
- Silent clamping of negative computed length — we match bash's error.

## Implementation tasks (handoff to writing-plans)

| # | Task | Notes |
|---|---|---|
| 1 | AST scaffold + placeholder evaluator arm | Compile-clean baseline. |
| 2 | Lexer: `:` dispatch + `scan_substring_operands` + digit-name fall-through + lexer unit tests | TDD: write tests first. |
| 3 | Evaluator: `substring()` helper + exhaustive unit tests | Pure-function tests. |
| 4 | Evaluator: wire `Substring` arm into `expand_modifier` + through-the-arm tests | Resolves `eval_arith_word` + positional fetch shape. |
| 5 | Integration tests | Binary-driven via piped stdin (same harness as `tests/param_substitution_integration.rs`). |
| 6 | Docs: M-16 → fixed, L-04 sub-bullet, changelog, README row | Mechanical. |
| 7 | Full-suite verification | `cargo test` + `cargo clippy --all-targets -- -D warnings`. No separate commit. |

Process: subagent-driven per `[[huck-iteration-workflow]]` on a
`v33-substring-expansion` branch. Final code-reviewer pass over the whole
branch diff before `merge --no-ff` into `main`.
