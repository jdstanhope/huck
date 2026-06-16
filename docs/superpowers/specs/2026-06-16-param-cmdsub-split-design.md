# v165: `$()`-aware `${…}` operand split (fix L-10) — Design

**Status:** approved 2026-06-16
**Iteration:** v165 (v164 was abandoned — see
`2026-06-16-rc-cow-vars-design.md`)
**Origin:** the 2026-06-16 architecture review flagged the `${…}` operand split
scanners as duplicated logic that directly causes divergence **L-10**. This is
improvement #3 of the post-review sequence (lib.rs shipped v163; Rc-COW vars
abandoned v164).

## Goal

Make the `${var/pat/repl}` and `${var:off:len}` operand splitters skip over
command substitutions (`$(…)`, `$((…))`, backticks) and the other already-handled
spans (single/double quotes, `{}` braces) when locating the modifier delimiter,
so a `/` or `:` *inside* a command substitution is no longer mistaken for the
delimiter. Collapse the two near-identical splitters into one shared helper so
the fix lives in a single place.

## Problem (verified against the current binary)

`${var/pat/repl}` and `${var:off:len}` are split by two functions in
`src/lexer.rs`:

- `split_substitution_body` (~line 3098) — splits on the first unescaped `/`.
- `split_substring_body` (~line 3172) — splits on the first unescaped `:`.

Both track `{}` brace depth, single quotes, double quotes, and `\`-escapes — but
**neither tracks `$(…)` / `$((…))` / backticks**. So a delimiter inside a command
substitution is treated as the operand delimiter, which splits the command
substitution apart, leaving an unbalanced `$(` / `` ` `` that the operand
re-parse then rejects. The observed symptom is **not** a silent mis-split (as
L-10's text describes) but a hard error:

| Input | bash | huck (current) |
|---|---|---|
| `s=abcdefgh; "${s:$(echo 1:2 \| cut -d: -f1)}"` | `bcdefgh` | `syntax error: unterminated command substitution` |
| `s=a-b-c; "${s/$(echo a/x)/Z}"` | `a-b-c` | `syntax error: unterminated command substitution` |
| `s=a-b; "${s/`echo a/x`/Z}"` (backtick) | `a-b` | `syntax error` |
| `s=xyz; "${s/$(echo $(echo a/b))/Q}"` (nested) | `xyz` | `syntax error` |

These are scripts that simply fail under huck. (`$((1+1))`-style operands happen
to work today only because that example has no delimiter inside the arithmetic;
a `$(( a?b:c ))` ternary in a substring operand would break under the current
code and is fixed by this change.)

The two functions are otherwise byte-for-byte structurally identical, differing
only in the delimiter char (`/` vs `:`), the escaped-delimiter handled
(`\/` vs `\:`), and the return shape. The duplication is what let them drift out
of sync with the main lexer's `$(`-awareness in the first place.

## Design

### One shared, command-substitution-aware split helper

Add to `src/lexer.rs`:

```rust
/// Splits a `${…}` modifier operand body on the FIRST top-level `delim`,
/// returning `(before, Some(after))` if a delimiter was found at the top
/// level, or `(before, None)` otherwise.
///
/// "Top level" means: not inside single quotes, double quotes, backticks, a
/// `$(…)` command substitution (nested parens — also covers `$((…))` and
/// `$( (…) )`), or `{…}` braces. Everything skipped is appended VERBATIM so the
/// returned segments re-parse exactly as written. At the top level only,
/// `\delim` un-escapes to `delim` and `\\` un-escapes to `\`; any other `\x`
/// keeps the backslash. Inside a command substitution, escapes are verbatim
/// (they belong to the command), mirroring `scan_paren_substitution`.
fn split_modifier_operand(body: &str, delim: char) -> (String, Option<String>)
```

State machine over a `CharCursor` (the project's byte+line cursor), tracking:

- `paren_depth: u32` — raised by `$(` (a `$` immediately followed by `(`) and by
  any `(` while already `> 0`; lowered by `)` while `> 0`. While `paren_depth >
  0` the scanner is inside a command substitution: delimiters are ignored,
  escapes are verbatim, and `{}` are literal.
- `brace_depth: u32` — raised/lowered by `{`/`}` only while `paren_depth == 0`
  (preserves the existing `${X:-${Y}}` plain-nesting behavior).
- Quote/backtick spans — on `'`, `"`, or `` ` `` at any point, the matching span
  is consumed and appended verbatim (double-quote and backtick spans honor `\`
  escapes inside, matching `scan_paren_substitution`).

The first top-level `delim` (with `paren_depth == 0 && brace_depth == 0` and no
delimiter yet seen) flips to the second segment and is **not** appended.

### The two callers become thin wrappers (return types preserved)

```rust
fn split_substring_body(body: &str) -> (String, Option<String>) {
    split_modifier_operand(body, ':')
}

fn split_substitution_body(body: &str) -> (String, String) {
    let (pattern, replacement) = split_modifier_operand(body, '/');
    (pattern, replacement.unwrap_or_default())
}
```

`split_substitution_body` keeps returning `(String, String)` — bash treats
`${var/pat}` (no replacement) the same as `${var/pat/}` (empty replacement), so
`None` collapses to `""`, matching the current behavior.

No other call sites change: `scan_substitution_operands` /
`scan_substring_operands` (and the `${var/…}` modifier path) call these two
wrappers exactly as before.

## Out of scope — logged as a new deferred divergence (L-52)

`scan_braced_operand` (the function that extracts the `${…}` operand body up to
the matching `}`) tracks `{}` depth and quotes but **not** `$(…)`, so a command
substitution whose body contains a literal `}` truncates the operand early:
`s=a}b; "${s/$(echo a}b)/Z}"` → bash `Z`, huck `syntax error`. Same
syntax-error symptom, a different scanner, and a markedly rarer trigger (a
literal `}` inside a command substitution inside a `${…}` operand). It is
explicitly **not** fixed here; a new `L-52 [deferred]` entry records it. The
narrow split fix fully covers the common L-10 cases (delimiter inside `$(…)` /
backticks / `$((…))` / nesting, with no `}` inside the command substitution).

## Verification

### Correctness

- **New bash-diff harness** `tests/scripts/param_cmdsub_split_diff_check.sh`
  asserting byte-identical output between bash and huck for: the L-10 substring
  and substitution cases; `:`/`/` inside `$(…)`, `$((…))` (incl. a ternary
  `a?b:c`), and backticks; nested `$( $() )`; a quoted delimiter inside the
  operand (`${s/"a/b"/x}`); an escaped delimiter (`${s/a\/b/x}`); and the
  plain no-command-substitution forms (`${s:2:3}`, `${s//./X}`,
  `${s#pre}`, `${s/pat/repl}`) to pin that the refactor preserves current
  behavior. This becomes the 92nd harness.
- **Unit tests** in `src/lexer.rs` for `split_modifier_operand` directly: top-level
  split, delimiter inside `$(…)`/backtick/`$((…))`, nested parens, escaped and
  quoted delimiter, no-delimiter (`None`) case, and the `{}`-brace plain-nesting
  case.
- **Full regression:** the entire unit suite, all integration tests, and all
  existing bash-diff harnesses must stay green, and `cargo clippy --lib --bins`
  clean. The existing parameter-expansion harnesses and unit tests guard that
  the no-command-substitution behavior is byte-identical before and after.

### Behavior preservation

The refactor must be behavior-preserving for every input that does **not**
contain a command substitution in the operand: such inputs never raise
`paren_depth`, so the new state machine reduces to the existing `{}`-depth +
quote + escape logic. The L-10 cases are the only intended behavior change (from
`syntax error` to bash-matching output).

## Docs / iteration close-out

- On merge, **delete** the L-10 entry from `docs/bash-divergences.md` (resolved —
  the doc tracks only current divergences) and **add** the new `L-52 [deferred]`
  entry for the `scan_braced_operand` sibling. Adjust the Tier-4 count.
- Record the iteration in `project_huck_iterations.md` + `MEMORY.md` and mark
  improvement #3 of the architecture-review sequence done.

## Scope boundary

Per the approved scope decision, this iteration touches **only** the two `${…}`
operand splitters (via the new shared helper) plus tests/docs. It does **not**
add general quote/balanced-span helpers to `CharCursor`, does **not** modify
`scan_braced_operand`/`scan_paren_substitution`/`scan_regex_operand`, and does
**not** migrate `arith.rs` (the L-24 follow-on). Those remain candidates for a
later, separately-scoped iteration.
