# huck v101 — subshell inside command substitution (paren balancing) Design

**Status:** approved design, ready for implementation plan.
**Implements:** a subshell `( … )` (and a nested arithmetic `$((…))`) inside a
command substitution `$( … )` now balances parens correctly — `$( (cmd) )`,
`$( (a) || b )`, `$(cmd | (sub))`, `$( $((1+2)) )`. Today huck's `$(…)` scanner
treats a bare `(` as a literal character WITHOUT incrementing paren-depth, so the
subshell's `)` is mistaken for the command-sub's closing `)`, truncating the body
mid-subshell → `syntax error in command substitution: unterminated '('`.
**Primary driver:** `~/.nvm/nvm.sh`'s `nvm_resolve_alias` (line 1287):
`ALIAS_TEMP="$( (nvm_alias … | command head … | command tail …) || nvm_echo)"`.
The next nvm parse blocker.
**Closes:** a new low Tier-2 entry (command-sub subshell paren balancing)
`[fixed v101]`.
**Branch (impl):** `v101-cmdsub-subshell-paren`.

## Root cause (verified)

`scan_paren_substitution` (`src/lexer.rs:1637`) scans a `$(…)` body by tracking
`depth` and closing on a `)` at depth 0. The `$` arm increments `depth` for a
nested `$(` (`:1699`), and the `)` arm decrements for `depth>0`. But the bare-`(`
arm (`:1651`) only pushes the char:
```rust
'(' => {
    // Bare `(` is just a character. huck has no subshell
    // `(cmd)` syntax — only `$(` increments depth (handled in the `$` arm below).
    body.push(c);
}
```
The comment is stale — huck has had subshell `(cmd)` syntax since v28. So for
`$( (echo a) )`, the inner `(` does not raise depth; the subshell's `)` hits the
`depth==0` arm and closes the command-sub early with body `" (echo a"`, which
fails to re-parse (`unterminated '('`). The same truncation hits `$( $((1+2)) )`
(nested arith — also currently broken).

**Audit of sibling paren-counters (all already correct, no change needed):**
`scan_arith_block` (`:1437`), `read_array_element_word` (`:2156`), and the extglob
`flush` (`:792`) all `depth += 1` on `(`. The main tokenizer's `(` (`:446`) emits
the `LParen` operator (subshell open) — unrelated. The backtick scanner doesn't
paren-count (scans to the matching `` ` ``) — `` `(echo a)` `` already works.
Failing cases like `${x:-$( (a) )}` and `a=( "$( (x) )" )` fail ONLY because they
contain a `$(…)` routed through the buggy `scan_paren_substitution` — fixed
transitively by the single change.

## Section 1 — The fix (`src/lexer.rs`)

In `scan_paren_substitution`, make a bare `(` increment `depth` and drop the stale
comment:
```rust
'(' => {
    depth += 1;
    body.push(c);
}
```
Now every unquoted `(` (subshell open, or the second `(` of a `$((…))`) raises
depth and its matching `)` lowers it, so the command-sub closes only at the true
depth-0 `)`. `(` inside `'…'`/`"…"`/after `\` is already consumed by the existing
quote/backslash arms (never reaches the `(` arm). Note: a `$((…))` whose `$(` was
already counted by the `$` arm will have its SECOND `(` counted by this arm
(depth +2) — balanced by the two `)` (depth −2), so it stays correct.

## Section 2 — No other changes

Lexer-only, one arm. No parser/executor/AST change. `parse_substitution_body`
re-tokenizes and parses the now-complete body, so the inner subshell is parsed by
the normal pipeline/subshell path (which already works).

## Edge cases & notes

- **`case` pattern bare `)` inside `$(…)`** (`$( case x in a) echo ;; esac )`):
  an unmatched `)` (the pattern terminator) at depth 0 still closes the
  command-sub early. This is a PRE-EXISTING limitation (naive paren-counting
  can't distinguish a case-pattern `)` from the closing `)`; bash uses full
  recursive parsing). NOT worsened by this fix, NOT in scope — documented as a
  remaining low divergence. nvm does not use this inside a `$(…)`.
- **`$((…))` arithmetic inside `$(…)`** is fixed as a bonus (was also broken).
- **No regression for non-`(` command-subs**: `$(echo a)`, `$( $(echo a) )`
  (nested), `` `(echo a)` `` (backtick) are unaffected — only the bare-`(` arm
  changes, and it now matches the already-correct `$(`/arith handling.

## Files & responsibilities

| File | Change |
|------|--------|
| `src/lexer.rs` | `scan_paren_substitution`: bare `(` increments `depth`; remove the stale comment |
| `tests/cmdsub_subshell_integration.rs` | NEW — `$( (cmd) )`, `$( (a) || b )`, `$(cmd | (sub))`, nested-arith, subshell-in-`${:-}`/array-literal, regression |
| `tests/scripts/cmdsub_subshell_diff_check.sh` | NEW — 26th bash-diff harness |
| `docs/bash-divergences.md`, `README.md` | new Tier-2 entry `[fixed v101]`; the case-pattern remaining-edge note; changelog; README row |

## Testing

1. **Lexer unit test**: `tokenize("$( (echo a) )")` succeeds and produces a
   `CommandSub` whose inner sequence's first command is a `Subshell` (or at least:
   it lexes without `UnterminatedSubstitution`/`Substitution` error). Mirror
   neighboring lexer tests.
2. **Integration** (`tests/cmdsub_subshell_integration.rs`) — stdout vs bash:
   - `echo "$( (echo a) )"` → `a`.
   - `echo "$( (echo a) || echo b )"` → `a` (the nvm shape).
   - `echo "$(echo a | (cat))"` → `a`.
   - `echo "$( (exit 3); echo done )"` → `done`.
   - nested arith: `echo "$( echo $((1 + 2)) )"` → `3`.
   - subshell in default: `echo "${x:-$( (echo d) )}"` → `d`.
   - subshell in array literal: `a=( "$( (echo x) )" ); echo "${a[0]}"` → `x`.
   - the exact nvm shape: `r="$( (printf 'a\\nb\\n' | head -n 1) || echo z )"; echo "$r"` → `a`.
   - regression: `echo "$(echo a)"`, `echo "$(echo "$(echo a)")"`,
     `` echo "`(echo a)`" `` all unchanged.
3. **bash-diff harness** `tests/scripts/cmdsub_subshell_diff_check.sh` (26th):
   the deterministic forms above byte-identical to bash 5.2.
4. **Regression**: full suite — especially the command-substitution, subshell,
   pipeline, arith, and array suites.
5. **End-to-end**: re-bisect `nvm.sh` — `nvm_resolve_alias` (de-wrapped line
   1287) now parses; report the next gap (if any).
