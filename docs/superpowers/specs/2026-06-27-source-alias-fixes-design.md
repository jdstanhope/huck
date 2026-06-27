# v231 — source path/device-file fixes + `expand_aliases` in non-interactive mode — Design

**Status:** approved (brainstorm 2026-06-27)
**Iteration goal:** Fix three bugs surfaced by the `builtins` bash-test category,
two of which are genuine `source`/`.` bugs and one of which is the dominant
blocker of the entire `alias` category:

- **A** — `source FILE` (no slash) does not fall back to CWD and ignores
  `shopt sourcepath` (source4.sub). *builtins-shrink.*
- **B** — `source` rejects non-regular files (`/dev/null`, `/dev/stdin`, fifos,
  procsub `/dev/fd/N`) via `Path::is_file()` (source6.sub). *builtins-shrink.*
- **C** — `shopt -s expand_aliases` is ignored in non-interactive (file/`source`/
  `-c`) mode: aliases are never expanded outside the REPL. This gates the **entire
  `alias` category** + source7.sub. *alias-category FLIP CANDIDATE.*

## Background & measurement (measure-first)

Re-measuring the post-v230 `builtins` diff shows **~13 independent blocker
clusters** (set +p, source4/6/7, declare prologue, kill prologue, echo escapes,
`declare -p` empty-array format, assoc-array ordering, nameref unset, `declare` of
a function, source `-o` option, exit-bad-arg abort, …). A/B/C clear only 3 — so
**`builtins` will NOT flip** (it is a broad-shrink for that category).

The value is lopsided toward **C**. Measuring the `alias` category: line 15 of
`alias.tests` is `shopt -s expand_aliases`, and **every** failure in `alias.diff`
is `command not found` from an un-expanded alias (`foo`/`a`/`myalias`/`a1`/`a2`).
So C is a **flip candidate for `alias`**, not just a shrink. A/B are small source
cleanups with no flip (B may also help `procsub`). User-confirmed scope: all three.

Diagnosis confirmed by direct repro (release huck vs bash):
- A: `shopt -u sourcepath; cd dir; . file_in_cwd` → huck "No such file or
  directory"; bash sources it.
- B: `. /dev/null` and `echo x | . /dev/stdin` → huck "No such file or directory"
  rc 1; bash rc 0 (and runs the piped content).
- C: `shopt -s expand_aliases; alias foo='echo X'; foo` → huck "foo: command not
  found"; bash prints `X`. (Control: WITHOUT `expand_aliases`, both shells
  correctly suppress the alias — so huck's bug is specifically ignoring the shopt.)

## Part A — source path resolution (`resolve_source_path`)

`resolve_source_path` (builtins.rs ~6245) currently: if the filename contains a
slash, accept iff `is_file()`; else search `$PATH` only; else `None`. Two bugs:
it ignores `shopt sourcepath` and never falls back to CWD.

Match bash for a **no-slash** filename:
- `sourcepath` **on** (default): search each `$PATH` dir for the file; if not
  found in PATH, **fall back to `./FILE` (CWD)**.
- `sourcepath` **off** (`shopt -u sourcepath`): **CWD only** — do not search PATH.
- filename **with** a slash: unchanged (resolve relative to CWD as written).

The PATH/CWD candidate acceptance uses the Part-B file-type test (accept any
existing non-directory), not `is_file()`.

`shopt sourcepath` already exists (`shell_state.rs:289`, `default: true` — matches
bash). Read it via `shell.shopt_options.get("sourcepath")`.

*Out of scope (separate blocker in the same builtins hunk):* the adjacent
`cd bash-dir-a` / `/tmp/bash-dir-a` lines are a **CDPATH** gap, not a source bug —
A will not clear them.

## Part B — source device-files / fifos / procsub fds

`resolve_source_path` rejects `/dev/null`, `/dev/stdin`, fifos, and procsub
`/dev/fd/N` because `Path::is_file()` is false for non-regular files. The v229
source restructure already distinguishes a **directory** ("is a directory") and a
**binary/non-UTF-8** file ("cannot execute binary file"); this part adds: a path
that **exists and is not a directory** is accepted (regular file, char device,
fifo, symlink-to-device).

Fix: replace the `is_file()` acceptance check (both the slash branch and the PATH/
CWD candidates) with **"exists and is not a directory"**. Concretely, stat the
path (following symlinks, e.g. `std::fs::metadata`); accept if the stat succeeds
and the target is not a directory; a directory continues to route to the v229
"is a directory" handling; a stat failure (ENOENT) → not-found.

huck reads sourced content with `std::fs::read_to_string` (whole-file, UTF-8),
which already streams `/dev/null` (→ empty → rc 0), `/dev/stdin` (→ reads huck's
stdin to EOF), and a fifo (→ blocks until the writer closes, then EOF) correctly
for the finite ASCII content these tests use. No reader change is required.

The procsub case `. <(echo "echo two - OK")`: the `<(…)` argument is realized
during the source command's word expansion to a readable `/dev/fd/N`; `source`
then opens/reads it. Included in scope; **flagged for verification** that huck's
procsub fd is open and readable for the duration of the source read (the procsub
lifecycle must span the synchronous read — confirm during implementation; if the
procsub fd is reaped too early, note it as a follow-on rather than expanding
scope).

## Part C — `expand_aliases` in non-interactive mode

huck already has a correct alias expander, `alias_expand::expand_aliases_in_tokens`
(command-position eligibility, cycle protection, the trailing-space continuation
rule), wired into the interactive REPL via `process_line`. The non-interactive
file/`source`/`-c` seam — `run_sourced_contents_in_sinks` (builtins.rs ~6285) —
never calls it, so `shopt -s expand_aliases` is silently ignored in scripts.

### The execution-loop seam

`run_sourced_contents_in_sinks` runs an `'outer` loop that:
1. `tokenize_partial(&contents[start..])` → `(tokens, offsets, lex_lines, terr)`;
   `offsets[i]` is the byte offset of token `i` within the chunk.
2. builds a `TokenCursor` and, in an inner loop, `parse_one_unit` parses ONE
   logical unit at a time, then executes it. It uses `offsets[unit_start_idx]` /
   `offsets[unit_end_idx]` to compute `unit_end_abs` (advance `start`) and to
   slice `contents[prev_end..unit_end_abs]` for `set -v` echo.

Alias expansion **rewrites the token stream**, so naïvely expanding breaks this
offset bookkeeping. The design preserves it with a provenance map.

### Mechanism

1. **Activation:** compute `expand = shell.is_interactive ||
   shell.shopt_options.get("expand_aliases").unwrap_or(false)`, read fresh on each
   chunk (so a mid-script `shopt -s expand_aliases` takes effect for later lines).
   When false, the loop is byte-for-byte unchanged (no regression).

2. **Provenance-mapped expansion:** add
   `expand_aliases_in_tokens_mapped(tokens, aliases) -> Result<(Vec<Token>,
   Vec<usize>)>` returning, per output token, the **index of the source token it
   came from**. Alias-body tokens inherit the index of the alias-name token they
   replaced; untouched tokens map to themselves. The existing
   `expand_aliases_in_tokens` becomes a thin wrapper that drops the map. After
   expanding the chunk, the loop remaps the parallel arrays:
   `offsets2[j] = offsets[map[j]]`, `lines2[j] = token_lines[map[j]]`.

   This anchors every offset to the **original source bytes**. For
   `alias ll='ls -l'; ll /usr`: the expanded tokens `ls`/`-l` both carry the
   offset of the source `ll` (offset 0); the real `/usr` keeps its source offset.
   Consequences (both bash-correct and desired):
   - `set -v` echoes the RAW line (`ll /usr`), because the slice uses original
     offsets — bash's `set -v` also prints input as read, pre-expansion.
   - `start` advances by the RAW byte length (past `/usr`), so the next line is
     read correctly regardless of how many tokens the expansion produced.

3. **Correct def-then-use timing (no double-expansion):** an alias defined by an
   executed command must affect *subsequently-parsed* commands. Add a
   `shell.alias_generation: u64` counter bumped by the `alias` and `unalias`
   builtins. After executing each unit, if the generation changed during that
   unit, set `start = unit_end_abs` and `continue 'outer` to **re-tokenize +
   re-expand the remaining raw bytes** with the updated table. Re-expanding from
   raw bytes (never from already-expanded tokens) means each source token is
   expanded exactly once — no double-expansion (e.g. `ll`→`ls -l` is never
   re-applied to its own output). When no alias/unalias ran in the chunk, there is
   no re-tokenize and no added cost.

### Known edge-limits (document, do not chase)

- An alias defined **and first used within the same logical unit** (e.g. a single
  compound command, or `alias x=…; x` on one physical line parsed as one unit):
  the use is expanded with the pre-unit alias table, so it may not expand. bash's
  incremental reader handles some of these; matching it exactly would require a
  push-back lexer. Not exercised by the alias category's def-then-use-on-later-
  line patterns.
- `set -v` echo of a line whose alias expansion spans a unit boundary is anchored
  to the alias-name token; exotic but bash-faithful for the common case.

### Risk

C touches the hot path for ALL non-interactive script execution. Regression is
guarded by `cargo test --workspace` + the full `*_diff_check.sh` sweep. Because
expansion is gated on `is_interactive || expand_aliases`-shopt (default off), only
scripts that explicitly enable the shopt change behavior.

## Non-goals

- The other ~10 `builtins` blocker clusters (set +p, declare/kill prologue, echo
  escapes, `declare -p` array format, assoc ordering, nameref unset, `declare` of
  a function, source `-o`, exit-bad-arg abort) and **CDPATH** — independent,
  separate iterations.
- A push-back/streaming lexer for fully-bash-faithful mid-unit alias expansion
  (the documented edge-limits above).
- Any claim that `builtins` flips — it does not.

## Testing & verification

- **Unit tests:** `resolve_source_path` (slash / PATH-found / CWD-fallback /
  sourcepath-off-CWD-only / device-file accepted / directory still rejected);
  `expand_aliases_in_tokens_mapped` (the provenance map: `ll /usr` →
  `ls -l /usr` with map `[0,0,1]`; no-op map is identity).
- **Integration tests (file mode, `run_file` + `AtomicU64`):**
  - A: `shopt -u sourcepath; . file_in_cwd` sets positional args; `sourcepath` on
    finds a PATH file then falls back to CWD.
  - B: `. /dev/null` (rc 0), `echo CMD | . /dev/stdin` (runs CMD, rc 0), a fifo,
    and `. <(echo CMD)` (procsub).
  - C: `shopt -s expand_aliases; alias foo='echo X'; foo` → `X`; def-then-use
    across lines; `unalias` then use → command-not-found; trailing-space
    continuation (`alias a='b '; alias b='echo'; a c`); and the negative —
    WITHOUT the shopt, the alias is NOT expanded.
- **Diff-check harnesses (byte-identical vs bash, file mode, same temp path):**
  - `source_device_diff_check.sh` — A (CWD/sourcepath on+off) + B (`/dev/null`,
    `/dev/stdin` via pipe, a fifo). (Procsub byte-identity included if it verifies
    cleanly; else covered by integration only.)
  - `alias_expand_diff_check.sh` — C: def-then-use, unalias, trailing-space,
    shopt-off negative.
- **Regression:** `cargo test --workspace` (0 failed) + all
  `tests/scripts/*_diff_check.sh` (`funcnest_diff_check.sh` release-only). Watch
  for any script-execution regression from the C seam change.
- **Category re-measure:** `alias` (the FLIP target — report whether it flips or
  the exact residual that remains), `builtins`, `procsub`. No `builtins` flip
  expected; `alias` is the deliverable to watch.

## Risks summary

- **C / offset bookkeeping** is the intricate part — the provenance map must keep
  `unit_end_abs` and `set -v` correct after token rewrites; the re-tokenize-on-
  alias-change loop must not double-expand or infinite-loop (the generation
  counter only triggers a re-tokenize when the table actually changed, and `start`
  always advances past the executed unit, so the loop strictly progresses).
- **B / procsub fd lifecycle** — confirm the `<(…)` fd survives the source read.
