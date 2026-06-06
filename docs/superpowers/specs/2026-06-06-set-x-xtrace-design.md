# huck v103 — `set -x` (xtrace) Design

**Status:** approved design, ready for implementation plan.
**Implements:** `set -x` / `set +x` / `set -o xtrace` / `set +o xtrace` — when on,
huck prints each command (after expansion) to stderr, prefixed by `$PS4` (default
`+ `), before executing it. Adds `x` to `$-`. Today huck rejects `set -x` with
"not yet supported in this version".
**Why now:** the last commonly-used `set -o` debugging option, AND the diagnostic
tool needed for the next task — pinpointing the source-time hang in nvm's
`nvm_ls_current` (it'll print the exact command that blocks). (v104 will use it.)
**Closes:** the `-x` half of **M-08** (re-`[fixed v103]` note; M-08 stays partial
for the remaining `-n`/`-f`/`-a`/`-C`/`-b`/`-h`).
**Branch (impl):** `v103-set-x-xtrace`.

## Verified bash 5.2 semantics (the contract — scoped)

- `set -x; echo hi` → stderr `+ echo hi`, stdout `hi`. The enabling `set -x` line
  itself is NOT traced; the disabling `set +x` line IS traced (it runs while
  xtrace is still on).
- Each SIMPLE command is traced after expansion: `x=hi; set -x; echo "$x" a` →
  `+ echo hi a`.
- Commands inside a called function / loop / subshell are traced as they run.
- `$-` includes `x` while xtrace is on.
- **PS4**: bash repeats PS4's first char by call-nesting depth (`+`/`++`/`+++`) and
  expands `$PS4`. **v103 scope (per brainstorm): FLAT** — emit `$PS4` literally
  (default `+ `), no depth-repeat, no `$PS4`-escape/var expansion. The depth-repeat
  + PS4-expansion are logged as a low divergence (every command still prints; the
  diagnostic value is unaffected; top-level depth-1 traces byte-match bash).
- xtrace writes to **stderr** (fd 2).

## Section 1 — Option plumbing

- `ShellOptions` (`src/shell_state.rs:107`) gains `pub xtrace: bool` (Default
  `false` — zero behavior change when off).
- `dollar_dash_value` (`src/shell_state.rs:~402`): push `'x'` when
  `shell_options.xtrace` (after `'v'`, keeping the alphabetical order `e i u v x`).
- `option_get` / `option_set` (`src/builtins.rs:~3948`): add
  `"xtrace" => …shell.shell_options.xtrace…` arms (`xtrace` is already in
  `SETO_TABLE`), so `set -o xtrace` / `set +o xtrace` work.
- `builtin_set` short-flag loop: add `b'x'` mirroring EXACTLY how `b'v'` (verbose)
  is wired for both `-x` (enable) and `+x` (disable). (Grep `b'v'` / `verbose` and
  replicate in the same place(s) — including whatever path handles the `+`
  prefix.)

## Section 2 — Trace emission (`src/executor.rs`)

In `run_exec_single`, after `resolve(cmd, shell)` (the command is now expanded:
`resolved.program` + `resolved.args`) and AFTER the `command`-bare-form
interception (v99) — but BEFORE the dispatch chain runs the command — emit the
trace when `shell.shell_options.xtrace`:

```rust
if shell.shell_options.xtrace {
    let ps4 = shell.lookup_var("PS4").unwrap_or_else(|| "+ ".to_string());
    let mut line = String::new();
    // (inline-assignment prefix: best-effort — see note)
    if !resolved.program.is_empty() {
        line.push_str(&resolved.program);
        for a in &resolved.args { line.push(' '); line.push_str(a); }
    } else {
        // pure assignment / redirect-only command: render the applied assignments
        // (reconstruct `name=value` from the values just set — do NOT re-expand).
    }
    eprintln!("{ps4}{line}");
}
```

Requirements / guidance:
- **Emit BEFORE execution** so a command that hangs is still printed (this is the
  whole point for the nvm diagnosis).
- **Expanded form**: use `resolved.program`/`resolved.args` (already expanded). Do
  NOT re-run expansion (would double-execute command substitutions / re-trigger
  side effects).
- **Pure assignments** (`x=1`, no program): trace `name=value` using the value
  AS APPLIED (read back after `apply_inline_assignments`, or from the assignment’s
  evaluated result) — not by re-expanding.
- **Inline-assignment PREFIX on a command** (`VAR=v cmd`): bash traces
  `VAR=v cmd`. v103 may either include it (reconstructed from applied values,
  no re-expansion) OR trace just `cmd` and log the missing-prefix as a low
  divergence — implementer's choice based on what's cleanly reachable; do NOT
  double-expand. Document whichever.
- **Recursion is automatic**: commands inside a called function / loop / `( )` run
  through `run_exec_single` too, so they trace without extra work (flat `+ ` each,
  per scope).
- **Capture mode**: xtrace goes to stderr regardless of the stdout `StdoutSink`
  (Terminal/Capture) — `$( set -x; cmd )` traces to stderr while stdout is
  captured, matching bash.

## Files & responsibilities

| File | Change |
|------|--------|
| `src/shell_state.rs` | `ShellOptions.xtrace`; `dollar_dash_value` pushes `x` |
| `src/builtins.rs` | `option_get`/`option_set` `xtrace` arms; `builtin_set` `-x`/`+x` short flags |
| `src/executor.rs` | `run_exec_single`: emit `$PS4`-prefixed expanded command to stderr when `xtrace`, before dispatch |
| `tests/set_x_integration.rs` | NEW — `set -x` traces simple cmd / multiple cmds / inside function / `$-` has `x` / `set +x` stops; capture-mode |
| `tests/scripts/set_x_diff_check.sh` | NEW — 28th bash-diff harness (top-level depth-1 fragments) |
| `docs/bash-divergences.md`, `README.md` | M-08 `-x` note `[fixed v103]`; PS4-depth/expansion + inline-prefix divergences; changelog; README row |

## Testing

1. **Unit**: `set -x` flips `shell_options.xtrace`; `$-` includes `x`; `set +x`
   clears it; `set -o xtrace`/`+o xtrace` likewise.
2. **Integration** (`tests/set_x_integration.rs`) — capture STDERR (the `run`
   helper must split stderr) and assert:
   - `set -x; echo hi` → stderr contains `+ echo hi`, stdout `hi\n`.
   - `x=hi; set -x; echo "$x" a` → stderr `+ echo hi a`.
   - `set -x` enabling line NOT traced; `set +x` line IS traced.
   - inside a function: `f() { echo in; }; set -x; f` → stderr has `+ f` and
     `+ echo in` (recursion).
   - `set -x; echo a; set +x; echo b` → `+ echo a` and `+ set +x` traced, `+ echo
     b` NOT.
   - `$-`: `set -x; case "$-" in *x*) echo on;; esac` → `on`.
   - capture: `r=$(set -x; echo cap); echo "[$r]"` → stdout `[cap]` (the trace is
     on stderr, not captured).
3. **bash-diff harness** `tests/scripts/set_x_diff_check.sh` (28th): TOP-LEVEL,
   depth-1, default-PS4 fragments whose combined stdout+stderr byte-matches bash —
   e.g. `set -x; echo hi`, `x=1; set -x; echo "$x"`, `set -x; true; set +x; echo
   done`. AVOID nested/function fragments (PS4 depth differs) and `$PS4`
   customization in the harness (test those via integration / note as divergence).
   Confirm bash's exact output for each fragment first (bash may trace slightly
   differently — e.g. quoting in the expanded form); pick fragments where they
   agree, and STOP+report if a top-level simple-command trace genuinely diverges.
4. **Regression**: full suite — `set -e`/`-u`/`-v`/`-o`, `$-`, the existing set
   tests, and the verbose (v89) tests must be unaffected (xtrace default off).
5. **End-to-end (the payoff, noted not blocking)**: `set -x` then source a
   reduced nvm fragment / `nvm_ls_current` — confirm the trace prints each command
   and the LAST printed line before the hang identifies the blocking command
   (feeds v104). (Full nvm source under `set -x` may still hang — that's expected;
   the value is the trace up to the hang.)

## Edge cases & notes

- **PS4 depth + `$PS4` expansion**: deferred (flat `+ `). Documented divergence.
- **Inline-assignment prefix / pure-assignment exact form**: best-effort, no
  re-expansion; documented if it diverges from bash.
- **What's traced**: simple commands (incl. builtins/functions/externals via
  `run_exec_single`). Compound keywords themselves (`for`/`if`/`while`/`case`) are
  not separate trace lines in bash either — their inner commands trace; huck
  matches by tracing each inner simple command. (bash also traces `for` iteration
  var-sets and `[[ ]]`/arith `(( ))`; those finer traces are a noted low
  divergence — out of scope.)
- **stderr**: trace uses `eprintln!` → real fd 2. (The general M-90 "builtin
  stderr ignores `2>`" applies to xtrace too — `2>/dev/null` won't suppress the
  trace; consistent with M-90, noted.)
- **Default off** ⇒ zero change to every existing test.
