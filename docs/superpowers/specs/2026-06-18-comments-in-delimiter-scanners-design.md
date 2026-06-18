# v183: comments in delimiter close-finders (one shared mechanism) — Design

**Status:** approved 2026-06-18
**Iteration:** v183
**Supersedes:** the array-only `2026-06-18-array-literal-comments-design.md`
(removed). Origin: the parse sweep's "expected a command" cluster traced to a
`#` comment inside a `name=( … )` array literal whose text contains a `)`. While
designing the fix, a sibling instance of the SAME class surfaced in the `$( … )`
command-substitution close-finder. Per a code-quality directive ("we should not
have special cases throughout the code for comments"), v183 fixes the whole
class with ONE shared comment-skip mechanism rather than a localized patch.

## Problem

`#` comments are recognised in exactly ONE place today — the main tokenizer loop
(`src/lexer.rs:606`, `'#' if !has_token`): a `#` at a word boundary runs to
end-of-line. The `!has_token` guard is essential — a `#` is a comment ONLY at a
word boundary; mid-word (`x#y`), inside quotes (`'#'`), and the `#` in `$#` /
`${#x}` / `${x#pat}` are literal. Comments therefore cannot be stripped at the
raw character level; recognition needs word-boundary context.

huck has several *separate* scanners that find their own closing delimiter by
counting raw characters, bypassing the main loop. Two of them are blind to
comments, so a delimiter character inside a comment closes the construct early:

1. **`scan_array_literal`** (`:2744`) — skips inter-element whitespace/newlines
   (`skip_array_literal_whitespace`, `:2790`) but not `#` comments. A comment at
   element-start is scanned as elements and a `)` in its text closes the array
   → generic `expected a command` (a cascade). Affects ioam6.sh, sysctl.sh,
   huck's own `param_cmdsub_split_diff_check.sh`, and file-history snapshots.
2. **`scan_cmdsub_body`** (`:2081`) — the `$( … )` close-finder (also used by
   `consume_paren_cmdsub_verbatim` for `$()` inside array elements / `${…}`
   operands). It tracks paren-depth + quotes + escapes but not comments, so a
   `)` inside a comment in the body closes the substitution early.

Confirmed (parse-only): `a=(\n# has (parens) here\n)` and `x=$(echo hi  # c with
) paren\n)` both error in huck, parse in bash.

**Out of scope (separate bug, deferred):** `( … )` subshell with a
comment/blank line before the first command — `(\n# c\necho hi\n)` errors in
huck though `( # c\necho hi )` works. `(` is a real token and the body is
tokenized normally (comments already stripped), so this is NOT a comment-scanner
bug — it is `parse_subshell` not skipping leading newlines/comment-blanks.
Tracked as a new `[deferred]` divergence, not fixed here.

## bash behavior (verified)

A `#` begins a comment when the preceding character is start-of-input,
whitespace (space/tab/newline), or a metacharacter (`( ) ; & | < >`); otherwise
it is literal:

| context | bash |
|---|---|
| `a=(\n 1\n # note (paren) and ) here\n 2\n)` → `"${a[@]}"` | `1 2` |
| `a=( # lead\n x y\n)` → `"${a[@]}"` | `x y` |
| `a=(x#y a#b)` (mid-word `#`) → `"${a[@]}"` | `x#y a#b` |
| `set -- a b c; a=($#)` → `"${a[@]}"` | `3` |
| `$(echo hi  # c with ) paren\n)` (cmdsub) | `hi` |
| `$(# c with ) paren\necho hi)` (comment after `$(`) | `hi` |
| `$(echo a#b)` (mid-word) | `a#b` |
| `$(echo a;# c with ) x\necho b)` (`#` after `;`) | `a`⏎`b` |
| `$(echo a |# c ) x\ncat)` (`#` after `|`) | `a` |

## Goal

One shared comment-skip mechanism, applied at each word/element boundary
scanner, so a delimiter inside a comment never closes its construct — clearing
the array cluster AND the `$()` close-finder sibling, with no duplicated
comment logic.

## Design

### 1. Shared helper `skip_line_comment` (`src/lexer.rs`)

```rust
/// Consumes a `#` line comment's body up to (but NOT including) the terminating
/// newline; the caller's loop handles the newline. The opening `#` must already
/// be confirmed as a comment-start (word boundary) by the caller — this only
/// runs to end-of-line.
fn skip_line_comment(chars: &mut CharCursor<'_>) {
    while let Some(&c) = chars.peek() {
        if c == '\n' {
            break;
        }
        chars.next();
    }
}
```

Replace the inline comment-skip in the main tokenizer (`:606-614`) with a call:

```rust
            '#' if !has_token => {
                // POSIX: an unquoted `#` that begins a word starts a comment to
                // end-of-line. Mid-word `#` (has_token) falls through as literal.
                skip_line_comment(&mut chars);
            }
```

So there is ONE comment-body-skip implementation.

### 2. Array literal — fold comments into the separator skip

Rename `skip_array_literal_whitespace` → `skip_array_literal_separators` and make
it skip whitespace, newlines, AND `#` comments (all inter-element separators), so
`scan_array_literal`'s loop body carries NO special-case `if`:

```rust
/// Skips inter-element separators inside an array literal: whitespace, newlines,
/// and `#` comments. The post-skip position is always an element boundary (after
/// `(` or inter-element whitespace), so a `#` here is unambiguously a comment —
/// its body (incl. any `)`) must not be read as elements / close the literal.
fn skip_array_literal_separators(chars: &mut CharCursor<'_>) {
    loop {
        while let Some(&c) = chars.peek() {
            if c.is_whitespace() {
                chars.next();
            } else {
                break;
            }
        }
        if chars.peek() == Some(&'#') {
            skip_line_comment(chars);
        } else {
            break;
        }
    }
}
```

`scan_array_literal` calls `skip_array_literal_separators` at the loop head (the
v183-WIP loop-body `if` is removed; the fold-in replaces it). The single call
site changes; update any test referencing the old name.

### 3. `scan_cmdsub_body` — word-boundary-aware comment skip

`scan_cmdsub_body` collects the `$(…)` body verbatim into `out` while finding the
matching `)`. Add a word-boundary flag and, on a word-start `#`, consume the
comment to end-of-line VERBATIM into `out` (the body is re-tokenized later by
`parse_substitution_body`, which strips the comment) so its `)` is not counted:

```rust
fn scan_cmdsub_body(
    chars: &mut CharCursor<'_>,
    out: &mut String,
    unterminated: LexError,
) -> Result<(), LexError> {
    let mut depth: usize = 0;
    let mut at_boundary = true; // start of body is a word boundary
    loop {
        match chars.next() {
            None => return Err(unterminated),
            Some('#') if at_boundary => {
                // Word-start `#` is a comment to end-of-line. Keep it VERBATIM in
                // `out` (re-tokenized + stripped later) so a `)` inside it does
                // NOT close the substitution. The trailing newline (next char)
                // restores `at_boundary`.
                out.push('#');
                while let Some(&c) = chars.peek() {
                    if c == '\n' {
                        break;
                    }
                    out.push(c);
                    chars.next();
                }
            }
            Some(')') if depth == 0 => return Ok(()),
            Some(')') => {
                depth -= 1;
                out.push(')');
                at_boundary = true;
            }
            Some('(') => {
                depth += 1;
                out.push('(');
                at_boundary = true;
            }
            Some('\\') => {
                out.push('\\');
                match chars.next() {
                    Some(c) => out.push(c),
                    None => return Err(unterminated),
                }
                at_boundary = false;
            }
            Some('\'') => {
                out.push('\'');
                loop {
                    match chars.next() {
                        Some('\'') => { out.push('\''); break; }
                        Some(c) => out.push(c),
                        None => return Err(unterminated),
                    }
                }
                at_boundary = false;
            }
            Some('"') => {
                out.push('"');
                loop {
                    match chars.next() {
                        Some('"') => { out.push('"'); break; }
                        Some('\\') => {
                            out.push('\\');
                            match chars.next() {
                                Some(c) => out.push(c),
                                None => return Err(unterminated),
                            }
                        }
                        Some(c) => out.push(c),
                        None => return Err(unterminated),
                    }
                }
                at_boundary = false;
            }
            Some(c) => {
                out.push(c);
                at_boundary = c.is_whitespace()
                    || matches!(c, ';' | '&' | '|' | '<' | '>');
            }
        }
    }
}
```

(The `(`/`)` arms set `at_boundary = true` — bash treats a `#` after `(`/`)` as a
comment-start. The quote/escape arms set it `false`. The catch-all sets it from
whether `c` is whitespace or a metacharacter. The `#`-comment arm leaves it
unchanged; the next char — the newline — restores it.)

This single change covers `$()` in command position (`scan_paren_substitution`),
`$()` inside array elements / modifier operands (`consume_paren_cmdsub_verbatim`),
and the `$((`-vs-`$( (` arith disambiguation, all of which route through
`scan_cmdsub_body`. Backtick substitution (`scan_backtick_body`, which closes on
an unescaped `` ` ``, not `)`) is NOT in scope — a comment containing a backtick
inside backticks is pathological and a separate path.

### Behavior after the fix

- ioam6 / sysctl / `param_cmdsub_split` arrays parse; `$(… # c with ) …)`
  parses; all table rows above match bash.
- Controls unchanged: mid-word `#`, `$#`, `${#x}`, `${x#pat}`, quoted `'#'`.

## Verification

- **New harness** `tests/scripts/array_comment_diff_check.sh` (8 cases):
  array-literal comments containing `)`/`(`, comment right after `(`, trailing
  comment, multiple comment lines; controls mid-word `#`, `$#`, plain array. Assert
  `"${a[@]}"` results match bash. (The subscript-`#`-value case is excluded — see
  the pre-existing `[i]=#value` divergence below.)
- **New harness** `tests/scripts/cmdsub_comment_diff_check.sh` (cases): `$(… # c
  with ) paren\n…)`, comment right after `$(`, `#` after `;`/`|`, mid-word `a#b`,
  nested `$( ( … ) # c\n )`, and a backtick / plain-cmdsub control. Assert command
  RESULTS match bash.
- **Lexer unit tests**: array — `parse_assignments("a=(\n# c )\n1\n)")` → 1
  element; `a=(x#y z)` → 2 elements. cmdsub — `scan_cmdsub_body` over
  `echo hi # c with ) paren\n)` collects the whole body (comment incl. its `)`)
  and stops at the FINAL `)`; `scan_cmdsub_body` over `echo a#b)` keeps `a#b`
  (mid-word `#` not a comment). A `skip_line_comment` unit test.
- **Parse-sweep payoff:** re-run `tools/parse_sweep.sh`; confirm ioam6 ×2,
  sysctl ×2, `param_cmdsub_split_diff_check.sh` parse (`huck -n` rc 0). Report
  `HUCK_GAP` from the 20 baseline; LENIENT/CRASH stay 0; note the file-history
  snapshots.
- **Full `cargo test`** (0 failures). UP-FRONT grep `tests/` + `src/` for
  `skip_array_literal_whitespace` (the rename) and any array/cmdsub test
  encoding pre-fix behavior; update only genuine old-behavior tests.
- All `tests/scripts/*_diff_check.sh` green; clippy clean.

## Docs / close-out

No tracked divergence covers the array/cmdsub comment bug (sweep-found). Add TWO
new `[deferred]` low entries for the bugs found-but-not-fixed here:
- `( … )` subshell with a leading comment/blank line before the first command
  (`parse_subshell` newline/comment-blank skip).
- `[i]=#value` in an array literal: a subscripted element whose value begins with
  `#` is dropped (re-tokenization of the element value treats the leading `#` as
  a comment) — bash keeps it (`a=([0]=#x)` → `#x`, huck → empty).

Record the iteration in `project_huck_iterations.md` + `MEMORY.md`; update the
backlog note (the "expected a command" cluster was the array-literal-comment bug;
re-survey the remaining gaps after the sweep).

## Scope boundary

In scope: `skip_line_comment` (shared), the main-loop dedup, the array
separator-skip rename+fold, `scan_cmdsub_body` comment-awareness, the two new
harnesses + unit tests, the two new deferred-divergence entries. **Not** in
scope: the subshell `parse_subshell` bug and the `[i]=#value` bug (logged,
deferred); `scan_backtick_body`; `${…}` operand `}`-finders; the runtime; the
other sweep clusters.
