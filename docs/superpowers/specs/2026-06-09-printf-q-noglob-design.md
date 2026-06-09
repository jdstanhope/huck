# huck v120 — `printf %q` + `set -f`/`noglob` (M-73 / M-08 sub-features) Design

**Status:** approved design, ready for implementation plan.
**Implements:** two deferred sub-features, both on mise's `_mise` completion
handler (one layer past `_init_completion`, which v119 made byte-identical):
1. **`printf %q`** — the shell-quote conversion (deferred under **M-73**).
2. **`set -f` / `set -o noglob`** — disable pathname expansion (deferred under
   **M-08**).
**Why now:** live `mise<TAB>` reaches mise's `_mise` handler, which fails with
`printf: \`%q': invalid directive` and `set: noglob: not yet supported`. Both are
needed for `mise<TAB>` to emit completion candidates.
**Bundle rationale:** two independent, small, well-bounded features the user
chose to ship together to unblock mise in one pass.
**Branch (impl):** `v120-printf-q-noglob`.

---

## Feature 1 — `printf %q` (shell-quote)

### Background — bash behavior (probed this session)
`printf '%q' ARG` emits ARG quoted so it re-reads as the same word:

| input | bash `%q` |
|---|---|
| `plain` / `p/q` / `a-b` / `ünï` | unchanged (letters, digits, `%+-./:=@_`, UTF-8) |
| `a b` | `a\ b` |
| `c'd` | `c\'d` |
| `a$b` / `x"y` / `*` / `!h` / `a;b` | `a\$b` / `x\"y` / `\*` / `\!h` / `a\;b` |
| `` (empty) | `''` |
| `tab\tx` (has control char) | `$'tab\tx'` |
| `a\nb` | `$'a\nb'` |
| `%6q` of `a b` (width) | `[  a\ b]` (width pads the QUOTED result) |
| `%q` of `one two three` (cycling) | `one\ntwo\nthree` (consumes + cycles) |

**Algorithm:**
1. empty → `''`.
2. contains any control char (`is_control()`: `\0`..`\x1F`, `\x7F`) → the
   `$'…'` ANSI-C form (the WHOLE string; same encoder `@Q` uses).
3. else → backslash-escape each char in the set **SPACE + `!"#$&'()*,;<>?[\]^`{|}~`**;
   all other chars (letters, digits, `%+-./:=@_`, UTF-8) emitted as-is.

Note: `%q` uses **backslash** style for the simple case (`a\ b`), whereas
`${v@Q}` uses **single-quote** style (`'a b'`). Both re-read identically; the
formats deliberately differ, matching bash. Only the control-char `$'…'` branch
is shared.

### Architecture
huck's `shell_quote` (`src/param_expansion.rs:270`, the `@Q` impl) already has
the `$'…'` ANSI-C encoder (its `is_control()` branch, lines 271-292) and a
separate single-quote branch. For `%q`:
- **Refactor (no behavior change):** extract the `$'…'` encoder block into
  `pub(crate) fn ansi_c_quote(v: &str) -> String`; `shell_quote`'s control
  branch calls it.
- **New `printf_q(arg: &str) -> String`** (`src/builtins.rs`, the printf
  module): implements the 3-case algorithm above; case 2 calls
  `crate::param_expansion::ansi_c_quote`.
- **Plumbing:** add `ConvChar::Q`; `b'q' => ConvChar::Q` in the conversion
  match (`builtins.rs:2424`); a `ConvChar::Q` arm in `format_one`
  (`:2518`) that produces `printf_q(arg)` then applies width/precision via the
  existing `pad_string` (so `%6q`/`%.2q` behave like `%s` on the quoted string —
  width pads, precision truncates the quoted string; verify the precision edge
  against bash). `Q` must count as a **consuming** conversion wherever
  `format_one` / the cycling logic checks "does the format consume an arg" (so
  `%q one two` cycles).

### Correctness / edges
- The escape set is exactly the probed printable-ASCII set; `\` itself is in it
  (→ `\\`); `%`,`+`,`-`,`.`,`/`,`:`,`=`,`@`,`_` are NOT escaped.
- UTF-8 multibyte chars are printable → emitted as-is (not `$'…'`).
- Missing arg (`printf '%q'` with no args) → bash emits `''` (empty-string
  default through `%q`); verify and match.
- `-v VAR` capture works (routes through the existing `-v` path unchanged).

---

## Feature 2 — `set -f` / `set -o noglob` (disable pathname expansion)

### Background — bash behavior (probed this session)
`set -f` (= `set -o noglob`) disables filename generation: `*`/`?`/`[` stay
literal in command words, even when files match. It is **pathname-only** —
`case`, `[[ == ]]`, and `${var//pat}` glob matching are UNAFFECTED:
```
set -f; echo *.txt          # *.txt  (literal, even if x.txt exists)
set +f; echo *.txt          # x.txt  (restored)
set -f; case abc in a*) …   # still matches (CASE-Y)
set -f; echo "${s//[0-9]/_}"  # still substitutes
set -f; [[ x == ? ]]        # still matches (BR-Y)
set -euf; echo $-           # efhuB  ($- order: f after e)
```

### Architecture
Wire a real `noglob` toggle (huck already lists `noglob` in `SETO_TABLE` but
`option_set` returns `Unimplemented`) and gate the single pathname-glob site on
it.
- `ShellOptions` (`src/shell_state.rs:107`): add `pub noglob: bool` (mirrors
  `verbose`/`xtrace`).
- `option_set` (`builtins.rs:4289`): add `"noglob" => { shell.shell_options.noglob = value; Ok(()) }`.
- `option_get`: add a `noglob` arm returning `shell_options.noglob` (so
  `[[ -o noglob ]]`, `set -o`, and `set +o` listing reflect it).
- Short flags: in the `set -` loop add `b'f' => shell.shell_options.noglob = true`,
  and in the `set +` loop `b'f' => shell.shell_options.noglob = false` (replacing
  the `other => not yet supported` fall-through for `f`).
- `dollar_dash_value` (`shell_state.rs:418`): push `'f'` when
  `shell_options.noglob`, positioned AFTER `e` (bash order `efhuB`): so the order
  becomes `e f i u v x`.
- `GlobOpts` (`expand.rs:10`): add `pub noglob: bool`; `glob_opts()`
  (`shell_state.rs:~430`) sets it from `self.shell_options.noglob` (it's a `set`
  option, not `shopt`).
- Gate: in `glob_expand_fields_opts` (`expand.rs:1409`), when `opts.noglob`,
  take the literal-field path (push `field.chars`, no globbing) — short-circuit
  BEFORE the `has_unquoted_metachar`/`is_extglob` glob decision. So `*`/`?`/`[`
  stay literal. POSIX-class routing (v119) and extglob are also skipped when
  noglob (bash: `set -f` disables all filename generation).

### Correctness / scope
- noglob affects ONLY pathname expansion (the one gate). `case` (executor),
  `[[ == ]]` (executor), `${var//pat}` (pe_pattern_matches), and completion
  matching do NOT consult `noglob` — verified bash leaves them active.
- Brace expansion, tilde, `$()`, parameter expansion are unaffected (separate
  passes).
- `set +f` restores globbing; `[[ -o noglob ]]` and `$-` reflect state.

---

## Files & responsibilities

| File | Change |
|------|--------|
| `src/param_expansion.rs` | extract `ansi_c_quote` from `shell_quote` (refactor) |
| `src/builtins.rs` | `ConvChar::Q` + `printf_q` + `format_one` arm + cycling-consumes; `ShellOptions.noglob` wiring in `option_set`/`option_get`/short-flags |
| `src/shell_state.rs` | `ShellOptions.noglob` field; `dollar_dash_value` `f`; `glob_opts()` noglob |
| `src/expand.rs` | `GlobOpts.noglob` + the `glob_expand_fields_opts` gate |
| `tests/printf_q_noglob_integration.rs` | NEW — `%q` + noglob, vs bash |
| `tests/scripts/printf_q_noglob_diff_check.sh` | NEW — 44th harness |
| `docs/bash-divergences.md`, `README.md` | M-73 drop `%q` from deferred; M-08 drop `-f`/noglob from deferred; changelog; README row |

## Testing
1. **Unit** (`src/builtins.rs`): `printf_q` — plain/space/quote/`$`/`*`/empty/
   control(`$'…'`)/UTF-8/the full escape set; `ansi_c_quote` refactor leaves
   `@Q` byte-identical (run the existing `@Q` tests). Unit on `option_set`
   noglob round-trip + `dollar_dash` `f`.
2. **Integration** (`tests/printf_q_noglob_integration.rs`, binary vs bash,
   file-arg per L-27): `printf %q` (plain, space, quote, `$`, glob chars,
   control→`$'…'`, UTF-8, cycling `%q a b c`, `%6q` width, `-v` capture);
   noglob (`set -f; echo *` literal; matching-file stays literal; `set +f`
   restores; `[[ -o noglob ]]`; `$-` has `f`; `set -f` leaves case/`${//}`/`[[`
   matching active). Each byte-identical to bash.
3. **44th bash-diff harness** `tests/scripts/printf_q_noglob_diff_check.sh` —
   `%q` spread + noglob spread, byte-identical.
4. **Regression**: full suite (2872+), all 44 harnesses, clippy `--all-targets`.
   Watch `printf`/`param`(`@Q`)/`glob`/`set`/`dbracket`/`case` suites.
5. **Payoff**: re-run the extracted `_mise`-handler context (or at minimum
   confirm `printf '%q'` and `set -o noglob`/`set -f` no longer error); report
   that the two `_mise` errors are gone. (Full end-to-end `mise<TAB>` still
   depends on the `mise` binary + the rest of `_mise`, which can't be exercised
   headless — honest: report the two specific errors cleared, and that the user
   should re-test live.)

## Edge cases & notes
- **`%q` precision** (`%.3q`): bash truncates the QUOTED string to N chars;
  confirm and match (or, if it's an unusual edge, note as a low `L-` divergence).
- **`%q` width with multibyte**: `pad_string` pads by char/byte as the existing
  `%s` does; match `%s`'s behavior (already shipped).
- **noglob + nullglob/failglob interaction**: noglob short-circuits before
  those apply, so a `set -f` pattern never triggers failglob/nullglob — matches
  bash (no filename generation at all).
- **`set -f` then a literal `[abc]`**: stays literal (no class matching in
  pathname context) — consistent with `*`/`?`.
