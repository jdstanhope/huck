# huck v104 — Linear-time script source reader (M-99) Design

**Status:** approved design, ready for implementation plan.
**Implements:** replacing the O(n²) line-accumulation + per-line `classify`
loop in `run_sourced_contents` with a **tokenize-once, parse-and-execute one
command at a time** reader. Scripts (`source`/`.`, `huck SCRIPT`, `huck -c`,
`--rcfile`) become **O(n)** to read. `classify`/`continuation` is no longer
called on the script path — it remains exclusively for the interactive
line-at-a-time reader (`read_logical_command`), where it is correct.
**Why now:** the v102 milestone made `~/.nvm/nvm.sh` parse with zero syntax
errors, but sourcing it appears to *hang*. v103 (`set -x`) + this session's
systematic debugging proved the "hang" is **not** a hang and **not** a runtime
issue — it is **super-linear (≈O(n²)) parse time**. nvm.sh wraps its entire
4619-line body in one `{ … }` brace group → a single logical command → the
per-line `classify` call re-lexes and double-re-parses the whole accumulated
buffer on every physical line.
**Closes:** new bug **M-99** `[fixed v104]` (Tier-1, performance).
**Branch (impl):** `v104-linear-source-reader`.

## Root cause (verified this session)

`run_sourced_contents` (`src/builtins.rs:4960`) reads the script line by line:

```rust
for line in contents.lines() {
    buf.push_str(line); buf.push('\n');
    if let Completeness::Incomplete(_) = classify(&buf, extglob) { continue; } // ← per line
    … tokenize(buf) … parse … execute …
}
```

`classify` (`src/continuation.rs:44`) **tokenizes** the whole `buf`, then
**parses** it twice (`parse(tokens.clone())` then `parse(tokens)`). For a
logical command spanning *L* physical lines this runs `1+2+…+L` times over an
ever-growing buffer = **O(L²)** lex+parse work.

**Evidence (clean, isolated):**

| Input | Time |
|---|---|
| 800 simple commands at top level (each its own 1-line logical command) | 0.13 s — linear |
| The same 400 commands wrapped in `f() { … }` (one logical command) | 21.5 s — quadratic |
| Complete `nvm()` function alone (1506 lines, one logical command) | does not finish in 40 s |
| `nvm()` truncated to 337 / 437 / 487 lines | 2.2 / 4.1 / 6.0 s |

Top-level resets `buf` each line → `classify` always sees 1 line → linear. A
multi-line construct accumulates → `classify` sees the whole growing buffer →
quadratic. The fix is to stop using `classify` on a script: the whole text is
already in memory, so the parser can consume complete commands directly.

## Verified design constraints (this codebase)

- `command::parse(tokens) -> Result<Option<Sequence>, ParseError>` already
  parses the **whole** token stream into one `Sequence` (`src/command.rs:577`).
- `parse_sequence` treats a top-level `Token::Newline` as a *continue* (Semi)
  connector, so it never stops mid-program. Parse-per-unit needs an opt-in
  "stop at a top-level newline" mode.
- Tokens carry **no** source offsets; `Token` is an enum used by ~399 callers,
  so it must **not** gain a field. Offsets are delivered as a parallel sidecar.
- Aliases are **not** a script concern: `run_sourced_contents` never calls
  `expand_aliases_in_tokens` (that is the interactive path, `src/shell.rs:566`;
  `expand_aliases` defaults off non-interactively).
- `extglob` **is** a script concern: the current loop re-reads the `extglob`
  shopt per logical command (`src/builtins.rs:4981`), so an early
  `shopt -s extglob` affects how later lines lex (bash_completion relies on
  this). This must be preserved.

## Section 1 — Lexer: offset sidecar (`src/lexer.rs`)

Add an additive entry that returns byte offsets alongside the tokens; the
existing `tokenize` / `tokenize_with_opts` are untouched (399 callers safe).

```rust
/// Byte offset (into `input`) of each token's start, plus a trailing
/// sentinel `offsets[tokens.len()] == input.len()`. On a lex error, the
/// `usize` is the byte offset where lexing failed.
pub fn tokenize_with_offsets(
    input: &str,
    opts: LexerOptions,
) -> Result<(Vec<Token>, Vec<usize>), (LexError, usize)>;
```

- `offsets.len() == tokens.len() + 1`; `offsets[i]` = byte start of `tokens[i]`;
  `offsets[tokens.len()] = input.len()`.
- Implemented by recording the cursor's byte position at the start of each token
  in the lexer's main loop (and on error). `tokenize_with_opts` may be
  refactored to delegate to the offset-tracking core and discard the sidecar, so
  there is a single tokenizer (no duplicated logic) — provided its existing
  `Result<Vec<Token>, LexError>` signature and output are byte-identical.

## Section 2 — Parser: stop-at-top-newline mode (`src/command.rs`)

`parse_sequence` gains an internal "unit" mode without changing its 1 existing
public behavior or its many compound-body callers:

- Keep `parse_sequence(iter, stop_at)` as a thin wrapper that calls a new
  `parse_sequence_opts(iter, stop_at, /*stop_at_top_newline=*/false)` — so every
  current caller (compound bodies, `parse`) is byte-identical.
- In `parse_sequence_opts`, the **only** change when `stop_at_top_newline` is
  true: in the `Token::Op(Semi) | Token::Newline` arm, if the token is a
  `Newline` (not `;`), **consume it and break** instead of continuing. `;`,
  `&&`, `||`, `&`, and compound-internal newlines are unchanged — so a unit is a
  top-level command list terminated by an unescaped top-level newline or EOF.
  This is exactly the granularity of the old `classify`-bounded logical command
  (e.g. `a; b` on one line stays one unit; `a &&\nb` stays one unit because the
  newline after `&&` is consumed inside the And arm, not at loop top; a
  multi-line `if … fi` stays one unit).

New public entry for the source reader:

```rust
/// Parse ONE top-level command unit, stopping at (and consuming) the next
/// top-level newline or EOF. Returns Ok(None) when only separators / EOF
/// remain. Leading blank lines / bare `;` / `&` separators before the unit
/// are skipped.
pub fn parse_one_unit<I: Iterator<Item = Token>>(
    iter: &mut std::iter::Peekable<I>,
) -> Result<Option<Sequence>, ParseError>;
```

`parse_one_unit` skips leading `Newline`/`Semi` tokens; if the iterator is then
empty, returns `Ok(None)`; otherwise calls `parse_sequence_opts(iter, &[],
true)` and wraps the result in `Some`. The parser itself remains generic over
`Peekable<I>` and otherwise unchanged.

## Section 3 — The new source reader (`src/builtins.rs::run_sourced_contents`)

Replace the line loop with a tokenize-once / parse-per-unit / re-lex-on-extglob
loop. **Position bookkeeping uses `Peekable::len()`** (`IntoIter<Token>` is
`ExactSizeIterator`, so `Peekable<IntoIter<Token>>::len()` is the count of
not-yet-consumed tokens — the peeked token still counts as remaining). The
parser stays unchanged; offsets are indexed by `total - iter.len()`.

Pseudocode (semantics, not literal):

```
let mut last_status = shell.last_status();
let mut start = 0usize;            // absolute byte offset of the unconsumed remainder
let mut prev_end = 0usize;         // absolute byte offset already echoed for set -v
'outer: loop {
    let extglob = shell.shopt_options.get("extglob").unwrap_or(false);
    let (tokens, offsets) = match tokenize_with_offsets(&contents[start..], LexerOptions{extglob}) {
        Ok(t) => t,
        Err((e, fail_off)) => {
            report lex error at line(start + fail_off);
            last_status = 2;
            // resync: skip to the next physical line in the source, re-lex.
            start = next_newline_after(contents, start + fail_off);   // = idx after '\n', or len
            prev_end = start;
            if start >= contents.len() { break; }
            continue 'outer;
        }
    };
    let total = tokens.len();
    if total == 0 { break; }
    let mut iter = tokens.into_iter().peekable();
    loop {
        let unit_start_idx = total - iter.len();
        match command::parse_one_unit(&mut iter) {
            Ok(None) => { start = contents.len(); break 'outer; }   // only separators / EOF left
            Ok(Some(seq)) => {
                let unit_end_idx = total - iter.len();
                let unit_end_abs = start + offsets[unit_end_idx];
                if shell.shell_options.verbose {
                    // echo everything since the previous unit end (incl. blank/comment lines),
                    // BEFORE executing, using the current verbose state — matches old set -v.
                    eprint!("{}", &contents[prev_end .. unit_end_abs]);
                }
                let span = &contents[start + offsets[unit_start_idx] .. unit_end_abs];
                let outcome = executor::execute(&seq, shell, span);
                prev_end = unit_end_abs;
                match outcome { … identical ExecOutcome handling as today … }
                // extglob may have flipped (shopt -s/-u extglob). If so, re-lex the
                // remainder under the new setting so later lines lex correctly.
                let new_extglob = shell.shopt_options.get("extglob").unwrap_or(false);
                if new_extglob != extglob {
                    start = unit_end_abs;            // = start + offsets[unit_end_idx]
                    prev_end = start;
                    if start >= contents.len() { break 'outer; }
                    continue 'outer;
                }
            }
            Err(e) => {
                report parse error at line(start + offsets[unit_start_idx]);
                last_status = 2;
                // resync: consume tokens up to & including the next top-level Newline,
                // then continue with the rest of this token batch.
                while let Some(t) = iter.next() { if matches!(t, Token::Newline) { break; } }
                prev_end = start + offsets[total - iter.len()];
            }
        }
        if iter.peek().is_none() { start = contents.len(); break 'outer; }
    }
}
ExecOutcome::Continue(last_status)
```

Notes:
- **ExecOutcome handling is copied verbatim** from the current loop (Exit →
  return; FunctionReturn → return Continue(n); Continue(c) with the non-
  interactive `take_pending_fatal_pe_error` mid-loop abort; LoopBreak/Continue
  → last_status = 0). No semantic change.
- **`line(abs_offset)`** = `1 + contents[..abs_offset].bytes().filter(|&b| b ==
  b'\n').count()`. Errors are rare, so the per-error scan is fine. This
  reproduces v94's `cmd_start_line` (the line where the unit began).
- **`set -v`** echoes `contents[prev_end .. unit_end]` (the source bytes since
  the previous unit, including blank/comment/separator lines, which already end
  in `\n`) via `eprint!` — byte-identical to the old per-physical-line
  `eprintln!("{line}")`, including "enabling `set -v` not echoed, `set +v` is".
- **`set -x`** (xtrace) is unaffected — it traces at execution time inside
  `run_exec_single`, independent of the reader.
- The `physical_line` / `cmd_start_line` counters and the `classify` import are
  removed from this function.

## Section 4 — Untouched

- `read_logical_command` (`src/shell.rs:395`) — interactive / piped-stdin REPL —
  keeps `classify` and line-at-a-time reading. No change.
- `continuation::classify` and the whole `continuation` module — unchanged
  (still used by the REPL).
- `command::parse`, `executor::execute`, every compound-body parse path — the
  `parse_sequence` wrapper preserves them byte-identically.

## Files & responsibilities

| File | Change |
|------|--------|
| `src/lexer.rs` | Add `tokenize_with_offsets`; refactor `tokenize_with_opts` to delegate to an offset-tracking core (output byte-identical) |
| `src/command.rs` | `parse_sequence_opts(stop_at, stop_at_top_newline)` + `parse_sequence` wrapper; `parse_one_unit` public entry |
| `src/builtins.rs` | Rewrite `run_sourced_contents` body: tokenize-once / parse-per-unit / re-lex-on-extglob; preserve line-numbers, `set -v`, errexit/exit/return, fatal-PE abort, continue-past-error |
| `tests/linear_source_reader_integration.rs` | NEW — behavior + a large-input timing guard |
| `tests/scripts/source_reader_diff_check.sh` | NEW — 29th bash-diff harness |
| `docs/bash-divergences.md`, `README.md` | M-99 `[fixed v104]`; changelog; README row; any L-note |

## Testing

1. **Performance (the bug):** a generated input of a single ~2000-line `{ … }`
   brace group (or `f() { … }`) of trivial commands must parse+run in well under
   1 s. Add an integration test asserting completion under a generous wall-clock
   bound (e.g. 5 s) for an input that previously took minutes. Also assert the
   real `~/.nvm/nvm.sh` *parses+defines* under a few seconds when its top-level
   `nvm_process_parameters "$@"` invocation is stubbed/removed (define-only), as
   a regression guard (gated/skipped if the file is absent — do not depend on a
   user file in CI).
2. **Granularity equivalence (vs bash, byte-identical):** `a; b`, `a && b`,
   `a || b`, `a && b || c`, `a & b` (background then foreground), `cmd &`
   (trailing background), multi-line `if/then/fi`, `while/do/done`, `case/esac`,
   `for`, `{ … }`, `( … )`, a function definition then a call, and a heredoc
   (`cat <<EOF … EOF`) — each producing identical stdout/exit to the old path
   and to bash.
3. **`set -v`** (`2>&1` vs bash): enabling line not echoed, `set +v` echoed,
   blank/comment lines echoed, multi-line construct echoed line-for-line.
4. **`set -e` / `exit N` / function `return N` / traps**: a script that exits
   mid-way stops there; `set -e` aborts on first failure; `EXIT` trap fires —
   all identical to today.
5. **`set -u`** fatal mid-script abort (non-interactive) still aborts (the
   `take_pending_fatal_pe_error` path).
6. **Mid-file `extglob`:** `shopt -s extglob` on an early line, then a later
   top-level `case x in @(a|b)) echo hit ;; esac` (and an extglob pattern in
   `[[ ]]`) lexes correctly — and a `shopt -u extglob` later turns it back off.
   Byte-identical to bash. (This is the re-lex-on-flip path; assert it does NOT
   regress vs the current per-command behavior.)
7. **Syntax-error: report + continue:** a script `echo a\n)bad(\necho b` reports
   one syntax error with the correct line number (line 2) and still prints `a`
   and `b`. Confirms continue-past-error and line-number fidelity.
8. **Error line numbers (v94):** a syntax error on line N of a multi-line file
   reports `huck: FILE: line N: syntax error: …` exactly as before.
9. **Regression:** full suite (2653+), all 28 existing bash-diff harnesses, and
   `huck --rcfile ~/.bashrc` in a pty still loads.

## Edge cases & notes

- **Lex-error line precision:** `tokenize_with_offsets` returns the failure byte
  offset, so an unterminated quote/heredoc at EOF reports an accurate line
  (matching or improving on the old `cmd_start_line`).
- **Resync after a parse error** consumes tokens to the next top-level
  `Newline` — the token-stream analogue of the old "clear `buf`, continue at the
  next line." A pathological error deep inside a multi-line compound may resync
  to a slightly different point than the old buffer-clear; this only affects the
  (already divergent-from-bash) cascade *after* a syntax error and is acceptable
  / low. Record as an `L-` note if a concrete divergence is observed.
- **`set -v` span vs physical-line echo:** identical bytes because the span is
  exactly the concatenation of the physical lines (each ending in `\n`).
- **`extglob` re-lex cost:** one extra full-remainder lex per flip. Flips are
  rare (0–2 per real file), so total stays ~O(n).
- **`huck -c CMD`:** a single string; same path, same O(n) behavior. Usually one
  line — no behavior change.
