# v183: comments inside array literals — Design

**Status:** approved 2026-06-18
**Iteration:** v183
**Origin:** The parse sweep's "expected a command" cluster — the largest
remaining group. Five real files share ONE root cause: a multi-line array
literal `name=( … )` containing a `#` comment whose text includes a `)`:
- `tools/testing/selftests/net/ioam6.sh` (×2 header copies): `ALPHA=( … #
  Schema ID (0xffffff = None)\n)`.
- `tools/testing/selftests/sysctl/sysctl.sh` (×2): `local magnitudes=( … # and
  INT_MAX respectively) if truncated…)`.
- huck's own `tests/scripts/param_cmdsub_split_diff_check.sh`: `fragments=( …
  # --- plain forms (must be unchanged by the refactor) ---\n)`.
- (Two `~/.claude/file-history` snapshots of harness files — same construct.)

## Problem

`scan_array_literal` (`src/lexer.rs:2744`) scans the body of a `name=( … )`
array literal. Each loop iteration calls `skip_array_literal_whitespace` (which
skips whitespace AND newlines, `:2790`) and then either closes on `)`, errors on
EOF, or parses one element via `scan_array_element_word`. It does NOT recognise
`#` comments. So when the element-start position lands on a `#` comment line,
the `#` is scanned as an element word, and the FIRST `)` inside the comment text
(`scan_array_element_word` breaks on an unquoted `)`, `:2819`) is taken as the
array's closing paren — ending the literal early. The leftover tail (` here\n)…`)
then fails to parse, surfacing the generic `syntax error: expected a command`
(reported at the real `)` or a later construct — it is a cascade, e.g. sysctl's
error lands at `run_wideint_tests()` on line 323 though the bad array is at 330).

Confirmed minimal trigger (parse-only): `a=(\n  # has (parens) here\n)\necho ok`
errors in huck (`expected a command`), parses in bash. `a=(\n  # hello\n)` (a
comment with NO paren) already works — only a `)`/`(` inside the comment breaks
the paren accounting.

## bash behavior (verified)

A `#` at an **element-start position** — immediately after `(`, or after
inter-element whitespace/newline — begins a comment to end-of-line. The `)`
inside such a comment does NOT close the array. A `#` elsewhere is literal:

| fragment | bash `"${a[@]}"` |
|---|---|
| `a=(\n 1\n # note (paren) and ) here\n 2\n)` | `1 2` |
| `a=( # lead\n x y\n)` (comment right after `(`) | `x y` |
| `a=(\n p q # trailing ) brace\n r\n)` | `p q r` |
| `a=(x#y a#b)` (mid-word `#`) | `x#y a#b` |
| `set -- a b c; a=($#)` | `3` |
| `a=([0]=#x [1]=y)` (subscript value) | `#x` / `y` |

## Goal

Skip `#` comments at element-start in `scan_array_literal`, matching bash, so a
`)` inside a comment no longer closes the literal — clearing the cluster.

## Design

In `scan_array_literal`'s loop, right after `skip_array_literal_whitespace` and
before the `)` / EOF / subscript handling, add a `#`-comment skip:

```rust
        skip_array_literal_whitespace(chars);
        // A `#` at element-start (right after `(` or inter-element whitespace)
        // begins a comment to end-of-line — bash allows comments between array
        // elements, and a `)` inside the comment must NOT close the literal.
        // Mid-word `#` (`x#y`), `$#`, and `[i]=#x` never reach here (the word /
        // `$` / `[` is hit first), so those stay literal. Skip to end-of-line
        // and re-loop; the next `skip_array_literal_whitespace` eats the `\n`.
        if chars.peek() == Some(&'#') {
            while let Some(&c) = chars.peek() {
                if c == '\n' {
                    break;
                }
                chars.next();
            }
            continue;
        }
        match chars.peek() {
            Some(&')') => {
                chars.next();
                return Ok(elements);
            }
            None => return Err(LexError::UnterminatedArrayLiteral),
            _ => {}
        }
```

### Why this is the right position

The loop's post-whitespace-skip peek is ALWAYS an element boundary (the only
ways to reach it are after the opening `(` or after `scan_array_element_word`
returns + whitespace-skip). So a `#` there is unambiguously a comment, exactly
as bash treats a `#` at word-start. Mid-word `#`, `$#` (the `$` is matched
first), and `[i]=#x` (the `[` subscript is matched first, then `#x` is the value
via `scan_array_element_word`) never present a bare leading `#` here, so they
are untouched. A comment at EOF with no trailing newline consumes to EOF, then
the re-loop hits `None` → `UnterminatedArrayLiteral` (bash also errors).

### Behavior after the fix

- ioam6 `ALPHA=(…)`, sysctl `magnitudes=(…)`, the `param_cmdsub_split` harness's
  own `fragments=(…)`, and the file-history snapshots all parse: `huck -n`
  silent, rc 0 = bash.
- The `"${a[@]}"` results for all table rows above match bash.

## Verification

- **New bash-diff harness** `tests/scripts/array_comment_diff_check.sh`
  (executing, byte-identical bash↔huck stdout+exit): the table cases above —
  comment-with-`)`/`(` inside the array (→ `1 2`), comment right after `(` (→
  `x y`), trailing comment after an element (→ `p q r`), and controls mid-word
  `#` (→ `x#y a#b`), `$#` (→ `3`), subscript value `[0]=#x` (→ `#x`). NOTE: the
  harness itself must AVOID a bare comment-with-`)` at array-element-start in its
  OWN top-level code if it predates the fix being built — keep such constructs
  inside the single-quoted test fragments, not the harness scaffolding. Compare
  stdout+exit (clean output, no intentional-stderr cases).
- **Lexer unit test** (`src/lexer.rs` `mod tests`): `tokenize("a=(\n# c )\n1\n)")`
  succeeds (is Ok) and yields an array-literal Word whose elements are exactly
  `[1]` (the comment and its `)` skipped). Add a second asserting a mid-word
  `tokenize("a=(x#y)")` keeps `x#y` as one element (regression: the fix must not
  treat mid-word `#` as a comment).
- **Parse-sweep payoff:** re-run `tools/parse_sweep.sh tools/scripts.tsv
  tools/parse_results.tsv`; confirm ioam6 ×2, sysctl ×2, and
  `param_cmdsub_split_diff_check.sh` now parse (`huck -n` rc 0, no stderr — note
  any that fail on a DIFFERENT construct as a derail/follow-on). Report
  `HUCK_GAP` movement from the 20 baseline; `HUCK_LENIENT`/`HUCK_CRASH` stay 0;
  report whether the two `file-history` snapshots also clear.
- **Full `cargo test`** (0 failures). UP-FRONT (v178/v180/v181/v182 lesson) grep
  all of `tests/` + `src/` for array-literal tests that might encode the
  pre-fix behavior (e.g. a test feeding a `#` inside an array expecting it
  literal). Update only ones that encode the old bug; do not weaken unrelated
  tests.
- All `tests/scripts/*_diff_check.sh` green; clippy clean.

## Docs / close-out

No tracked `M-*`/`L-*` divergence covers this (sweep-found). No
`bash-divergences.md` change. Record the iteration in `project_huck_iterations.md`
+ `MEMORY.md`, and update the backlog note (the "expected a command" cluster was
the array-literal-comment bug; once cleared, re-survey the remaining gaps —
some "expected a command" rows may have been cascades of THIS bug).

## Scope boundary

In scope: the `#`-comment skip in `scan_array_literal`, the new harness + lexer
unit tests. **Not** in scope: `scan_array_element_word` (mid-word `#` stays
literal), `skip_array_literal_whitespace`, subscript parsing, the runtime, the
other sweep clusters (`unterminated '((' arith`, `unterminated 'case`,
`unexpected token after command`, `parameter expansion with empty name`,
`function definition`). No `bash-divergences.md` change.
