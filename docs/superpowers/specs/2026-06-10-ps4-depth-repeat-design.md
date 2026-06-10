# huck v131 — PS4 depth-repeat + PS4 expansion in xtrace Design

**Status:** approved design, ready for implementation plan.
**Implements:** two of the L-21 residual-(a) xtrace gaps — (1) bash repeats the
first character of `$PS4` by nesting level (`+`/`++`/`+++`); (2) bash expands
`$PS4` (prompt escapes + `$VAR`) rather than printing it literally. Reuses huck's
existing `prompt::expand_prompt` for the expansion. The remaining PS4-expansion
forms (`$(...)`, `$((...))`, `$LINENO`) are explicitly OUT of scope and logged as
a new deferred divergence for a future iteration.
**Branch (impl):** `v131-ps4-depth-repeat`.

## Background — measured behaviour (huck vs bash 5.x)

huck's xtrace (v130) emits `ps4(shell)` verbatim — the RAW `$PS4` value (default
`+ `), flat at every nesting level. bash differs in two ways:

**Depth-repeat.** bash replicates the FIRST character of the (expanded) `$PS4`
once per level of "indirection", appending the rest of PS4 unchanged:

| fragment | bash | huck (current) |
|---|---|---|
| `a=$(echo $(echo hi))` | `+++ echo hi` / `++ echo hi` / `+ a=hi` | `+ echo hi` / `+ echo hi` / `+ a=hi` |
| `f(){ echo $(echo x); }; f` | `+ f` / `++ echo x` / `+ echo x` | `+ f` / `+ echo x` / `+ echo x` |
| `eval "echo ev"` | `+ eval 'echo ev'` / `++ echo ev` | `+ eval 'echo ev'` / `+ echo ev` |

What increments the level (confirmed by probe): **command substitution** (`$()`
AND backticks) and **`eval`**. What does NOT: function calls, plain subshells
`( )`. Replication rule (probed with `PS4="XY "`): first char ×level, then
`PS4[1..]` once — level 1 `XY `, level 2 `XXY `, level 3 `XXXY `.

**PS4 expansion.** bash expands `$PS4` (prompt-string decode + parameter/command/
arith expansion), THEN replicates the first char of the EXPANDED string:

| PS4 | bash (level 1 / 2) | huck (current) |
|---|---|---|
| `\h+ ` | `devbox+ ` | `\h+ ` (literal) |
| `$VAR ` (VAR=p) | `p ` | `$VAR ` (literal) |
| `$LINENO ` | `3 ` / `33 ` | `$LINENO ` (literal) |
| `[$(echo P)] ` | `[P] ` | `[$(echo P)] ` (literal) |

huck reuses `prompt::expand_prompt` (the PS1/PS2 expander: Tier-A escapes
`\h \u \H \w \W \$ \! \# …` + `$VAR`/`${VAR}`). That covers the escape and `$VAR`
rows. It does NOT cover `$(...)`, `$((...))`, or `$LINENO` (huck has no `LINENO`
var, and `expand_prompt` does not run command/arith substitution) — see Residuals.

## Architecture

### Component 1 — depth counter (`src/shell_state.rs`)
Add `pub xtrace_depth: usize` to `Shell` (init `0` in `new()`). It is the
command-substitution/`eval` nesting depth; the PS4 first-char repeat count is
`xtrace_depth + 1` (top level = 1 char). Being a plain `usize`, `#[derive(Clone)]`
copies it by value (important: command substitution clones the shell — see C2).

### Component 2 — increment in command substitution (`src/expand.rs`, `run_substitution` ~1172)
`run_substitution` already does `let mut cloned = shell.clone();` per `$()`/
backtick. Add `cloned.xtrace_depth += 1;` before `execute_capturing(seq, &mut
cloned)`. The clone scopes the increment automatically (the parent `shell`'s depth
is untouched; nested `$()` clones-of-clones stack correctly → `+++`). No restore
needed. All command substitutions and backticks route through this single
function (call sites expand.rs:869/874/1035), so this one edit covers them all.

### Component 3 — increment around `eval` (`src/builtins.rs`, `builtin_eval` ~4698)
`builtin_eval` runs `process_line(&joined, shell, true)` on the SAME shell, so
save/restore:
```rust
let saved = shell.xtrace_depth;
shell.xtrace_depth += 1;
let r = crate::shell::process_line(&joined, shell, true);
shell.xtrace_depth = saved;
r
```
The `+ eval '…'` line itself is emitted by `run_exec_single` BEFORE `builtin_eval`
runs (trace-before-dispatch), so it prints at the OUTER depth; the eval body then
traces at depth+1. Nested `eval "eval …"` works via the per-frame `saved`.

### Component 4 — `ps4(shell)` rewrite (`src/executor.rs` ~2805)
```rust
fn ps4(shell: &Shell) -> String {
    let raw = shell.lookup_var("PS4").unwrap_or_else(|| "+ ".to_string());
    let expanded = crate::prompt::expand_prompt(&raw, shell);
    let mut chars = expanded.chars();
    let Some(first) = chars.next() else { return String::new(); };
    let rest: String = chars.collect();
    let level = shell.xtrace_depth + 1;
    let mut out = String::with_capacity(level + rest.len());
    for _ in 0..level { out.push(first); }
    out.push_str(&rest);
    out
}
```
Order matches bash: EXPAND `$PS4` first, then repeat the first char of the
EXPANDED value. Empty expanded PS4 → empty prefix (no repeat). All four trace
emit sites (run_exec_single command line + inline-assignment lines, the Assign
arm, the external-stage trace) already call `ps4(shell)`, so they inherit the new
behaviour with no further change.

## Correctness / ordering notes
- **Ordering is already correct.** Command substitutions execute during EXPANSION
  (inside `resolve`/`apply_one_assignment`), which runs BEFORE the outer command's
  trace line is emitted — so inner (`+++`) lines print before the outer (`+`)
  line, matching bash. No reordering work needed; only the prefix changes.
- **Subshells/functions unaffected.** `fork_and_run_in_subshell` (subshell bodies,
  pipeline stages) does NOT touch `xtrace_depth`, so `( … )` and function bodies
  trace at the enclosing depth — matching bash.
- **No restore bug.** C2 mutates only the clone; C3 saves/restores; the common
  (non-substitution, non-eval) path never changes depth.

## Scope, residuals & must-not-regress
- **MUST NOT regress** the default `PS4='+ '` common case: level 1 → `+ ` (one
  `+`), identical to today for any non-nested command. Verified by the existing
  v103/v130 set_x tests (they use default PS4 at depth 0) — they must stay green.
- **Residual (NEW deferred divergence, to be logged):** `$(...)`, `$((...))`, and
  `$LINENO` inside `$PS4` are NOT expanded (huck's `expand_prompt` handles escapes
  + `$VAR` only, and huck has no `LINENO`). bash expands all three. The user wants
  this tackled later → log a new `[deferred]` entry (e.g. **L-29**, low) capturing
  command-sub/arith/LINENO-in-PS4 (and the broader "huck prompt expansion lacks
  `$(...)`/`$((...))`" gap that also affects PS1/PS2). Do NOT fold it into L-21.
- **L-21 item (a)** ("Flat `$PS4`: no per-level char repeat AND no escape/`$VAR`
  expansion") is now RESOLVED for depth-repeat + escape/`$VAR` expansion → REMOVE
  item (a) from L-21 entirely and RELETTER the current (b)→(a), (c)→(b), (d)→(c),
  (e)→(b)… i.e. shift the remaining four items up one letter so they read (a)–(d).
  The remaining sub-gap of old (a) (command-sub/arith/LINENO not expanded in PS4)
  becomes the NEW L-29 entry. Update L-21's Status line (drop the "Flat `$PS4`"
  framing; note "v131 — PS4 depth-repeat + `$VAR`/escape expansion now match
  bash"), the **bash** line (drop "depth-repeated `$PS4` with `$VAR`/escape
  expansion"), and the **Why intentional** line accordingly. After removal the
  prose's "four residual differences" count is correct (the four remaining items).

## Files & responsibilities

| File | Change |
|------|--------|
| `src/shell_state.rs` | Add `pub xtrace_depth: usize`; init `0` in `new()`. |
| `src/expand.rs` | `run_substitution`: `cloned.xtrace_depth += 1` before `execute_capturing`. |
| `src/builtins.rs` | `builtin_eval`: save/increment/restore `xtrace_depth` around `process_line`. |
| `src/executor.rs` | `ps4()`: expand via `expand_prompt`, then repeat first char by `xtrace_depth + 1`. |
| `tests/ps4_depth_repeat_integration.rs` (NEW) | Exact-byte trace assertions. |
| `tests/scripts/ps4_depth_repeat_diff_check.sh` (NEW) | Bash-diff harness (depth + supported-expansion cases). |
| `docs/bash-divergences.md` | Remove L-21 item (a) + reletter remaining to (a)-(d); ADD a new Tier-4 `[deferred]` **L-29** (command-sub/arith/`$LINENO` not expanded in PS4 / prompts — `expand_prompt` does escapes + `$VAR` only; affects PS1/PS2 too); increment the Tier-4 count 24→25. |

## Testing

1. **Integration `#[test]`s** (`tests/ps4_depth_repeat_integration.rs`) — run a
   fragment with `set -x`, capture STDERR, assert the exact trace line(s):
   - `a=$(echo $(echo hi))` → stderr has `+++ echo hi`, `++ echo hi`, `+ a=hi`
   - `f(){ echo $(echo x); }; f` → has `+ f`, `++ echo x`, `+ echo x`
   - `eval "echo ev"` → has `+ eval 'echo ev'` and `++ echo ev`
   - `( echo s )` → `+ echo s` (depth 1; subshell adds NO depth)
   - function call no depth: `g(){ echo y; }; f(){ g; }; f` → all `+ …`
   - custom first char: `PS4='> '` + `a=$(echo hi)` → `++ echo hi`, `+ a=hi`
   - multi-char PS4: `PS4='XY '` + `a=$(echo hi)` → `XXY echo hi`, `XY a=hi`
   - `$VAR` in PS4: `P=Q; PS4='$P '` + `echo z` → `Q echo z`
   - default-PS4 no-regress: `echo hi` → `+ echo hi`
2. **Bash-diff harness** `tests/scripts/ps4_depth_repeat_diff_check.sh` — stderr-
   only (`2>&1 >/dev/null`), exact bytes, for the depth cases + the PS4-expansion
   cases huck SUPPORTS (default, custom first char, `XY `, `$VAR`, `\h`). Do NOT
   include `$(...)`/`$((...))`/`$LINENO` fragments (known residual — they would
   diverge; that's the L-29 gap, not a harness failure to mask).
3. **Full regression:** whole suite + all existing harnesses green (esp. the v103
   `set_x_integration` + v130 `setx_trace_fidelity_*` — default-PS4 depth-0 output
   is unchanged); clippy clean.

## Edge cases & notes
- `expand_prompt` takes `&Shell` (immutable) and does no command substitution, so
  calling it per trace line is cheap and side-effect-free (no double-eval risk).
- A PS4 whose expanded first char is multibyte is handled (`chars()`-based).
- `xtrace_depth` is reset implicitly per command-sub clone; a long-running shell
  never accumulates depth at the top level (only nested contexts raise it).
- No interaction with `set -o`/options beyond reading `shell_options.xtrace` at the
  emit sites (unchanged).
