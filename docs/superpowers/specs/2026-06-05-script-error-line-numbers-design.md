# huck v94 — line numbers in sourced-script syntax errors Design

**Status:** approved design, ready for implementation plan.
**Implements:** physical line numbers in lex/parse (syntax) error diagnostics for
sourced files — `source FILE` / `. FILE`, `huck --rcfile FILE`, and script-file
mode `huck SCRIPT`. Today these report `huck: FILE: syntax error: MSG` with no
location; v94 adds the line: `huck: FILE: line N: syntax error: MSG` (bash's
`line N` convention).
**Why:** locating a syntax error in a large sourced file (e.g.
`/usr/share/bash-completion/bash_completion`) currently requires bisection.
Line numbers make the failing construct directly findable. This is a diagnostics
iteration (like v80 — no `M-*` divergence flip).
**Branch (impl):** `v94-script-error-line-numbers`.

## Scope (confirmed during brainstorming)

- **In:** lex errors AND parse errors raised inside `run_sourced_contents`
  (`src/builtins.rs`) — the shared engine for `source`/`.`/`--rcfile`/script-file
  mode.
- **Out (noted follow-ons):** runtime errors (`command not found`, bad
  substitution, etc. — they originate deep in the executor with no line context
  and don't carry the filename today); exact token-level `line:col` (needs
  position tracking on every `Token` — a large lexer refactor); the piped-stdin
  REPL path (`process_line`/`read_logical_command` in `src/shell.rs`).

## Approach

`run_sourced_contents` already drives parsing as a loop over physical lines,
accumulating into `buf` until `continuation::classify` reports the logical
command is complete, then lexing + parsing `buf`. Add a 1-based physical-line
counter and remember the line on which the current `buf` started; report that
line in the two error arms.

- Maintain `physical_line: usize` incremented once per `for line in
  contents.lines()` iteration (starts at 1).
- Maintain `cmd_start_line: usize`. Set `cmd_start_line = physical_line` at the
  point a new logical command begins — i.e. when `buf` is empty just before the
  current line is appended.
- In the lex-error arm and the parse-error arm, change the message to:
  `huck: {path}: line {cmd_start_line}: syntax error{...}` (lex) and
  `huck: {path}: line {cmd_start_line}: syntax error: {...}` (parse), preserving
  the existing `lex_error_message` / `parse_error_message` suffixes verbatim.

## Semantics & limitation

- A **single-physical-line** command → the reported line is exact (matches bash).
- A **multi-physical-line** logical command (function body, continued `if`,
  multi-line `[[`, line-continuation) → the reported line is the command's
  **first** physical line, whereas bash reports the line of the offending token.
  Documented limitation; still a large improvement and correct for the common
  case. (Exact-token line is the deferred `line:col` follow-on.)
- Blank lines and comment-only lines still advance `physical_line`, so the count
  stays aligned with the file's actual line numbers.
- The piped-stdin/REPL path is unchanged: those errors keep the existing
  `huck: syntax error: MSG` form (no file, no line) — and the byte-diff harnesses
  feed fragments via stdin, so they are unaffected by this change.

## Files & responsibilities

| File | Change |
|------|--------|
| `src/builtins.rs` | `run_sourced_contents`: add `physical_line` + `cmd_start_line` tracking; include `line {N}:` in the lex-error and parse-error `eprintln!` arms |
| `tests/script_line_numbers_integration.rs` | NEW — write a temp script with a syntax error on a known line, run it via `huck SCRIPT`, assert stderr contains `line N:` |
| `docs/bash-divergences.md`, `README.md` | change-log entry; README v94 row; (optionally) note the multi-line-points-at-first-line limitation in the Low-impact tier |

## Testing

1. **Integration** (`tests/script_line_numbers_integration.rs`, via `tempfile`):
   - a script whose 3rd line has a parse error (e.g. a stray `fi`) → stderr
     contains `: line 3: syntax error`.
   - a script whose Nth line has a lex error → stderr contains `: line N: syntax
     error`.
   - a valid first command then an error later → the line number reflects the
     erroring command's start line, not line 1.
   - a multi-line construct (e.g. a 3-line function) with a syntax error → line
     number is the construct's first line (documents the limitation).
2. **Regression**: existing source/`.` error-message tests still pass (update any
   that asserted the exact `FILE: syntax error:` string to expect the new `FILE:
   line N: syntax error:` form).

## Edge cases & notes

- If a logical command spans lines and lexing of the completed `buf` fails, the
  reported line is `cmd_start_line` (the first line) — acceptable per the
  limitation above.
- `cmd_start_line` is only (re)assigned when `buf` is empty before appending, so
  continuation lines never move it.
- No change to exit status (`last_status = 2` on syntax error) or control flow —
  only the message string gains `line N:`.
