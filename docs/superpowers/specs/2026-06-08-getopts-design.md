# huck v111 — the `getopts` POSIX builtin (M-106) Design

**Status:** approved design, ready for implementation plan.
**Implements:** `getopts` — the POSIX option-parsing builtin huck is missing
(new **M-106**, Tier-2). This single builtin clears **both** errors seen on
`mise<TAB>`:
- `huck: command not found: getopts` (bash_completion's `_get_comp_words_by_ref`
  / `_init_completion` drive their arg parsing with `getopts`), and
- `bash_completion: : `-n': unknown argument` — a *cascade*: with `getopts`
  missing, the `while getopts …` loop never runs, so `OPTIND` never advances and
  the function's next loop trips on the unconsumed `-n` flag.
**Why now:** `~/.bashrc` sources cleanly (v110), but `mise<TAB>` completion
fails on the missing `getopts`. Verified root cause — the two errors are one
bug; implementing `getopts` fixes both.
**Branch (impl):** `v111-getopts`.
**Scope (locked):** `getopts` only. `FUNCNAME` (empty inside huck function
bodies — the blank `:` in `bash_completion: :`) is a SEPARATE gap logged as
**M-107** `[deferred]`; it only affects diagnostic text and the error branch is
no longer reached once `getopts` works.

## Background — the cascade (verified)

`/usr/share/bash-completion/bash_completion` line 374 (`_get_comp_words_by_ref`):
```bash
while getopts "c:i:n:p:w:" flag "$@"; do … n) exclude=$OPTARG ;; … done
while [[ $# -ge $OPTIND ]]; do case ${!OPTIND} in cur) … prev) … *) echo "…\`${!OPTIND}': unknown argument" >&2 ;; esac; ((OPTIND+=1)); done
```
Called as `_get_comp_words_by_ref -n : cur prev`. In bash, `getopts` consumes
`-n :` (setting `OPTIND=3`), so the second loop starts at `cur`. In huck,
`getopts` is command-not-found → the `while` condition returns 127 → the body
never runs → `OPTIND` stays `1` → `${!OPTIND}` is `-n` → the `*)` branch fires:
`bash_completion: <empty FUNCNAME>: \`-n': unknown argument`. Reproduced
exactly. Fixing `getopts` makes `OPTIND` advance and the cascade disappears.

## Component 1 — registration, signature, variables

`getopts` is a **regular** (non-special) builtin. Register it:
- add `"getopts"` to `is_builtin` (`src/builtins.rs`),
- add a `"getopts" => builtin_getopts(args, shell)` arm in `run_builtin`.

**Signature:** `getopts optstring name [arg …]`.
- Fewer than 2 operands → usage error to stderr, return 2 (bash:
  `usage: getopts optstring name [arg]`).
- With `arg …` present, parse those; otherwise parse the current
  `shell.positional_args` (the `$@` of the enclosing function/script).
- `name` is the variable getopts assigns the matched option character to (or
  `?` / `:` on error). If `name` is not a valid identifier → error.

**Shell variables** (read/written via `shell.lookup_var` / `shell.set`, the
`read`-builtin model):
- `OPTIND` — 1-based index of the next `arg` to process. Read at entry,
  defaulting to `1` when unset or non-numeric/`<1`; written back (as an integer
  string) on every return.
- `OPTARG` — set to an option's argument, or (silent mode) the offending option
  character. Unset/cleared where bash does.
- `OPTERR` — if its value is exactly `"0"`, suppress the verbose error messages
  (bash honors this); any other value (or unset) ⇒ messages on.

## Component 2 — the within-word cursor (clustered options)

Clustered options (`-abc`, one char per call) need a position *within* the
current word that `OPTIND` alone can't express (it doesn't move until the word
is fully consumed). Add to `Shell` two hidden fields (default `0`):
```rust
/// getopts: 1-based char offset of the next option char within the current
/// word (1 = "at a fresh word, re-check the leading dash"). Paired with
/// getopts_optind_cache to detect external OPTIND resets. (M-106)
pub getopts_sp: usize,
/// getopts: the OPTIND value getopts itself last wrote. If the live OPTIND
/// differs at entry, the caller reset it → start a fresh scan.
pub getopts_optind_cache: usize,
```
Initialize both to `0` at every `Shell` construction site (grep
`positional_args:` to find them). They are plain copied fields (a forked
subshell gets its own copy — fine; getopts state is per-process like bash's).

**Reset rule (bash-faithful):** at entry, read `OPTIND`. If
`OPTIND != getopts_optind_cache`, the caller changed `OPTIND` externally (e.g.
`local OPTIND=1` at the top of a completion function, or a new loop) → set
`getopts_sp = 1` (fresh word). When getopts advances `OPTIND` itself it sets the
cache to match, so its own advance is **not** seen as an external reset — this is
what makes clustered parsing of the *first* word work (where `OPTIND` stays `1`
across calls while `getopts_sp` walks `2,3,4,…`). The naive "reset when
`OPTIND==1`" rule is wrong precisely there.

## Component 3 — the per-call parse algorithm

A pure helper (testable without a process) computes the outcome from
`(optstring, args, optind, sp, silent)`; `builtin_getopts` wraps it with the
variable I/O. Algorithm:

```
silent = optstring.starts_with(':')
optind = read OPTIND (default 1, clamp <1 → 1)
if optind != cache: sp = 1            # external reset → fresh scan

if optind > args.len():               # options exhausted
    name = '?'; return DONE (rc 1)    # leave OPTIND
word = args[optind-1]
if sp == 1:                           # at a fresh word
    if word is empty OR word[0] != '-' OR word == "-":
        name = '?'; return DONE (rc 1)    # non-option; OPTIND unchanged
    if word == "--":
        optind += 1; name = '?'; return DONE (rc 1)   # explicit end of options
    sp = 2                            # skip the leading '-'
c = nth char of word at 1-based position sp
sp += 1
word_done = (sp > word.char_count())

if c == ':' OR c not found in optstring:        # invalid option
    name = '?'
    if silent: OPTARG = c.to_string()
    else:      unset OPTARG; if OPTERR != "0": eprintln "huck: illegal option -- {c}"
    if word_done: optind += 1; sp = 1
    return OPTION (rc 0)

if optstring marks c as taking an argument (next char in optstring is ':'):
    if !word_done:                              # arg attached: -bVAL
        OPTARG = rest of word from position sp; optind += 1; sp = 1
    else if optind + 1 <= args.len():           # arg is next word: -b VAL
        OPTARG = args[optind];     optind += 2; sp = 1
    else:                                        # missing argument
        if silent: name = ':'; OPTARG = c.to_string()
        else:      name = '?'; unset OPTARG; if OPTERR != "0": eprintln "huck: option requires an argument -- {c}"
        optind += 1; sp = 1
        return OPTION (rc 0)
    name = c
    return OPTION (rc 0)

else:                                            # plain valid option
    name = c
    if word_done: optind += 1; sp = 1
    return OPTION (rc 0)
```

On every return: write `OPTIND` back (integer string), set `getopts_optind_cache
= optind` and `getopts_sp = sp`, assign `name`, and set/unset `OPTARG` as above.
Return code: **0** while an option (even an invalid one) was processed; **>0
(1)** when options are exhausted, a non-option is reached, or `--` is hit — which
terminates `while getopts … ; do … ; done`.

### Detail notes
- **`name` assignment uses the caller scope** (`shell.set(name, …)`), like
  `read` — so a `while getopts … flag` loop sees `flag` update each iteration.
- **`OPTARG` unset vs empty:** in verbose-error cases bash unsets `OPTARG`;
  match by `shell.unset` (or set empty if huck has no unset-from-builtin path —
  verify against bash and pick the byte-matching one).
- **Invalid `name` / too few operands:** usage error (rc 2), message body
  matching bash, `huck:` prefix.
- **`OPTIND` written as the integer index**, never a char position.

## Component 4 — error-message divergence (L-note)

bash's verbose `getopts` prints `<shell>: illegal option -- c` and `<shell>:
option requires an argument -- c` to stderr (where `<shell>` is `$0`/script
name). huck matches the **message body** verbatim but uses its own `huck:`
prefix — the same prefix divergence every huck builtin has. The load-bearing
results (the `name` value, `OPTARG`, `OPTIND`, and the return code) are
byte-identical to bash. Logged as a low `L-` divergence (stderr text only).

## Files & responsibilities

| File | Change |
|------|--------|
| `src/builtins.rs` | `builtin_getopts` + a pure parse helper; register in `is_builtin` + `run_builtin` |
| `src/shell_state.rs` | `getopts_sp` + `getopts_optind_cache` fields (default 0) at every construction site |
| `tests/getopts_integration.rs` | NEW — binary-driven `while getopts` loops |
| `tests/scripts/getopts_diff_check.sh` | NEW — 35th bash-diff harness |
| `docs/bash-divergences.md`, `README.md` | M-106 `[fixed v111]`; M-107 (FUNCNAME) `[deferred]`; the getopts-message `L-` note; changelog; README row; Tier counts |

## Testing

1. **Integration** (`tests/getopts_integration.rs`, real `while getopts` loops):
   - simple `ab:c` over `-a -b val -c` → `a`, `b`(OPTARG=`val`), `c`; final OPTIND.
   - clustered `-abc` → `a`,`b`,`c` across calls (the cursor path).
   - attached arg `-bval` vs separate `-b val`.
   - `--` and a bare non-option terminate the loop with the right OPTIND.
   - missing arg: verbose `ab:` over `-b` → `opt=?`; silent `:ab:` over `-b` →
     `opt=:`, OPTARG=`b`.
   - invalid: verbose over `-z` → `opt=?`; silent over `-z` → `opt=?`,
     OPTARG=`z`.
   - no-`arg` form parses `$@`; explicit-arg form parses the given args.
   - `f() { local OPTIND=1; while getopts … ; done }` reset (bash_completion
     shape).
   - **regression**: the `_get_comp_words_by_ref -n : cur prev` shape resolves to
     `cur`/`prev` with NO `unknown argument`.
2. **Unit tests** on the pure parse helper — drive `(optstring, args, optind,
   sp)` and assert `(name, OPTARG, new optind, new sp, rc)` for each branch.
3. **35th bash-diff harness** `tests/scripts/getopts_diff_check.sh` —
   byte-identical fragments. Verbose-error fragments redirect stderr
   (`2>/dev/null`, reliable post-v110) and print `opt`/`OPTARG`/`OPTIND`/`rc`;
   silent-mode fragments compare directly.
4. **Regression**: full suite (2754+), all 35 harnesses, clippy clean.
5. **Payoff**: `mise<TAB>` (or the `_get_comp_words_by_ref -n : cur prev`
   fragment) through huck no longer prints `command not found: getopts` or the
   `-n: unknown argument` cascade. Report before/after.

## Edge cases & notes
- **Subshell**: `getopts_sp`/cache are plain copied fields; a forked subshell
  gets its own copy (per-process, like bash). No special handling.
- **`OPTIND` interplay with `local`**: bash_completion uses `local OPTIND=1`;
  the cache-mismatch reset handles it. Two interleaved getopts loops share the
  single hidden cursor (as bash does) and the reset-on-external-change rule keeps
  the common cases correct.
- **`OPTERR=0`** suppresses verbose messages (matches bash); silent mode (leading
  `:`) is independent and also suppresses + uses `name=:`/OPTARG.
- **Out of scope:** `FUNCNAME` population (M-107, deferred); GNU long-option
  extensions (getopts is short-options only in both bash and POSIX — nothing to
  do).
