# huck v152 — `LINENO` (current line number) Design

**Status:** approved design, ready for implementation plan.
**Adds:** `$LINENO` — the source line number of the currently-executing command. huck has
no line tracking today, so `$LINENO` expands empty (the open L-29). This is the foundation
for v153 (`BASH_SOURCE` / `BASH_LINENO`).
**Branch (impl):** `v152-lineno`.
**Scope (MVP, agreed):** per-simple-command line accuracy in the common contexts — multi-line
`-c`, scripts, function bodies, and sourced files. `eval`'s arcane line offset and
compound-command-*header* `$LINENO` are documented edge divergences, not matched.

## Semantics (verified against bash 5.x)

`$LINENO` is the source line of the command being executed, **fixed at parse time relative
to the text that was parsed** — each parse unit (script, `-c` string, function body, sourced
file, `eval` string) has its own 1-based line space.

| Context | `$LINENO` |
|---|---|
| multi-line `-c` | line within the `-c` string (`echo $LINENO` on lines 1,2,3 → 1,2,3) |
| function (defined in `-c` or a script) | the command's **absolute line in the source where it was defined** — NOT function-relative (a function whose `echo` is on script line 3 reports 3) |
| sourced file | line within the sourced file (own 1-based space), restored after `source` returns |
| `eval "…"` | bash uses an arcane `+offset`; huck reports the line within the eval string (documented divergence) |

Because lines are baked into the AST at parse time, **no runtime line-context stack is
needed** — functions and sourced files self-correct (their commands carry their own parsed
lines; the executor just sets a single "current line" before each command).

## Architecture

### 1. AST — `ExecCommand.line`

Add `pub line: u32` to `ExecCommand` (`src/command.rs:319`). It holds the 1-based source line
of the command's first token (0 = unknown, for internally-constructed commands / callers that
don't supply positions). Every `ExecCommand { … }` literal in the codebase (builder at
`src/command.rs:229`/`241`, the test helper at `~2366`, the v150 procsub `Command::Redirected`
wrap if it builds one, etc.) gains `line: …` (0 where no position is available).

### 2. Lexer — offset → line

`tokenize_with_offsets(input, opts) -> Result<(Vec<Token>, Vec<usize>), …>` already yields each
token's start byte offset. Add a helper that maps an offset to a 1-based line:
```rust
/// 1-based line number of byte offset `off` within `src` (1 + count of '\n' before it).
pub fn line_at_offset(src: &str, off: usize) -> u32 {
    1 + src.as_bytes()[..off.min(src.len())].iter().filter(|&&b| b == b'\n').count() as u32
}
```
Precompute a parallel `Vec<u32>` of token lines: `offsets.iter().map(|&o| line_at_offset(src, o))`.

### 3. Parser — thread the per-token line; stamp `ExecCommand.line`

The parser currently threads `&mut Peekable<I: Iterator<Item = Token>>` through ~20 functions
and builds `ExecCommand` in one helper. Replace that token stream with a **position-aware
cursor** carrying the lines, so the simple-command builder can stamp the line of the command's
first token:

- New `pub struct TokenCursor` owning `tokens: Vec<Token>`, `lines: Vec<u32>` (parallel), and
  a `pos: usize`. It provides the exact subset of `Peekable` operations the parser uses —
  `peek() -> Option<&Token>`, `next() -> Option<Token>`, and any `peek2`/`next_if` currently
  relied on (audit the parser for which `Peekable` methods are used and replicate them), plus
  `current_line() -> u32` (the line of the token at `pos`, i.e. the next token to be returned;
  `0` past the end).
- Change the ~20 parser fn signatures from `iter: &mut Peekable<I>` (generic) to
  `cur: &mut TokenCursor` (concrete). Function BODIES are unchanged — `peek()`/`next()` keep
  the same `Option<&Token>`/`Option<Token>` return types, so the match arms don't change. This
  removes the `<I: Iterator<Item = Token>>` generics.
- In `parse_simple_stage` (`src/command.rs:1707`), capture `let line = cur.current_line();` at
  the START (before consuming the command's first token) and thread it into the
  `ExecCommand`-building helper so both `ExecCommand` literals (229/241) set `line`.
- Entry points:
  - `pub fn parse(tokens: Vec<Token>) -> Result<Option<Sequence>, ParseError>` becomes a thin
    shim that builds a `TokenCursor` with `lines = vec![0; tokens.len()]` (no positions →
    line 0) and delegates — keeps every existing caller / test compiling unchanged.
  - New `pub fn parse_with_lines(tokens: Vec<Token>, lines: Vec<u32>) -> Result<Option<Sequence>, ParseError>`
    (and a `parse_one_unit`-equivalent that takes lines) for the runtime call sites that have
    offsets.

### 4. Call sites — feed real lines into the runtime parse paths

Switch the RUNTIME tokenize→parse sites to compute lines from `tokenize_with_offsets` and call
the line-aware parse. (Test/internal parses can keep the line-0 `parse` shim.)
- `process_line_in_sink` (`src/shell.rs:626`) — the `-c` string and interactive line.
- `run_sourced_contents` / the linear `parse_one_unit` loop (`src/builtins.rs`) — sourced files
  and the main script (`huck script.sh`). The offsets are over the file/string text → file/
  string lines.
- `eval` (`builtin_eval`) — re-tokenize the eval string with offsets → lines within it.

Function-def bodies need NO special handling: a `FunctionDef`'s body is parsed as part of the
enclosing source pass, so its `ExecCommand`s already carry the enclosing source's lines
(matching bash's "function reports def-site line"). Command substitution `$(…)` bodies parsed
via the lexer's recursive path may default to line 0 for `$LINENO` inside `$()` — a documented
MVP edge (rare).

### 5. Executor + `$LINENO`

- New `Shell.current_lineno: u32` (init `0`), in `src/shell_state.rs`.
- At the top of `run_exec_single` (before word expansion), `shell.current_lineno = cmd.line;`
  — but only when `cmd.line != 0`, so internally-synthesized commands (line 0) don't clobber a
  meaningful current line. (Expansion is where `$LINENO` is read, so setting it here makes
  `$LINENO` reflect the executing command.)
- `lookup_var("LINENO")` special-case (alongside `$?`/`$$`/`$0`): `return Some(self.current_lineno.to_string())`. Like other dynamic params, an assignment to `LINENO`
  is overwritten by the next command (documented dynamic-var behavior).

No save/restore: the per-command stamp makes functions, sourced files, `-c`, and `eval` all
self-correct (a function's commands set their def-site lines; the caller's next command resets
`current_lineno` on return).

## Files & responsibilities

| File | Change |
|------|--------|
| `src/command.rs` | `ExecCommand.line: u32`; `TokenCursor` + parser signature switch (`Peekable<I>` → `&mut TokenCursor`); stamp `line` in `parse_simple_stage`; `parse` shim (line 0) + `parse_with_lines`. |
| `src/lexer.rs` | `line_at_offset(src, off) -> u32` helper. |
| `src/shell.rs` | `process_line_in_sink` → tokenize-with-offsets + `parse_with_lines`. |
| `src/builtins.rs` | sourced/script reader + `eval` → line-aware parse. |
| `src/shell_state.rs` | `Shell.current_lineno: u32`; `lookup_var("LINENO")`. |
| `src/executor.rs` | set `shell.current_lineno = cmd.line` (when non-zero) at the top of `run_exec_single`. |
| `tests/scripts/lineno_diff_check.sh` | bash-diff harness. |
| `docs/bash-divergences.md` | update L-29 (the `$LINENO`-variable part is resolved; keep any residual). |

## Behaviour matrix (target = bash)

| Input | Result |
|---|---|
| `huck -c $'echo $LINENO\necho $LINENO'` | `1` then `2` |
| `huck -c $'f(){\n echo $LINENO\n}\nf'` | `2` (def-site line in the `-c` text) |
| `huck script.sh` with `echo $LINENO` on line 3 | `3` |
| sourced file: `echo $LINENO` on its line 2 | `2`; back to the parent's line after `source` |
| `huck -c $'if true; then echo $LINENO; fi'` | line of the `echo` (condition + body are ExecCommands) |
| top level after a function returns | the caller's line (current_lineno reset per command) |

## Edge cases (MVP-documented)

- **`eval`:** `$LINENO` = line within the eval string (bash's `+offset` not matched).
- **Compound-command headers:** `$LINENO` in a `for … in <list>` / `case <word> in` header (expanded by the compound runner, not a leaf `ExecCommand`) may report the previous command's
  line. The `while`/`if` *condition* is an `ExecCommand`, so it is correct.
- **`$()` body:** `$LINENO` inside command substitution may be line 0 (rare).
- **Assignment to `LINENO`:** overwritten on the next command (dynamic-var behavior).
- **Line 0 (unknown):** a `$LINENO` evaluated before any positioned command runs expands `0`.

## Testing

1. **Unit tests:** `line_at_offset`; `parse_with_lines` stamps `ExecCommand.line` correctly
   across multi-line input (e.g. three statements on lines 1/2/3); `lookup_var("LINENO")`
   reflects `current_lineno`.
2. **`lineno_diff_check.sh`:** byte-identical bash↔huck for multi-line `-c`, a function body,
   `if`/`while` conditions, and `$LINENO` on consecutive lines. The script-file and sourced-file
   cases run via file-args (the L-27 piped-stdin history-expansion caveat).
3. **Full regression:** suite + all harnesses green; clippy clean. (The `parse`→`TokenCursor`
   refactor touches the whole parser — the existing parser test suite is the safety net; it must
   stay green with zero behavioral change for line-0 parses.)

## Notes
- The `Peekable<I>` → `TokenCursor` refactor is the dominant effort and risk: it is broad but
  mechanical (signatures change; bodies don't). The existing parser tests gate it.
- **Git safety:** implementer subagents must NOT `git checkout <sha>`; the controller verifies
  the branch tip before merge. Commit trailer:
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
