# huck v154 — shell-maintained builtin variables + completion Design

**Status:** approved design, ready for implementation plan.
**Adds:** the shell-maintained "builtin" variables huck lacks (`RANDOM`, `SECONDS`, `UID`,
`EUID`, `PPID`, `BASH_VERSION`, etc.), and a completion registry so computed/special variables
tab-complete and appear in `compgen -v`. Closes the gap found while testing v153
(`$LINENO`/`$RANDOM` work as values but don't tab-complete).
**Branch (impl):** `v154-builtin-vars`.

## The variables

### Static (set once in `Shell::new`, stored in the vars table → tab-complete for free)

| Var | Value | Readonly |
|---|---|---|
| `UID` | `libc::getuid()` | yes |
| `EUID` | `libc::geteuid()` | yes |
| `PPID` | `libc::getppid()` (parent at startup) | yes |
| `GROUPS` | `libc::getgroups()` → indexed array | yes |
| `HOSTNAME` | `gethostname()` (libc) | no |
| `HOSTTYPE` | `std::env::consts::ARCH` (`x86_64`/`aarch64`/…) | no |
| `OSTYPE` | `linux-gnu` (Linux) / `darwin` (macOS), via `#[cfg(target_os)]` | no |
| `MACHTYPE` | `{ARCH}-pc-linux-gnu` / `{ARCH}-apple-darwin`, via cfg | no |
| `BASH` | path to the huck executable (`std::env::current_exe()`) | no |
| `BASH_VERSION` | bash-compatible string `5.2.0(1)-release` | no |
| `BASH_VERSINFO` | indexed array `[5, 2, 0, 1, "release", <MACHTYPE>]` | yes |
| `HUCK_VERSION` | `env!("CARGO_PKG_VERSION")` (huck's true version) | no |
| `SHLVL` | inherited `SHLVL` (numeric, else 0) + 1; exported | no |

Readonly flags match bash (`UID`/`EUID`/`PPID`/`GROUPS`/`BASH_VERSINFO` are readonly). Set via
the existing var/array machinery (`Variable { readonly, … }`; arrays like `set_pipestatus`).
A static var that is ALREADY present in the environment at startup (e.g. an exported `HOSTNAME`)
should not be clobbered destructively where bash would inherit it — but bash overwrites these
with its computed values, so huck sets them unconditionally (documented).

`BASH_VERSION` masquerade: huck reports a bash-compatible version so bash-ecosystem code
(`[ -n "$BASH_VERSION" ]`, `BASH_VERSINFO[0] >= 4`, mise/oh-my-posh/bash_completion) takes its
bash path; `HUCK_VERSION` lets scripts detect huck explicitly. `BASH` points at the huck binary.

### Dynamic (computed per read via a `lookup_var` special-case, like `LINENO`)

- `RANDOM` — pseudo-random `0..=32767` from a self-contained LCG (no new dependency). Advances
  on each read. Reseedable via `RANDOM=n` (deterministic sequence per seed; NOT bash's exact
  sequence — documented).
- `SECONDS` — whole seconds since shell start (or since a `SECONDS=n` reset).
- `EPOCHSECONDS` — `SystemTime::now()` UNIX seconds.
- `BASHPID` — `libc::getpid()` (the current process; differs from `$$` inside a forked subshell).

## Architecture

### State on `Shell`
- `random_state: std::cell::Cell<u64>` — interior mutability so `RANDOM` can advance the LCG
  inside `&self` `lookup_var`. (`Cell<u64>` is `Clone`; a `$()` COW-clone gets an independent
  sequence — acceptable.)
- `seconds_base: std::time::Instant` — set in `Shell::new`; `SECONDS` = `elapsed().as_secs()`.

### Reads — `lookup_var` special-cases
Add `RANDOM`/`SECONDS`/`EPOCHSECONDS`/`BASHPID` arms to the `lookup_var` special-parameter block
(alongside `?`/`$`/`!`/`0`/`LINENO`). `RANDOM`: advance `random_state` via the LCG (e.g.
`s = s*6364136223846793005 + 1442695040888963407; ((s >> 33) as u32 % 32768)`), `Cell::set` it,
return the value. `SECONDS`: `self.seconds_base.elapsed().as_secs().to_string()`. `EPOCHSECONDS`:
UNIX seconds. `BASHPID`: `libc::getpid()`.

### Assignment — reseed/reset (intercept at the scalar-assign chokepoint)
In `Shell::set(name, value)` (shell_state.rs:639), before the normal store:
- `name == "RANDOM"`: parse `value` as `u64` (non-numeric → ignore reseed, like bash); seed
  `random_state`; RETURN (do NOT store `RANDOM` in vars).
- `name == "SECONDS"`: parse as `u64` `n`; `seconds_base = Instant::now() - Duration::from_secs(n)`;
  RETURN (do NOT store).
Verify the inline-assignment / `apply_one_assignment` (executor.rs:4738) scalar path routes
through `Shell::set` (or add the same hook there) so `RANDOM=n cmd` and `SECONDS=0` both work.

### Completion registry
```rust
/// Special variables that are valid/known but not always present in the vars table
/// (computed dynamics + the sometimes-unset call-stack arrays). Surfaced in variable-name
/// completion and `compgen -v` so they complete like bash even when unset.
pub const DYNAMIC_SPECIAL_VARS: &[&str] =
    &["RANDOM", "SECONDS", "EPOCHSECONDS", "BASHPID", "LINENO", "BASH_SOURCE", "BASH_LINENO"];
```
New `Shell::completion_var_names() -> Vec<String>` = `var_names()` ∪ `DYNAMIC_SPECIAL_VARS`
(deduped, sorted by the caller as today). Switch the THREE completion/compgen sites to it:
`completion.rs` (the `CompletionContext::Variable` handler, ~:404/:570), `completion_spec.rs`
`Action::Variable` (~:419), and `builtins.rs` compgen `-v` (~:4708). `var_names()` and `declare
-p`/`set` listing stay UNCHANGED (so `declare` never tries to format a computed var). `FUNCNAME`
is intentionally NOT in the registry — bash omits it from top-level `compgen -v`; it completes
only when set (already true via v153's stored array).

## Files & responsibilities

| File | Change |
|------|--------|
| `src/shell_state.rs` | static-var setup in `Shell::new`; `random_state`/`seconds_base` fields; `lookup_var` arms (RANDOM/SECONDS/EPOCHSECONDS/BASHPID); `Shell::set` reseed/reset interception; `DYNAMIC_SPECIAL_VARS` + `completion_var_names()`. |
| `src/completion.rs` | Variable-completion sites → `completion_var_names()`. |
| `src/completion_spec.rs` | `Action::Variable` → `completion_var_names()`. |
| `src/builtins.rs` | compgen `-v` → `completion_var_names()`. (Confirm `apply_one_assignment` scalar path funnels RANDOM/SECONDS through `Shell::set`.) |
| `tests/builtin_vars.rs` | integration tests (behavior-checked vars). |
| `tests/scripts/builtin_vars_diff_check.sh` | bash-diff harness (byte-comparable vars). |
| `docs/bash-divergences.md` | DELETE/UPDATE the "missing builtin variables" note (if one exists; otherwise add a `[deferred]` for any var left out). |

## Behaviour matrix

- `echo $UID $EUID $PPID` → byte-identical to bash (same user; both `-c`s share the harness parent → same PPID).
- `echo $HOSTNAME $HOSTTYPE $OSTYPE $MACHTYPE` / `echo ${GROUPS[@]}` → byte-identical to bash on Linux.
- `[ -n "$BASH_VERSION" ] && echo yes` → `yes`; `echo ${BASH_VERSINFO[0]}` → `5`; `echo $HUCK_VERSION` → huck's version.
- `RANDOM` between 0 and 32767; `RANDOM=42; a=$RANDOM; RANDOM=42; b=$RANDOM` → `a == b` (reseed determinism).
- `SECONDS` → `0` immediately; `SECONDS=5; echo $SECONDS` → `5` (or slightly more).
- `EPOCHSECONDS` → a current UNIX timestamp (> 1700000000).
- `echo $BASHPID` → a pid; `( echo $BASHPID ) ` in a subshell differs from `$$`.
- `compgen -v | grep -E '^(RANDOM|LINENO|SECONDS|UID|BASH_VERSION)$'` lists them; `$RAND<TAB>` → `RANDOM`.

## Edge cases (documented divergences)

- `RANDOM`'s LCG is huck's own — only range (0–32767) and reseed-determinism are guaranteed; the
  sequence differs from bash's. So `RANDOM` is behavior-tested, never byte-compared to bash.
- `BASH_VERSION`/`BASH_VERSINFO` are a deliberate masquerade; `HUCK_VERSION` is the true identity.
- macOS `OSTYPE`/`MACHTYPE` are approximate (`darwin`, not bash's `darwinNN.0`); the byte-compare
  harness for these is Linux-only (or relaxed) — consistent with the macOS-portability memory.
- Assigning a readonly var (`UID=5`) errors, matching bash.
- `RANDOM=foo` (non-numeric) leaves the generator unchanged (bash-ish); documented.

## Testing

1. **Unit** (`shell_state.rs`): `RANDOM` in range + reseed determinism; `SECONDS` starts ~0 and a
   `SECONDS=n` reset reads ≥ n; static vars present with correct readonly flags;
   `completion_var_names()` includes the registry names; `Shell::set("RANDOM", "42")` reseeds
   (doesn't store a `RANDOM` var).
2. **Integration** (`tests/builtin_vars.rs`): behavior checks via `huck -c` for the
   non-byte-comparable vars (BASH_VERSION non-empty, BASHPID is a pid, EPOCHSECONDS recent,
   HUCK_VERSION = Cargo version, SHLVL numeric, RANDOM reseed determinism).
3. **`builtin_vars_diff_check.sh`**: byte-identical bash↔huck for the comparable set (`UID`,
   `EUID`, `PPID`, `HOSTNAME`, `HOSTTYPE`, `OSTYPE`, `MACHTYPE`, `GROUPS`). A completion check:
   `compgen -v` includes `RANDOM`/`LINENO`/`UID`/`BASH_VERSION` in BOTH shells.
4. **Regression:** full suite + all harnesses + clippy green. (Setting `SHLVL`/`OSTYPE` etc. must
   not break existing tests that read the environment.)

## Notes
- All libc calls are POSIX (macOS-portable); platform strings behind `#[cfg(target_os)]`.
- Interior mutability (`Cell`) for `RANDOM` avoids making `lookup_var` `&mut` (which would ripple
  across the whole expansion path).
- **Git safety:** implementer subagents must NOT `git checkout <sha>`; controller verifies the
  branch tip before merge. Commit trailer: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
