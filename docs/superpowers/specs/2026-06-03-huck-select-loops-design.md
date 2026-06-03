# huck v81 — `select` loops (M-24) Design

**Status:** approved design, ready for implementation plan.
**Closes:** M-24 (`select` loops) — currently `[deferred]` medium in `docs/bash-divergences.md`.
**Branch (impl):** `v81-select-loops` (created from `main` at plan time).

## Goal

Implement bash's `select` menu loop:

```sh
select NAME [in WORDS...]; do BODY; done
```

`select` prints a numbered menu of the expanded WORDS to **stderr**, prints
the `PS3` prompt, reads one line from stdin, sets `NAME` to the chosen word
and `REPLY` to the raw input line, runs BODY, and repeats until `break` or
end-of-input. It is a loop construct: `break` / `continue` (and v79's
`break N` / `continue N`) operate on it.

**Scope decision (approved):** the numbered menu is rendered with **full
byte-for-byte fidelity** to bash 5.2's multi-column `print_select_list`
algorithm (column-major, `$COLUMNS`-aware, tab/space packed), not a
simplified single-column list. This makes the bash-diff harness byte-identical
for any item set, at the cost of porting bash's exact column algorithm.

## Background — verified bash 5.2 behavior

All behavior below was confirmed against bash 5.2.21 and cross-checked against
the bash source (`execute_cmd.c`, functions `displen`, `print_index_and_element`,
`indent`, `print_select_list`, `select_query`, `execute_select_command`).

- Menu + `PS3` prompt go to **stderr**; the line is read from **stdin**.
- `REPLY` = the raw input line (set by the `read` builtin); `NAME` = the
  selected word.
- Empty line (just Enter) → reprint the menu and re-prompt; body is NOT run.
- A number outside `1..count`, or a non-numeric line → `NAME=""` but the body
  **still runs** (`REPLY` holds the raw line).
- EOF (Ctrl-D / end of piped input) → the loop terminates.
- No `in WORDS` → iterate the positional parameters `"$@"` (same as `for`).
- Empty word list → `select` exits immediately, status 0, body never runs.
- Menu is printed on the first iteration and again only after an empty line;
  on other iterations only the `PS3` prompt is reprinted (bash builds with
  `KSH_COMPATIBLE_SELECT`, confirmed empirically — input `1\n2\n` prints the
  menu once).
- `PS3` default is `"#? "`.

### The exact menu algorithm (bash `print_select_list`)

Constants: `RP_SPACE = ") "`, `RP_SPACE_LEN = 2`, `tabsize = 8`,
`NUMBER_LEN(n)` = decimal digit count of `n`, `COLS` = `default_columns()`
(= `$COLUMNS` if set and > 0, else 80).

Per `select_query`, computed once:
- `max_item` = max over items of `displen(item)` (display width;
  multibyte-aware `wcswidth`, falling back to byte length — for ASCII this is
  `strlen`).
- `indices_len = NUMBER_LEN(count)`.
- `max_elem_len = max_item + indices_len + RP_SPACE_LEN + 2`
  (i.e. `max_item + indices_len + 4`).

Column/row computation (`print_select_list`):
```
cols = max_elem_len ? COLS / max_elem_len : 1;   // integer division
if (cols == 0) cols = 1;
rows = ceil(count / cols);
cols = ceil(count / rows);
if (rows == 1) { rows = cols; cols = 1; }         // wide-terminal → single column
first_column_indices_len = NUMBER_LEN(rows);
other_indices_len = indices_len;                  // = NUMBER_LEN(count)
```

Emission (column-major; to stderr):
```
for row in 0..rows:
    ind = row; pos = 0
    loop:
        iw = (pos == 0) ? first_column_indices_len : other_indices_len
        print "%*d) %s"  with width=iw, number=ind+1, item     // print_index_and_element
        elem_len = displen(item) + iw + RP_SPACE_LEN
        ind += rows
        if ind >= count: break
        indent(pos + elem_len, pos + max_elem_len)
        pos += max_elem_len
    putc('\n')
```

`indent(from, to)` (reproduces bash's exact tab/space bytes):
```
while from < to:
    if (to / tabsize) > (from / tabsize):
        emit '\t'; from += tabsize - (from % tabsize)
    else:
        emit ' '; from += 1
```

Consequences that the implementation must reproduce exactly:
- Column 0 right-justifies the number to `NUMBER_LEN(rows)`; other columns to
  `NUMBER_LEN(count)`. So a 10-item, 2-row layout shows `1) one` in column 0
  but ` 9) nine` / `10) ten` aligned in the last column; a single-column layout
  (cols==1, everything is "column 0" with width `NUMBER_LEN(count)`) shows
  ` 1) one` … `10) ten`.
- The `rows == 1` flip is why a **wide** `$COLUMNS` collapses to a single
  column: e.g. for 10 short items, `COLUMNS` 80–~109 → 5 columns / 2 rows, but
  `COLUMNS ≥ ~110` → 1 column / 10 rows.

Worked validation (10 items `one..ten`, `max_item=5`, `indices_len=2`,
`max_elem_len = 5+2+4 = 11`):
- `COLUMNS=80`: cols=`80/11`=7 → rows=`ceil(10/7)`=2 → cols=`ceil(10/2)`=5 → 5×2 ✓
- `COLUMNS=40`: cols=`40/11`=3 → rows=4 → cols=3 → 3×4 ✓
- `COLUMNS=110`: cols=`110/11`=10 → rows=1 → flip → 1×10 ✓

### `select_query` read/loop (per outer iteration)
```
COLS = default_columns(); tabsize = 8; compute max_elem_len, indices_len.
loop:
    if print_menu: print_select_list(...)
    write PS3 to stderr; flush stderr
    r = read_builtin(no names)             // sets REPLY to the raw line
    if r failed (EOF): write '\n' to stdout; return None        // loop ends
    if REPLY is None: return None
    if REPLY == "": print_menu = true; continue                 // reprint, re-prompt
    if REPLY is not a legal integer: return ""                  // NAME=""
    if number < 1 or > count: return ""                         // NAME=""
    else: return the number-th item
```

### `execute_select_command` outer loop
```
check NAME is a valid identifier (error otherwise).
list = expand WORDS (or "$@" if no `in`); if empty → return 0 (body never runs).
loop_level++   // (huck: loop_depth++ via the run_select wrapper)
show_menu = true
loop:
    PS3 = $PS3 or "#? "
    selection = select_query(list, count, PS3, show_menu)
    if selection is None (EOF): retval = failure; break
    bind NAME = selection (honor readonly)
    retval = run BODY
    handle break / continue (huck: LoopBreak/LoopContinue bubble)
    show_menu = false; if REPLY == "" then show_menu = true
exit status = retval (last body status, or failure on EOF)
```

## huck design

`select` maps onto the existing `for` machinery plus a new menu-rendering
helper. The lex→parse→execute pipeline and v79 loop infrastructure are reused.

### AST + lexer + parser (`src/command.rs`, `src/lexer.rs`)
- `command.rs`: add `Keyword::Select` to the `Keyword` enum + its `as_str`
  arm (`"select"`) + the `keyword_of` map entry (`"select" => Some(Keyword::Select)`).
- New struct:
  ```rust
  pub struct SelectClause {
      pub var: String,
      /// None       => no `in` clause: iterate the positional params "$@".
      /// Some(words) => explicit `in WORDS` (Some(vec![]) = empty `in`, which
      ///               yields an empty list and exits without running BODY).
      pub words: Option<Vec<Word>>,
      pub body: Sequence,
  }
  ```
  **Note — must use `Option`, NOT `ForClause`'s `Vec<Word>` convention.** bash
  distinguishes `select x in ; do …` (explicit empty `in` → empty list → exit)
  from `select x; do …` (no `in` → iterate `"$@"`). `ForClause` represents
  no-`in` as an empty `Vec<Word>`, which conflates the two and (see the
  pre-existing-bug note below) iterates nothing in both cases. `select` is new
  code and will do this correctly with `Option`.
- New `Command::Select(Box<SelectClause>)` variant.
- New `parse_select_command(iter)` mirroring `parse_for_after_keyword`:
  consume `select`, read NAME (validate identifier via the existing helper),
  optional `in WORDS` terminated by `;`/newline, then `do BODY done`. Wire the
  `Some(Keyword::Select) => parse_select_command(iter)` arm into `parse_command`
  AND `parse_next_stage` (so `select` works as a pipeline stage, consistent
  with the other compound commands).
- Errors: reuse the existing for-style unterminated/missing-do/missing-done
  errors; add a `Select`-named `ParseError` variant only if a distinct message
  is warranted (decide during implementation — prefer reuse).
- Lexer: no new tokens. `select` is an ordinary word classified as a keyword
  only at command position by `keyword_of`, exactly like `for`/`case`; so
  `select=1` (assignment) and `echo select` keep working with no special-casing
  (same mechanism that protected `function`/`for`).

### Executor (`src/executor.rs`)
- New `run_select(clause, shell, sink)` loop runner, structured like `run_for`:
  - Wrap with the v79 single-return-path `loop_depth` inc/dec (so `break N` /
    `continue N` see `select` as one loop level).
  - Expand the word list (or positionals if `words` is `None`) using the same
    expansion path `run_for` uses. Empty list → `Continue(0)`, body never runs.
  - Run the bash `select_query`/outer-loop logic above. Read input by invoking
    huck's existing `read` builtin with no names (it already stores the raw
    line in `REPLY` with the standard `read` line semantics) so behavior matches
    bash, which literally calls `read_builtin`.
  - Body result handling uses the same four-arm decrement-and-bubble match as
    the other loop runners: `LoopBreak(1, st)` consumes (loop ends, terminal
    `$?`=`st`); `LoopBreak(n, st)` bubbles `(n-1, st)`; `LoopContinue(1)` →
    next iteration; `LoopContinue(n)` → bubble `(n-1)`. `Exit` / `FunctionReturn`
    propagate.
  - Set `NAME` via the normal variable-set path (honor readonly like bash).
- Add the `Command::Select` arm wherever `Command::For` is matched in the
  executor dispatch.

### Menu rendering (`src/executor.rs`, or a small private module)
- A pure helper `format_select_menu(items: &[String], cols_width: usize) -> String`
  implementing `print_select_list` byte-for-byte (incl. `indent`'s tab/space
  rule). Keeping it a pure string-builder makes it unit-testable against exact
  expected bytes without a pty.
- `displen` for ASCII = byte length; multibyte width can use the same approach
  huck already uses elsewhere for display width if present, otherwise
  `chars().count()` is an acceptable first cut (document any multibyte
  divergence; ASCII is the harness's domain).
- `COLS` source: read the shell variable `COLUMNS` (parse as int, must be > 0),
  else default 80. (Matches bash's observable behavior when `COLUMNS` is set;
  huck need not query the tty winsize — the harness sets `COLUMNS`.)

### `PS3` / `REPLY`
- `PS3`: read the shell variable `PS3` for the prompt; default `"#? "` when
  unset. No new prompt-expansion machinery — it is emitted literally (bash does
  `fprintf(stderr, "%s", prompt)` with no further expansion of `PS3`).
- `REPLY`: already produced by huck's `read` builtin; reused as-is.

## Testing

1. **Unit tests** (`src/command.rs`, `src/executor.rs`):
   - Parser: `select x in a b c; do …; done`; no-`in` form; pipeline-stage
     position; missing `do`/`done`; invalid identifier.
   - `format_select_menu`: exact-byte assertions across permutations —
     multi-column at `COLUMNS=80` and `40`, the single-column flip at `110`,
     mixed item widths, 1-digit vs 2-digit counts, and a single item. These
     pin the column math, number justification, and tab/space output.
2. **Integration tests** (`tests/select_integration.rs`, piped stdin): valid
   selection sets `NAME`+`REPLY`; out-of-range/non-numeric → `NAME=""` + body
   runs; empty line reprints menu; EOF ends loop; no-`in` uses positionals;
   `break`, `continue`, `break 2` from a nested loop; empty list runs nothing;
   custom `PS3`.
3. **bash-diff harness** `tests/scripts/select_diff_check.sh` (huck's 8th):
   fragments piped through bash and huck, compared byte-for-byte. Menu fragments
   compare `cat -A` output (so tabs/newlines are explicit). Cover: simple
   selection; invalid + valid; empty-then-valid (menu reprint); `COLUMNS=80`
   multi-column; `COLUMNS=40`; `COLUMNS=110` single-column flip; mixed widths;
   12-item 2-digit; custom `PS3`; no-`in` positionals; EOF behavior. Set
   `COLUMNS` explicitly in each fragment for determinism.
4. **One pty interactive test** in `tests/pty_interactive.rs`: drive a real
   `select` menu, type a choice, confirm body output and prompt. Apply the v80
   lesson: `settle()` after any post-transition prompt before sending input,
   and never treat a redrawn prompt as a sufficient readiness barrier.

## Pre-existing bug discovered during design (`for` no-`in`)

While confirming how `for` represents the no-`in` form, I found that huck's
`for NAME; do …; done` (no `in` clause) iterates **nothing**, whereas bash
iterates the positional parameters `"$@"`:

```
$ printf 'set -- a b c\nfor x; do echo got=$x; done\necho end\n' | huck   # → end
$ printf 'set -- a b c\nfor x; do echo got=$x; done\necho end\n' | bash   # → got=a got=b got=c end
```

`run_for_inner` iterates `clause.words` directly, and `parse_for_after_keyword`
leaves `words` empty for the no-`in` form, so the loop body never runs. This is
a real, currently-undocumented divergence (call it **M-24a**).

**Proposed handling (small, shares logic with `select`):** fix `for`'s no-`in`
form to iterate `"$@"` as part of v81, since `select` needs the exact same
positional-fallback expansion. This is a focused change (the loop runners gain
"if no explicit `in`, use `shell.positional_args`") plus a regression test, and
it closes M-24a in the same iteration. **Alternative:** leave `for` as-is, make
only `select` correct, and log M-24a as a new `[deferred] low` entry. Decision
to confirm at spec-review; the plan will assume the fix-both approach unless
told otherwise (it is cheap and removes an inconsistency).

## Out of scope / documented divergences

- `select` is interactive by nature; non-interactive use (piped stdin) is fully
  supported and is how the harness/integration tests exercise it.
- Multibyte `displen`: ASCII is byte-exact; if huck lacks a `wcswidth`
  equivalent, wide-character column alignment may differ from bash for
  non-ASCII menu items — note as a low sub-divergence if so.
- `COLUMNS` is read from the shell variable (or 80); huck does not re-query the
  tty winsize on `SIGWINCH` mid-`select` (bash re-reads per `select_query` call;
  for non-interactive/harness use this is irrelevant, and huck reads the live
  `COLUMNS` variable each call too).

## File-change map

| File | Change |
|------|--------|
| `src/command.rs` | `Keyword::Select` (+`as_str`,+`keyword_of`); `SelectClause` struct; `Command::Select`; `parse_select_command`; wire into `parse_command` + `parse_next_stage`; parser unit tests |
| `src/executor.rs` | `run_select` (loop runner, v79 `loop_depth` wrapper + decrement-and-bubble); `Command::Select` dispatch arm; `format_select_menu` helper + menu unit tests |
| `tests/select_integration.rs` | NEW — binary-driven integration tests |
| `tests/scripts/select_diff_check.sh` | NEW — huck's 8th bash-diff harness |
| `tests/pty_interactive.rs` | one new pty `select` test |
| `docs/bash-divergences.md` | flip M-24 `[deferred]` → `[fixed v81]`; change-log entry; Summary tier-count + "Last updated" stamp |
| `README.md` | v81 iteration row |
