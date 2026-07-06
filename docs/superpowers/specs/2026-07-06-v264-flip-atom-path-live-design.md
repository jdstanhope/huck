# v264 — THE FLIP: make the atom-command path the production parser (milestone 1 of the finale)

**Status:** design approved (2026-07-06)
**Arc:** THE FINALE, milestone 1. The reconciliation arc (v259–v263) is COMPLETE —
the dormant atom-command parser (`parse_sequence` over a `new_live_atoms` lexer) is
byte-identical to the `command.rs` oracle (`command::parse`) on every recorded
differential input, except a handful of documented exotic pins. This iteration
REPOINTS production command execution from the oracle to the atom path. It does
NOT delete anything — the oracle, the 6 forward-scanning scanners, and the
differential harness all stay resident (milestone 2 deletes them, and is gated on
first porting substitution/heredoc/arith bodies to atom-native scanning).

## Summary

Two production entry points do all command EXECUTION; v264 repoints exactly those
two to the atom path:

- **`shell.rs process_line_in_sinks`** (interactive lines, `eval`, trap actions,
  `PROMPT_COMMAND` — all funnel here): `Lexer::new_live` + `command::parse` →
  `Lexer::new_live_atoms` + `parser::parse_sequence`.
- **`builtins.rs run_sourced_contents_in_sinks`** (`source`/`.`, `-c STRING`,
  script-file execution): `Lexer::new_live` + a `command::parse_one_unit` loop →
  `new_live_atoms` + a new `parser::parse_one_unit` loop.

**Left on the oracle, unchanged** (auxiliary/non-execution; the oracle stays
resident as the atom path's own substitution-body engine + the differential
harness): `continuation.rs` (PS2 completeness classifier — depends on specific
`ParseError` variants), `prompt.rs` (PS1 `$(…)`), `shell_state.rs` (`BASH_FUNC`
env import), and the lexer-internal `parse_substitution_body` (which *is* the
atom path's delegated `$(…)`/`` `…` `` body engine).

**Why this is safe:** the atom path already reuses the oracle for substitution /
heredoc-with-expansion / arith bodies (v244/v250 architecture — `Mode::CommandSub`
/`Mode::Backtick` delegate body scanning to `scan_step_command`; heredoc bodies go
through the shared `collect_heredoc_bodies`). So repointing the top-level entry
points changes only WHO assembles the top-level command/word structure — and the
differential harness proves that structure is byte-identical to the oracle. Kept
alive, the harness gives atom==oracle proof at the AST level while the huck-engine
behavioral suite + bash-diff harnesses give atom==bash proof at the behavioral
level — simultaneously.

## Background — the surface (mapped)

**Production parse sites** (crate huck-engine, all confirmed non-test):
1. `shell.rs process_line_in_sinks` (~378-385) — `Lexer::new_live` + `command::parse`. **REPOINT.**
2. `builtins.rs run_sourced_contents_in_sinks` (~6330 + ~6380) — `new_live` + `command::parse_one_unit` loop. **REPOINT.**
3. `continuation.rs classify` (~50/69/79) — `tokenize_with_opts` + two `command::parse` (variant-sensitive). **KEEP on oracle.**
4. `prompt.rs run_prompt_cmdsub` (~248-256) — `tokenize` + `command::parse`. **KEEP.**
5. `shell_state.rs parse_imported_function` (~683-684) — `tokenize` + `command::parse`. **KEEP.**
6. `lexer.rs parse_substitution_body` (~6249-6257) — `tokenize_with_opts` + `command::parse`; the atom path's OWN delegated substitution-body engine. **KEEP.**

**The atom path** (crate huck-syntax): `parse_sequence(iter)` (parser.rs ~2953,
`pub(crate)`) mirrors the oracle `parse`/`parse_cursor`: skip leading
`Newline`/`Blank` → `Ok(None)` on EOF → `parse_and_or(iter, &[])` → stray-terminator
check → heredoc-body fill. It parses the WHOLE input to EOF. Driven by
`Lexer::new_live_atoms` (lexer.rs ~4530 = `new_live` [threads aliases] +
`command_atoms = true`).

**No atom `parse_one_unit` exists.** The oracle's `parse_one_unit` (command.rs
~804) = `parse_sequence_opts(iter, &[], stop_at_newline=true)`: skip leading
`Newline`, `None` on EOF, parse ONE unit up to and consuming the next top-level
newline. The source loop (site 2) needs this for incremental per-unit execution
(execute-unit / refresh-aliases / repeat; a late syntax error still runs earlier
units — bash semantics).

**The differential harness** (parser.rs `mod tests`): `old_seq`/`new_seq` +
`diff_cmd` (766×) / `diff_err` (50×) + the `assert_ne!` pins. Every `diff_*`
compares against `old_seq` → `command::parse`. It stays exactly as-is (the oracle
is retained), and remains the AST-level proof that atom == oracle.

**Visibility blockers:** `parse_sequence` is `pub(crate)`; huck-engine's
`lib.rs:53` re-exports `brace_expand, command, generate, lexer` but NOT `parser`.

## Architecture

### New: atom `parse_one_unit` (T1)

`pub fn parse_one_unit(iter: &mut Lexer) -> Result<Option<Sequence>, ParseError>`
in parser.rs, mirroring the oracle:
- skip leading `Newline` (and the atom-only `Blank`) atoms;
- `Ok(None)` when only newlines/blanks/EOF remain;
- parse ONE unit via the seq-builder in **stop-at-top-level-newline** mode,
  consuming the terminating newline; return `Ok(Some(seq))`.

Mechanism: thread a `stop_at_newline: bool` (or an internal `parse_and_or_opts`)
into the atom sequence builder `parse_and_or` — the direct analog of the oracle's
`parse_sequence_opts(iter, stop_at, stop_at_newline)`. In stop mode, a top-level
`Newline` ends the unit (consumed) instead of acting as an intra-sequence
separator. `parse_sequence` keeps its current whole-input behavior
(`stop_at_newline=false`), so the interactive path is unchanged. Heredoc-body
fill, the stray-terminator check, and blank-skipping are shared.

### Wiring (T2)

- `parse_sequence` and `parse_one_unit` → `pub`.
- huck-engine `lib.rs:53`: add `parser` to the `pub use huck_syntax::{…}` list.
- `shell.rs process_line_in_sinks`: `Lexer::new_live(line, aliases, opts)` →
  `Lexer::new_live_atoms(line, aliases, opts)`; `command::parse(&mut lx)` →
  `parser::parse_sequence(&mut lx)`. The `Ok(Some(seq))` → `execute_with_sink`,
  `Ok(None)` → `Continue(0)`, `Err(e)` → syntax-error arms are unchanged.
- `builtins.rs run_sourced_contents_in_sinks`:
  `Lexer::new_live(&contents[start..], &aliases_now, opts)` → `new_live_atoms(…)`;
  `crate::command::parse_one_unit(&mut iter)` → `parser::parse_one_unit(&mut iter)`.
  Surrounding logic — `set_base_line`, the newline-skip peek loop, `cursor_pos()`
  / `peek_span()` offset tracking, `set_aliases` between units, the lex-error
  branch — is method-level on the same `Lexer` and carries over unchanged.

### Types / errors (unchanged)

Both paths return `Result<Option<Sequence>, ParseError>` with the identical
`ParseError` enum (defined in command.rs, retained). `parse_error_message(&e)`,
`$LINENO` line reporting, and exit codes are untouched.

### Pin re-evaluation vs bash (T3)

The documented atom≠oracle pins were kept because "no production impact (dormant)."
After the flip the atom side IS production, so each is re-judged against BASH:
- `atoms_heredoc_redirect_target_before_arg_pin` (v260 ×2) — multi-heredoc-in-one-line body order.
- backtick backslash-run decode (parser.rs ~6172, `new_bt`/`old_bt`, 4 inputs) — comment claims "no production impact."
- `atoms_legacy_arith_backslash_quote_carryforward` (v261 ×1) — `\'`/`\"` in `$[ ]`; atom `Err` vs oracle `Ok`; `$[ ]` deprecated.
- re-confirm CF5/CF8/CF9/CF10 dispositions as production behavior (several are atom-closer-to-bash = improvements).

Per pin: **accept** (matches bash / exotic-both-reject / closer-to-bash → keep the
test, reword the stale "dormant" comment) or **record** in `docs/bash-divergences.md`.
No code change expected — but the flip makes verifying-against-bash mandatory.

## Verification (the safety net) — all must be green

1. **huck-syntax lib suite** (`cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1`):
   the differential harness (`diff_cmd`/`diff_err`/pins) + the new `old_unit`/`new_unit`.
   AST-level proof atom == oracle.
2. **huck-engine lib suite** (`cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`):
   the behavioral/executor/builtin tests now run THROUGH the repointed atom path.
   The end-to-end proof production still behaves correctly. (Single-threaded ONLY —
   `--workspace`/multi-threaded OOM-kills the 1-core/1.9 GB box; the known
   `posix_*` parallel-starvation failures are avoided by `-p huck-engine`
   single-threaded.)
3. **bash-diff harnesses**: build the huck binary, run every
   `tests/scripts/*_diff_check.sh` (bash vs huck byte-identical) — the gold-standard
   bash-compat gate, now exercising the atom path in the real binary.

## Task decomposition (SDD)

- **T1 — atom `parse_one_unit` + newline-stop mode + unit differential** (huck-syntax
  only; no production repoint). Thread `stop_at_newline` into `parse_and_or`; add
  `pub fn parse_one_unit`; add `old_unit`/`new_unit` + corpus (multi-unit scripts;
  blank-line / EOF / trailing-newline edges; compound spanning a newline; heredoc
  inside a unit; `;`/`&&`/`&` intra-unit). Gate: huck-syntax green; `command.rs`
  empty-diff.
- **T2 — wiring + repoint the two execution paths** (THE flip). `pub` + `parser`
  re-export; repoint `shell.rs` + `builtins.rs` source loop. Gate: **huck-engine
  lib suite single-threaded green** + huck-syntax green.
- **T3 — pin re-evaluation vs bash + doc updates.** Probe each pin against bash;
  accept-or-record; reword the now-false comments; `docs/bash-divergences.md` for
  any genuine divergence; re-confirm CF5/CF8/CF9/CF10.
- **Branch-level gate (not a task):** build the binary, run all
  `tests/scripts/*_diff_check.sh`; opus whole-branch review (probe source-loop
  edges, alias-refresh-between-units, `$LINENO` through the atom path,
  trap/eval/PROMPT_COMMAND routing, `-c`/script-file execution).

Order: T1 → T2 (the flip needs `parse_one_unit`) → T3 (pins judged as production).

## Constraints

- **`command.rs` EMPTY-diff** (the oracle is retained, untouched). The 6 scanners
  are retained (still the substitution-body engine). NO deletion in v264.
- Box is 1 core / 1.9 GB — test per-crate single-threaded ONLY; NEVER `--workspace`
  / multi-threaded.
- Commit trailer VERBATIM: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- rust-analyzer PHANTOM diagnostics — trust cargo.

## Out of scope (milestone 2 — a later arc)

Deleting the oracle + the 6 forward-scanning scanners + migrating the 766-test
differential harness. This is GATED on first porting substitution / heredoc /
arith bodies to atom-native scanning (today they reuse `scan_step_command` /
`scan_dollar_expansion` / … / `parse_substitution_body` → `command::parse`, so
those are live under the atom path, not dead). Also deferred: repointing the three
auxiliary sites (continuation/prompt/shell_state) onto the atom path.
