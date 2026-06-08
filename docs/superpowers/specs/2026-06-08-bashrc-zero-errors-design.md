# huck v109 — zero-error `~/.bashrc`: M-90 / `export -a` / `${arr[@]±word}` Design

**Status:** approved design, ready for implementation plan.
**Implements:** the three remaining (non-fatal) gaps that leak errors when the
user's `~/.bashrc` sources mise — clearing them makes `~/.bashrc` source with
**zero errors**:
1. **M-90** `[fixed v109]` — builtin error output now honors `2>` / `2>>` / `2>&1`.
2. **`export -a`** (updates **M-89**) — accept the `-a` flag (no-op, like bash).
3. **`${arr[@]±word}`** (array all-elements with a set/unset `+`/`-`/`:+`/`:-`
   modifier; updates **M-82**) — the safe-array-expansion idiom
   `${arr[@]+"${arr[@]}"}`.
**Why now:** v108 made `~/.bashrc` source without hanging; these are the last 6
leaked messages (3× `declare -p … 2>/dev/null` via M-90, 1× `export -a`, 2×
`${__MISE_FLAGS[@]+…}`). All from mise's activation.
**Branch (impl):** `v109-bashrc-zero-errors`.

The three fixes are independent (different files/layers) and can be done in
parallel. Each MUST be verified byte-identical to the **system bash** (the harness
is the gold standard).

## Gap 1 — M-90: builtin stderr honors `2>` (`src/executor.rs`)

**Root cause:** `open_stage_files` opens `files.stderr: Option<File>` for a
`2>file`/`2>>file` redirect (`src/executor.rs:~2339`), but the in-process builtin
exec path applies only `files.stdin` and `files.stdout` — `files.stderr` is
dropped unused, so builtins' `eprintln!`/`eprint!` (210 sites in `src/builtins.rs`)
always hit the real fd 2.

**Fix:** mirror the existing stdin RAII guard (`BuiltinStdinGuard` /
`prepare_builtin_stdin`, `src/executor.rs:~2200-2259`) for stderr:
- Add `struct BuiltinStderrGuard { saved_fd: RawFd }` with a `Drop` that
  `dup2(saved_fd, 2)` + `close(saved_fd)`.
- Add `fn prepare_builtin_stderr(stderr: Option<File>) -> Result<Option<BuiltinStderrGuard>, ()>`
  that, when a target is present, `let saved = dup(2); dup2(target_fd, 2);` and
  returns the guard (so it restores on drop after the builtin runs). `eprintln!`
  writes to libc fd 2, so this captures it.
- Apply it in BOTH in-process builtin arms: the regular builtin arm
  (`src/executor.rs:~2659`) and the control-builtin arm (`~2643`), alongside the
  existing stdin/stdout application, holding the guard for the duration of the
  `run_builtin` call.
- **`2>&1` (Dup) for builtins**: `Redirect::Dup { fd:2, source }` currently
  resolves to `stderr: None` (handled only in `run_subprocess`'s pre_exec). For an
  in-process builtin, `2>&1` must dup2 the CURRENT fd 1 target onto fd 2 for the
  builtin's duration. Handle this in the same guard mechanism: when the resolved
  stderr is a Dup-to-1 (or a Dup to another fd), `dup2(<that fd>, 2)`. Order
  matters vs the stdout redirect — if `>out 2>&1`, fd 1 is the file and fd 2 must
  follow it; if `2>&1 >out`, fd 2 follows the OLD fd 1. Match bash's
  left-to-right semantics (apply redirects in source order). For builtins whose
  stdout goes through the `out: &mut dyn Write` writer (not an fd dup), `2>&1`
  should send stderr to wherever fd 1 currently points — verify against bash for
  `{ declare -p X; } 2>&1` style cases; if the writer-vs-fd split makes exact
  `2>&1`+capture parity hard, scope `2>&1` to the common `cmd 2>&1` (fd-level) case
  and note any residual as a low `L-` divergence. The **file** case
  (`2>file`/`2>>file`) is the primary M-90 fix and is fully clean.

**Must-not-regress:** builtins without a stderr redirect (`eprintln!` still to real
fd 2); external commands (already correct via pre_exec); the stdin/stdout guards;
capture-mode command substitution; nested redirects.

## Gap 2 — `export -a` (`src/builtins.rs`, updates M-89)

**Root cause:** `builtin_export_decl` (`src/builtins.rs:~1156`, the DeclArg entry
dispatched for `export`) has NO flag parsing — every arg is a name/assign, so `-a`
hits `is_valid_name` and errors `export: '-a': not a valid identifier` (`~1195`).
`mise activate bash` runs `export -a chpwd_functions`.

**Bash contract (RE-VERIFY against the system bash in the impl):** the investigation
found bash 5.2 accepts `export -a` (rc 0, no error) and `export -a NAME` exports
NAME. `-a` is not a documented export flag but is tolerated as a no-op here.

**Fix:** add a leading-flag prelude to `builtin_export_decl` (before the per-arg
loop), modeled on `builtin_local_decl`'s flag loop (`src/builtins.rs:~1244`):
consume leading `DeclArg::Plain` args that are `-a` (and, for free,
`-p`/`-n`/`-f` — all currently broken the same way) as flags, then run the existing
name/assign loop over the remainder. `-a` is a **no-op** (huck has no allexport
attribute). `-p` (print) / `-n` (un-export) / `-f` may be wired or accepted-as-no-op
per what's cleanly reachable — but ONLY `-a` is required for the payoff; keep the
others' behavior at least non-erroring if trivially so, else leave them and scope
to `-a`. Confirm `export -a chpwd_functions` then `declare -p chpwd_functions`
shows it exported, matching bash.

**Must-not-regress:** `export NAME=val`, `export NAME`, bare `export` listing,
`export -p`-if-already-supported, readonly+export interactions.

## Gap 3 — `${arr[@]±word}` (`src/expand.rs`, updates M-82)

**Root cause:** `expand_array_param`'s `match (modifier, subscript)`
(`src/expand.rs:~537`) has a catch-all (`~639`) that rejects EVERY scalar modifier
when the subscript is `[@]`/`[*]`:
```rust
(other, SK::All | SK::Star) => { eprintln!("…modifier {:?} not supported on array in v71…"); … }
```
`expand_assoc_param` has the identical rejection (`~391`).

**Bash contract (verified):** with `a=(x y z)`, unset `b`, empty `e=()`:
- `${a[@]+word}` → `word` (one word) if the array has ≥1 element, else nothing;
  `${b[@]+word}`/`${e[@]+word}` → nothing.
- `${a[@]-word}` → the array elements (`x y z`, word-split like `${a[@]}`) if any
  element, else `word`; `${b[@]-word}` → `word`.
- Colon variants (`:+`/`:-`) behave identically when elements are non-null (the
  observed cases); treat a non-empty array as "set and non-null".
- **Set predicate** = `!collect_values(shell).is_empty()` (empty array `()` counts
  as unset — `collect_values` of `()` is empty).

**Fix:** replace the catch-all with arms for the set/unset modifiers on
`SK::All | SK::Star` (and mirror into `expand_assoc_param`):
- `(PM::UseAlternate { word, .. }, SK::All|SK::Star)`: if set → expand `word`
  (`expand_word_to_string`, split per the quoting), else `Empty`.
- `(PM::UseDefault { word, .. }, SK::All|SK::Star)`: if set → the existing
  all-elements result (reuse the `(PM::None, SK::All)` `WordList(collect_values)` /
  `SK::Star` joined-`Value` logic, quoting-aware), else expand `word`.
- Keep `:=` (`AssignDefault`) and `:?` (`ErrorIfUnset`) on `[@]` rejected (out of
  scope — and assigning to a whole array via `:=` is a bash error anyway); only
  `+`/`-` (both colon variants) are added. Per-element substitution/case-mod
  (`${arr[@]/p/r}`, `${arr[@]^^}`) stay deferred (separate M-82 follow-on).

The common idiom this enables: `${arr[@]+"${arr[@]}"}` → the quoted array
expansion if the array is set, else nothing (safe under `set -u`).

**Must-not-regress:** plain `${arr[@]}`/`${arr[*]}`/`${#arr[@]}`/`${!arr[@]}`,
single-element `${arr[i]±word}` (already works), scalar `${var±word}`.

## Files & responsibilities

| File | Change |
|------|--------|
| `src/executor.rs` | `BuiltinStderrGuard` + `prepare_builtin_stderr`; apply in both builtin arms; handle `2>&1` for builtins (Gap 1) |
| `src/builtins.rs` | leading-flag prelude in `builtin_export_decl` accepting `-a` (Gap 2) |
| `src/expand.rs` | `UseAlternate`/`UseDefault` arms on `SK::All`/`SK::Star` in `expand_array_param` + `expand_assoc_param` (Gap 3) |
| `tests/bashrc_zero_errors_integration.rs` | NEW — all three |
| `tests/scripts/bashrc_zero_errors_diff_check.sh` | NEW — 33rd bash-diff harness |
| `docs/bash-divergences.md`, `README.md` | M-90 `[fixed v109]`; M-89 `-a`; M-82 `±` array modifier; changelog; README row |

## Testing

1. **M-90**: `declare -p UNSET 2>/dev/null; echo after` → `after` only (no leak);
   `declare -p UNSET 2>/tmp/e; cat /tmp/e` captures the message; `false 2>/dev/null`
   / a builtin error with `2>&1 | grep` works; bare builtin error still reaches
   stderr. Each byte-identical to bash.
2. **export -a**: `export -a FOO=bar; declare -p FOO` → exported; `export -a NAME`
   (mise shape) → rc 0, no error; `export NAME=v` unchanged.
3. **`${arr[@]±}`**: `a=(x y z); echo "${a[@]+SET}"` → `SET`; `unset b; echo "[${b[@]+SET}]"`
   → `[]`; `echo "${a[@]-DEF}"` → `x y z`; `echo "[${b[@]-DEF}]"` → `DEF`; the idiom
   `set -u; a=(1 2); printf '<%s>' "${a[@]+"${a[@]}"}"` → `<1><2>`; empty array
   `e=(); echo "[${e[@]+SET}]"` → `[]`. Assoc `declare -A m=([k]=v); echo "${m[@]+SET}"`
   → `SET`. All vs bash.
4. **bash-diff harness** `tests/scripts/bashrc_zero_errors_diff_check.sh` (33rd):
   deterministic fragments for all three, byte-identical to bash 5.2.
5. **Regression**: full suite (2725+), all 32 harnesses.
6. **Payoff**: sourcing `mise activate bash` output emits **zero** `not a valid
   identifier` / `not supported on array` / `declare: … not found` lines (report
   the count before/after).

## Edge cases & notes
- **M-90 `2>&1` writer split**: if exact `{ builtin; } 2>&1` capture parity is hard
  due to the stdout-writer vs fd-1 split, scope `2>&1` to the fd-level `cmd 2>&1`
  case and log a low `L-` note; the file redirect (the mise case) is the priority.
- **export `-a` is a no-op** (no allexport attribute tracked) — document if a
  later `declare`-listing would differ from bash (it shouldn't, since `-a` here is
  just "export these names").
- **Colon vs non-colon on whole arrays**: huck treats a non-empty array as
  set-and-non-null (can't have a "set but null" whole array meaningfully); matches
  all observed bash cases.
