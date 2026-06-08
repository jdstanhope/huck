# huck v110 — genuinely zero-error `mise activate`: M-90 combined-redirect + M-105 Design

**Status:** approved design, ready for implementation plan.
**Implements:** the two residual issues that an end-to-end `mise activate bash`
smoke surfaced after v109 — fixing both makes `mise activate` (and the rest of
the user's `~/.bashrc`) source through huck with **zero errors**:
1. **M-90 combined-redirect** `[fixed v110]` — a builtin's `>file 2>&1`
   (e.g. `declare -p X >/dev/null 2>&1`, mise line 29) now routes the builtin's
   stderr to the redirected stdout file instead of leaking to the terminal.
2. **M-105** `[fixed v110]` — an UNQUOTED `${x+alt}` that expands to nothing no
   longer emits a spurious empty field (which was injecting an empty `''`
   argument into `mise hook-env ${__MISE_FLAGS[@]+"${__MISE_FLAGS[@]}"} -s bash`).
**Why now:** v109 cleared 3 of the 5 mise-activation leak types; these are the
last 2. After v110 the payoff smoke must show **0** error lines (v109: 4).
**Branch (impl):** `v110-mise-zero-errors`.
**Scope (locked):** A + B-minimal only. The converse-M-105 (a *set-but-null*
scalar through a quoted-inner alternate, `${u+"$u"}` with `u=`) and the
capture-mode `$(builtin 2>&1)` residual (L-25) stay documented deferred
divergences — NOT in v110.

The two fixes are independent (different files/layers) and can be implemented in
either order. Each MUST be verified byte-identical to the **system bash** (the
diff harness is the gold standard).

## Part A — M-90 combined redirect: builtin `>file 2>&1` (`src/executor.rs`)

**Root cause:** v109 made in-process builtins honor a stderr redirect via
`prepare_builtin_stderr(stderr: Option<File>, dup_to_stdout: bool)` and a
`BuiltinStderrGuard` (`src/executor.rs:~2266-2323`). For `2>&1` the
`dup_to_stdout` path dups the **real fd 1** (`libc::dup(STDOUT_FILENO)`). But an
in-process builtin whose stdout is redirected to a file writes stdout to a Rust
`File` object (the `match files.stdout { Some(mut file) => run_builtin(…,
&mut file, …) }` arm), NOT to fd 1. So the `2>&1` dup was gated off whenever
stdout was also redirected:
```rust
let dup_stderr_to_stdout = files.stderr.is_none()
    && files.stdout.is_none()          // <-- gates off `>file 2>&1`
    && matches!(sink, StdoutSink::Terminal)
    && stderr_dups_to_stdout(cmd, shell);
```
Both builtin arms have this: the control-builtin arm (`~2721`) and the regular
builtin arm (`~2772`). With `>/dev/null 2>&1`, `files.stdout` is `Some(/dev/null
File)`, so the dup is skipped and the builtin's `eprintln!` still hits the real
fd 2 → leak.

**Fix:** make `2>&1` target *wherever the builtin's stdout actually goes*:
1. Change `prepare_builtin_stderr`'s signature from
   `(stderr: Option<File>, dup_to_stdout: bool)` to
   `(stderr: Option<File>, dup_target: Option<RawFd>)`. Behavior:
   - `Some(file)` → `into_raw_fd()` and dup2 onto fd 2 (unchanged — the
     `2>file`/`2>>file` path).
   - `None` + `dup_target = Some(fd)` → `libc::dup(fd)` then dup2 the copy onto
     fd 2 (generalizes the old `dup(STDOUT_FILENO)` to any fd).
   - `None` + `dup_target = None` → no guard (unchanged).
2. In BOTH builtin arms, compute the dup target BEFORE the `match files.stdout`
   consumes the file (use `as_raw_fd()` — a borrow, does not consume):
   ```rust
   let stdout_fd: Option<RawFd> = files.stdout.as_ref().map(|f| f.as_raw_fd());
   let dup_target: Option<RawFd> = if stderr_dups_to_stdout(cmd, shell) {
       match (&files.stdout, &sink) {
           (Some(_), _) => stdout_fd,                       // >file 2>&1 → file's fd
           (None, StdoutSink::Terminal) => Some(libc::STDOUT_FILENO), // 2>&1 → real fd 1
           (None, StdoutSink::Capture(_)) => None,          // capture residual (L-25)
       }
   } else {
       None
   };
   let stderr_guard = prepare_builtin_stderr(files.stderr.take(), dup_target);
   let outcome = match files.stdout { … };   // file outlives the guard
   drop(stderr_guard);
   ```
   The `File` in `files.stdout` is not dropped until the `match` arm ends (after
   the builtin runs), so its fd stays valid for the dup. The dup'd fd 2 is an
   independent copy of the same open file description, so closing the `File`
   afterward does not invalidate fd 2; the guard then restores fd 2 on `Drop`.
   Both the `File` writer and fd 2 share the file offset, so stdout/stderr
   writes interleave by offset (matching bash's append order).

   The exact target-selection expression (`(&files.stdout, &sink)` match) is
   duplicated into both arms; an implementer may factor it into a small helper
   `fn builtin_stderr_dup_target(files, sink, cmd, shell) -> Option<RawFd>` if it
   reads cleaner, but a literal copy in each arm is acceptable (mirrors the
   existing duplicated `dup_stderr_to_stdout` block).

**Known residual (documented, not fixed in v110):** huck's `ExecCommand` keeps
only the **last** redirect per fd (`cmd.stdout`, `cmd.stderr` are each a single
`Option<Redirect>`), so left-to-right ordering between a stdout redirect and
`2>&1` is not preserved: `2>&1 >out` is represented identically to `>out 2>&1`,
and both will send fd 2 to the file. bash distinguishes them (`2>&1 >out` sends
fd 2 to the OLD fd 1). This is a pre-existing structural limitation, not
introduced by v110; mise uses the common `>/dev/null 2>&1` order. Note it as a
low `L-` divergence if not already covered.

**Must-not-regress:** `2>file`/`2>>file` (the v109 file path); bare `cmd 2>&1`
(Terminal sink, no stdout file — still dups real fd 1); a builtin with NO stderr
redirect (`eprintln!` still reaches the real fd 2); the stdin/stdout guards;
external commands (already correct via `run_subprocess` pre_exec); capture-mode
command substitution stdout.

## Part B — M-105: unquoted `${x+alt}`-yielding-nothing spurious empty field (`src/expand.rs`)

**Root cause:** in `expand()` (`src/expand.rs`), the `WordPart::ParamExpansion`
match's `ExpansionResult::Empty` arm (`~:898`) is:
```rust
crate::param_expansion::ExpansionResult::Empty => {
    has_emitted = true;
}
```
It sets `has_emitted = true` **unconditionally**, ignoring the part's `*quoted`
flag. At end-of-word, `if !current.is_empty() || has_emitted { result.push(current); }`
then pushes one empty field. So an unquoted parameter substitution that expands
to nothing (`${u+X}` / `${u:+X}` unset, or `${arr[@]+"${arr[@]}"}` on an
empty/unset array) wrongly contributes one empty field instead of vanishing.
`set -- ${u+X} a b; echo $#` → huck `3` vs bash `2`. This is PRE-EXISTING for
scalars; v109's M-87 exposed it for empty arrays, where it injects an empty `''`
argument into mise's `mise hook-env ${__MISE_FLAGS[@]+"${__MISE_FLAGS[@]}"} -s
bash`, breaking `mise activate` with `error: unexpected argument '' found`.

`ExpansionResult::Empty` is produced by: scalar `UseAlternate` when the operand
is null (`src/param_expansion.rs:151`), the assoc `UseAlternate` unset arm
(`src/expand.rs:404`), the v109 indexed-array `UseAlternate` unset arm, and a
defensive pending-fatal-error early return (`src/param_expansion.rs:58`).

**Fix (B-minimal):** make the arm quoted-aware:
```rust
crate::param_expansion::ExpansionResult::Empty => {
    // A quoted empty expansion ("${u+x}" when unset) still contributes one
    // empty field; an unquoted one vanishes (no field), matching bash.
    if *quoted {
        has_emitted = true;
    }
}
```
This uniformly corrects every `Empty` producer for the unquoted case. The
`expand_assignment()` Empty arm (`~:1015`) is already a no-op (`=> {}`) — no
field-splitting in assignment context — and stays unchanged.

**Bash contract (verify each against the system bash):**
- `set -- ${u+X} a b; echo $#` → `2` (was huck `3`).
- `f=(); set -- ${f[@]+"${f[@]}"} -s bash; echo $#` → `2` (mise shape — empty
  array, no spurious leading `''`).
- Quoted-empty still emits one field: `set -- "${u+x}" a; echo $#` → `2`
  (the quoted empty IS a field); `printf '<%s>' "${u+x}"; echo` → `<>` (one
  empty field).
- `${u-}` / `${u:-}` unset unquoted → still vanish (already via the `Value("")`
  path, unaffected).
- The idiom `a=(1 2); printf '<%s>' "${a[@]+"${a[@]}"}"` → `<1><2>` (unchanged
  from v109 — set array still yields its elements).

**Must-not-regress:** quoted empty fields (`""`, `"$unset"`, `"${u+x}"`); plain
`${unset}` (already vanishes via `Value("")`); `"$@"` / `$*` with no positional
params; word-splitting of non-empty values; the v109 array/assoc `±word`
behavior; `${u:-w}`/`${u:=w}`/`${u:?m}` paths (Value/Fatal, not Empty).

## Files & responsibilities

| File | Change |
|------|--------|
| `src/executor.rs` | `prepare_builtin_stderr` 2nd arg `bool` → `Option<RawFd>`; compute the dup target (file fd / real fd 1 / None) in both builtin arms; remove the `files.stdout.is_none()` gate (Part A) |
| `src/expand.rs` | `ExpansionResult::Empty` arm in `expand()` made quoted-aware (Part B) |
| `tests/mise_zero_errors_integration.rs` | NEW — both parts |
| `tests/scripts/mise_zero_errors_diff_check.sh` | NEW — 34th bash-diff harness |
| `docs/bash-divergences.md`, `README.md` | M-90 `[fixed v110]`; M-105 `[fixed v110]`; changelog; README row; Tier counts |

## Testing

1. **Part A (M-90 combined)**: `declare -p NOPE >/dev/null 2>&1; echo ok` →
   `ok` only (no leak); `declare -p NOPE 2>/dev/null; echo ok` → still `ok`
   (v109 file path intact); `{ declare -p NOPE 2>&1; } | grep -c NOPE` → `1`
   (bare `2>&1` intact); `declare -p FOO >/tmp/e 2>&1; cat /tmp/e` captures the
   error into the file. Each byte-identical to bash.
2. **Part B (M-105)**: the bash-contract list above — `$#` counts and `<%s>`
   field probes, scalar AND empty-array, quoted-still-one-field, unquoted-
   vanishes. Each byte-identical to bash.
3. **34th bash-diff harness** `tests/scripts/mise_zero_errors_diff_check.sh`:
   deterministic fragments for both parts, byte-identical to bash 5.2. Reuse the
   `check`/combined-stdout+stderr+exit pattern from
   `bashrc_zero_errors_diff_check.sh`.
4. **Regression**: full suite (2745+), all 34 harnesses, clippy clean.
5. **Payoff (the gate)**: source `mise activate bash` output through huck and
   report the error-line count BEFORE (v109: 4) and AFTER (target: **0**). Use a
   real `mise` if installed, else a synthetic fragment exercising both the
   `declare -p … >/dev/null 2>&1` and `${__MISE_FLAGS[@]+…}` shapes.

## Edge cases & notes
- **Part A fd lifetime**: the dup target is taken via `as_raw_fd()` (borrow)
  before `files.stdout` is consumed; the `File` lives until the `match` arm
  ends, after which the guard (already dropped) has restored fd 2. No use of a
  closed fd.
- **Part A capture residual (L-25)**: `(None, StdoutSink::Capture)` → `None`
  (no dup) — a Capture sink writes builtin stdout to a Rust buffer, not an fd,
  so fd-level `2>&1` cannot reach it; stays the documented deferred divergence.
- **Part A ordering residual**: `2>&1 >out` vs `>out 2>&1` indistinguishable in
  the AST (last-redirect-per-fd) — documented, not fixed.
- **Part B fatal-error Empty**: the `param_expansion.rs:58` pending-fatal early
  return also yields `Empty`; with the quoted-aware arm an unquoted one no
  longer emits a field, which is harmless (the pending fatal aborts the command
  anyway). No behavior change for the fatal path's observable result.
- **Part B converse (out of scope)**: `${u+"$u"}` with `u=` (set, empty) should
  emit one empty field but huck drops it (scalar `UseAlternate` "set" branch
  collapses the alternate via `expand_word_to_string`, losing the inner
  quoting). Deferred — would need field-preserving scalar-alternate expansion
  like M-87's array arms. Log/keep on M-105 as a known remaining sub-divergence.
