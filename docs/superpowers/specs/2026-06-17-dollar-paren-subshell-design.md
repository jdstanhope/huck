# v177: `$((` disambiguation — command substitution of a subshell — Design

**Status:** approved 2026-06-17
**Iteration:** v177
**Origin:** The "arith-expansion termination" cluster from the parse-compat sweep.
The genuine real-world members (`timing.sh`, `zdiff`) are NOT arithmetic — they are
command substitutions whose body starts with a subshell, written glued as `$((`:
`echo $((time -p $* >/dev/null) 2>&1)`, and similar. huck mis-lexes the leading
`$((` as an arithmetic expansion and fails "unterminated arithmetic expansion".

## Problem

In `scan_dollar_expansion` (`src/lexer.rs:1736`), on seeing `$((` huck
UNCONDITIONALLY commits to arithmetic:

```rust
Some('(') => {
    chars.next(); // consume first '('
    if chars.peek() == Some(&'(') {
        chars.next(); // consume second '(' — this is `$((`
        let inner = scan_arith_body(chars)?;     // <-- always arithmetic
        let body = arith_string_to_word(&inner, opts)?;
        parts.push(WordPart::Arith { body, quoted });
    } else {
        let sequence = scan_paren_substitution(chars, opts)?;
        parts.push(WordPart::CommandSub { sequence, quoted });
    }
}
```

bash instead treats `$((` as a *tentative* arithmetic expansion: it scans for the
closing `))`; if the content does not form `$(( … ))` (e.g. the leading `(` is a
subshell whose `)` is followed by more text, not another `)`), bash reparses from
`$(` as a command substitution — so `$( (subshell) … )` written glued as
`$((subshell) … )` is a command substitution of a subshell, not arithmetic.

Confirmed (huck FAIL "unterminated arithmetic expansion", bash OK):
```
echo $((echo hi) 2>&1)      x=$((echo a) | cat)      x=$((echo a) |& cat)
```
Works already: the spaced form `$( (sub) 2>&1 )` (v101), and all real arithmetic
(`$((1+2))`, `$(( (1+2)*3 ))`, `$(( ((1+2)) ))`, `$((1>0?2:3))`).

Key enabling fact: `scan_arith_body` (`lexer.rs:1985`) returns
`Err(LexError::UnterminatedArith)` *exactly* when the first depth-1 `)` is not
immediately followed by another `)` (or on EOF). For a real `$(( … ))` it returns
`Ok` at the `))`. So its `Err` is a reliable "this is not arithmetic" signal.

## Goal

Disambiguate `$((` the way bash does: try arithmetic, and on failure fall back to a
command substitution whose body begins with a subshell.

## Design

In `scan_dollar_expansion`'s `$((` branch, clone the cursor before consuming the
second `(`, attempt `scan_arith_body`, and on `Err` rewind and reparse as a
command substitution:

```rust
if chars.peek() == Some(&'(') {
    // `$((` is EITHER an arithmetic expansion `$(( … ))` OR a command
    // substitution whose body starts with a subshell, written glued:
    // `$( (subshell) … )`. Try arithmetic; if the body does not close as
    // `))`, rewind to just after the first `(` and reparse as a command
    // substitution (the inner `(` then parses as a subshell). Mirrors bash.
    let saved = chars.clone();
    chars.next(); // consume the second `(`
    match scan_arith_body(chars) {
        Ok(inner) => {
            let body = arith_string_to_word(&inner, opts)?;
            parts.push(WordPart::Arith { body, quoted });
        }
        Err(_) => {
            *chars = saved; // rewind to just after the first `(`
            let sequence = scan_paren_substitution(chars, opts)?;
            parts.push(WordPart::CommandSub { sequence, quoted });
        }
    }
} else {
    let sequence = scan_paren_substitution(chars, opts)?;
    parts.push(WordPart::CommandSub { sequence, quoted });
}
```

`CharCursor` derives `Clone` (it is `&str` + `pos`/`line`/`peeked`), so the
clone/restore is cheap and also rewinds the line counter, keeping line tracking
consistent (`scan_paren_substitution` re-advances it over the same text).

### Why this is correct / minimal

- Real arithmetic closes with `))` → `scan_arith_body` returns `Ok` → identical to
  today. Only the non-`))` case (the subshell signal) falls back.
- Fall back ONLY on `scan_arith_body`'s `Err`, NOT on `arith_string_to_word`'s
  error: a syntactically-closed-but-invalid arith (`$((1+))`) stays arithmetic
  (matches bash, which defers that to a runtime arithmetic error).
- Execution is already correct: the fallback produces `WordPart::CommandSub {
  sequence, quoted }` (the same shape the spaced `$( (sub) … )` form already
  produces), which the executor handles.

### Behavior

- `echo $((echo hi) 2>&1)`, `x=$((cmd) | filter)`, and the glued
  `$((subshell) redirect)` family → parse and execute as command substitutions,
  matching bash.
- Unchanged: all real arithmetic expansions; the spaced `$( (sub) … )` form; a
  genuinely unterminated `$((1+2` still errors (the fallback also hits EOF — only
  the error message may read "unterminated command substitution" instead of
  "unterminated arithmetic expansion"; both are errors, this is a pathological,
  rare input).

## Verification

- **New bash-diff harness** `tests/scripts/dollar_paren_subshell_diff_check.sh` —
  EXECUTING, byte-identical bash↔huck: `echo $((echo hi) 2>&1)`; a subshell with a
  redirect inside the cmdsub whose output is captured
  (`v=$((printf X; printf Y) 2>/dev/null); echo "[$v]"`); the spaced form
  `echo $( (echo s) 2>&1 )` (regression); and real-arith regressions
  (`echo $((1+2))`, `echo $(( (1+2)*3 ))`, `echo $(( ((4)) ))`,
  `echo $((1>0?2:3))`, and an arith with a parenthesized sub-expression used in a
  real computation).
- **Parse-sweep payoff:** re-run `tools/parse_sweep.sh tools/scripts.tsv` and
  confirm the genuine arith-termination gaps (`timing.sh`, `zdiff`) clear and the
  total `HUCK_GAP` falls (≈32 → ≈29–30), with no new `HUCK_LENIENT`/`HUCK_CRASH`.
  Note that `test_cpuset_prs` (and any other `|&` user) will NOT fully clear —
  they need the separate `|&` feature; report which arith-termination scripts
  remain and why.
- Full `cargo test` (0 failures) — including the existing arith / command-sub unit
  tests — all `tests/scripts/*_diff_check.sh` harnesses green, clippy clean.

## Scope boundary

In scope: the `$((` try-arith-then-fallback-to-command-substitution disambiguation
in `scan_dollar_expansion`; the new harness; the parse-sweep confirmation. **Not**
in scope: the `|&` pipe-both shorthand (a separate unsupported feature — its own
future iteration; so `test_cpuset_prs.sh` is only partially unblocked); the
test-harness-file / Claude-history false positives in the cluster; any executor
change. No `bash-divergences.md` change (never a tracked divergence). Record in
`project_huck_iterations.md` + `MEMORY.md`.
