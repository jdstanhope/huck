# huck v141 — command substitution + arithmetic + backticks in prompt expansion Design

**Status:** approved design, ready for implementation plan.
**Implements:** `$(…)` command substitution, `$((…))` arithmetic, and `` `…` ``
backticks inside prompt expansion (`$PS1`/`$PS2`/`$PS4` and `${var@P}`), closing
most of the deferred **L-29** gap. Primary motivation: oh-my-posh sets
`PS1='$(_omp_get_primary)'`, which huck currently renders as the literal string.
**Branch (impl):** `v141-prompt-cmdsub-arith`.

## Background — current state

`src/prompt.rs` `expand_prompt(template: &str, shell: &Shell) -> String` is a
hand-written scanner that decodes backslash escapes (`\u \h \H \w \W \$ \n \r
\\ \? \j \! \# \e \a \[ \]` + `\033`) and expands `$VAR`/`${VAR}` (bare variable
lookup only). It is the single renderer behind THREE production callers:
- `src/shell.rs:~428` — the interactive REPL PS1/PS2 render.
- `src/param_expansion.rs:~258` — the `${var@P}` operator (v96).
- `src/executor.rs:~2918` — `ps4()` (the `$PS4` xtrace prefix, v131).

It does NOT handle `$(…)`, `$((…))`, or backticks — `$(` falls to the
"`$` followed by non-identifier → pass through" branch, so the literal text is
emitted. Verified: `v='$(echo CMDSUB)'; echo "${v@P}"` → huck `$(echo CMDSUB)`,
bash `CMDSUB`. `v='$((40+2))'` → huck `$((40+2))`, bash `42`.

**oh-my-posh works under huck EXCEPT this** (measured): sourcing its bash init
and calling `_omp_get_primary` directly produces the full ANSI/powerline prompt;
PROMPT_COMMAND-as-array works; `trap … RETURN` works; `${var@P}` works. The ONLY
blocker is `PS1='$(_omp_get_primary)'` rendering literally.

Reusable machinery:
- `expand::run_substitution(seq: &Sequence, shell: &mut Shell) -> String` — runs a
  command substitution (clones the shell, captures output). Already records `$?`
  via `set_last_status` + `set_last_cmd_sub_status`.
- `command::parse(lexer::tokenize(s)?) -> Result<Option<Sequence>>` — string → AST.
- `arith::parse(s) -> Result<ArithExpr>` + `arith::eval(&expr, shell: &mut Shell)
  -> Result<i64>`.

## Architecture

### Signature change
`expand_prompt(&str, &Shell)` → **`expand_prompt(&str, &mut Shell)`** (command
substitution needs `&mut Shell`). Thread `&mut Shell` through the 3 production
callers and update the ~12 `&shell` unit-test call-sites in `prompt.rs`.

### New expansion forms in the scanner
In the `$`-handling branch of `expand_prompt`, BEFORE the existing
`${NAME}`/`$NAME` cases, add (order matters — check `$((` before `$(`):
- **`$((` … `))`** → arithmetic. Scan from after `$((` to the matching `))`
  using a paren-depth counter (start depth 1 for the inner `(`; the body ends
  when an unmatched `))` is reached). `arith::parse(body)` + `arith::eval(&body,
  shell)`; on Ok push the i64 as a decimal string; on Err push nothing.
- **`$(` … `)`** → command substitution. Scan from after `$(` to the matching
  `)` with a quote-aware paren-depth counter (track `'`…`'` and `"`…`"` spans so a
  `)` inside a quote doesn't close early; nested `$(` increments depth). Parse the
  captured body via `command::parse(lexer::tokenize(body)?)`; on `Ok(Some(seq))`
  call `run_substitution(&seq, shell)`, strip trailing newlines (bash strips
  trailing `\n` from `$(…)`), push the result. On lex/parse error push nothing.
- Add a top-level **`` ` ``** branch (alongside `\\` and `$`) in the fast-path
  loop's terminator set: scan to the next unescaped backtick, run the inner text
  as a command substitution (same parse + `run_substitution` + trailing-newline
  strip).

Everything else is unchanged: `\X` escapes, `$VAR`/`${VAR}`, and — crucially —
all other bytes (ANSI escapes, the `\[`/`\]`→`\x01`/`\x02` markers, powerline
glyphs, and metacharacters as literal data) pass through untouched. Only the
explicit `$(`/`$((`/backtick forms are interpreted, matching bash's promptvars
pass and avoiding any re-lex of arbitrary prompt content.

### `$?` preservation (the subtle correctness point)
`run_substitution` sets `last_status`/`last_cmd_sub_status`. Prompt rendering must
be transparent to the user-visible `$?` (bash saves/restores `$?` around prompt
expansion). Therefore the two callers that render a prompt as a SIDE EFFECT —
the **REPL PS1/PS2 render** (`shell.rs`) and **`ps4()`** (xtrace) — snapshot
`last_status` (and `last_cmd_sub_status`) before `expand_prompt` and restore them
after. The **`@P` caller does NOT** snapshot — `${var@P}` is a normal in-command
expansion, so a `$(…)` inside it setting `$?` is bash-correct (and is masked by the
enclosing command's own status anyway).

Implementation: add small accessors if needed (`Shell` already has
`last_status()`/`set_last_status` and `last_cmd_sub_status`/`set_last_cmd_sub_status`).
The render/ps4 sites do `let saved = (shell.last_status(), shell.take/peek
last_cmd_sub_status); let s = expand_prompt(...); shell.set_last_status(saved.0);
shell.set_last_cmd_sub_status(saved.1);`. (Confirm the exact getter for
`last_cmd_sub_status`; if there's no peek, snapshot via the field accessor used by
v126.)

### Error / edge behaviour
- Unterminated `$(`, `$((`, or backtick (no matching close before end-of-string)
  → emit the literal remaining text (no panic), matching bash's lenient prompt
  handling.
- Inner lex/parse error or arith error → push nothing (empty); the inner
  command's own stderr surfaces via `run_substitution` as usual. No crash.
- Quote-aware paren scan handles `'`/`"` spans; a pathological `)` inside an
  unusual nested-quote construct is an acceptable documented edge (prompts rarely
  contain it).

## Out of scope (L-29 remainder)
- **`$LINENO`** in prompts — needs line-number tracking, unrelated to oh-my-posh.
  After v141, L-29 narrows to just `$LINENO` (+ the PS4-self-assign-timing note).
- **`shopt -u promptvars`** gating — huck always expands the prompt forms
  (promptvars default-on); gating expansion on a `promptvars` shopt is a separate
  refinement (huck doesn't track promptvars today; see the L-17 note). Documented.

## Files & responsibilities

| File | Change |
|------|--------|
| `src/prompt.rs` | `expand_prompt(&str, &mut Shell)`; add `$((…))`/`$(…)`/backtick handling (paren-depth scanners + reuse `command::parse`/`lexer::tokenize`/`run_substitution`/`arith`); update the ~12 unit-test call-sites; add new unit tests. |
| `src/shell.rs` | REPL render: `borrow_mut`, snapshot/restore `$?` around `expand_prompt`. |
| `src/executor.rs` | `ps4()` → `&mut Shell`, snapshot/restore `$?`; its caller passes `&mut`. |
| `src/param_expansion.rs` | `@P` call-site → `&mut shell` (no `$?` snapshot). |
| `tests/scripts/prompt_expansion_diff_check.sh` (NEW, 61st) | Bash-diff over `${var@P}` with cmdsub/arith/backtick/nested. |
| `tests/prompt_omp_pty.rs` (NEW, best-effort) | PTY: source oh-my-posh init (if present) → prompt renders non-literal; skips gracefully. |
| `docs/bash-divergences.md` | Narrow L-29 to `$LINENO` only (note cmdsub/arith/backtick now work in prompts as of v141). |

## Testing

1. **Unit tests** (`src/prompt.rs`): build a `Shell`, set a var, assert:
   - `expand_prompt("$(echo hi)", &mut s)` → `"hi"`.
   - `expand_prompt("$((40+2))", &mut s)` → `"42"`.
   - nested: `expand_prompt("$(echo $(echo x))", &mut s)` → `"x"`.
   - backtick: ``expand_prompt("`echo y`", &mut s)`` → `"y"`.
   - trailing-newline strip: `$(printf 'a\n\n')` → `"a"`.
   - `$VAR` still works; mixed `"\\u $(echo z)"`.
   - unterminated `"$(echo"` → literal `"$(echo"`.
   - ANSI/marker pass-through: `"\x01\x1b[31m\x02$(echo r)"` → markers intact + `r`.
   - **`$?` preserved**: set `last_status` to 7, call the RENDER path (or a tiny
     helper mirroring it) with `PS1='$(false)'`, assert `$?` is still 7 afterward;
     and that the bare `@P` path does NOT preserve (documents the split).
2. **Bash-diff harness** `tests/scripts/prompt_expansion_diff_check.sh` (61st) — via
   `-c`, byte-identical bash↔huck:
   - `v='$(echo CMDSUB)'; echo "${v@P}"` → `CMDSUB`
   - `v='$((6*7))'; echo "${v@P}"` → `42`
   - `v='pre-$(echo mid)-post'; echo "${v@P}"`
   - `v='$(echo $(echo nested))'; echo "${v@P}"`
   - ``v='`echo bt`'; echo "${v@P}"``
   - `v='\u@\h $(echo X)'; echo "${v@P}"` (escapes + cmdsub together; note `\u`/`\h`
     render host-specific — pick fragments whose escape output is identical under
     bash and huck, OR assert only the cmdsub portion; prefer cmdsub/arith-only
     fragments for byte-identity and put the escape-mix in a unit test).
   - `PS1`-style: `PS1='$(echo P)>'; ` then read the rendered prompt is impractical
     non-interactively — keep harness on `@P`; the PS1 path is covered by the PTY
     test + the `$?`-preservation unit test.
3. **PTY payoff test** `tests/prompt_omp_pty.rs` (best-effort, mirror an existing
   `*_pty.rs`): if `oh-my-posh` is on PATH, spawn interactive huck, `source` its
   init (`oh-my-posh init bash` → the cached init file), send a newline, and assert
   the emitted prompt contains an ANSI escape (`\x1b[`) — i.e. it RENDERED rather
   than printing the literal `$(_omp_get_primary)`. Skip gracefully if no PTY or no
   oh-my-posh. Do not hard-fail the suite on environment absence.
4. **Full regression:** entire suite + all harnesses green — ESPECIALLY the
   existing `prompt.rs` tests, the `@P` tests, and the v130/v131 xtrace (`set -x`)
   tests (ps4 now takes `&mut` + snapshots `$?` — its output must be unchanged).
   clippy clean.

## Edge cases & notes
- **Trailing-newline strip** applies to cmdsub/backtick results only (not arith).
- **Recursion**: `expand_prompt` calling `run_substitution` which runs commands
  that might themselves render a prompt is not a concern (commands don't render
  prompts); arith/cmdsub inside cmdsub is handled by the normal pipeline, not by
  re-entering `expand_prompt`.
- **ps4 `&Shell`→`&mut Shell`**: `ps4` is called from the xtrace path
  (`run_exec_single`); thread `&mut shell` there (it already has `&mut Shell`).
- **No promptvars regression**: the change only ADDS expansion of forms that were
  previously literal; existing prompts without `$(`/`$((`/backtick are byte-identical.
- **Git safety:** implementer subagents must NOT `git checkout <sha>`; the
  controller verifies the branch tip before merging. Commit trailer:
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
