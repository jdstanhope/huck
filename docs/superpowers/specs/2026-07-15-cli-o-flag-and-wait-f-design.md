# v300 — `-o`/`+o` command-line flag (#159) + `wait -f` (#160) — Design

**Issues:** [#159](https://github.com/jdstanhope/huck/issues/159) (`huck -o <option>` rejected),
[#160](https://github.com/jdstanhope/huck/issues/160) (`wait -f` unsupported). A batch of two small,
independent option-parsing gaps surfaced by the `jobs` bash-suite category. Either could ship alone.

## Section 1 — #159: `-o` / `+o` command-line flag

**Behavior (bash 5.2.21, verified):**
```
bash -o posix -c 'echo ok'     # enables posix, runs -> ok
bash +o history -c 'echo ok'   # disables history, runs -> ok
bash -o badname -c 'echo x'     # bash: badname: invalid option name  (rc 2)
```
`-o` / `+o` consume the **next token** as the option name verbatim (even if it looks like a flag).
huck currently rejects `-o` at `parse_cli` with `unrecognized option: -o`, breaking `huck -o posix -c …`
(and the `jobs.tests` posix sub-test).

**Mechanism.** `parse_cli` (`crates/huck-engine/src/shell.rs:57`) is a per-arg matcher with no `-o`
arm, so `-o` falls into the generic `s.starts_with('-')` → `unrecognized option`. The engine already
has the option table: `option_set(shell, name, value) -> Result<(), OptSetErr>` (`builtins.rs:6149`)
accepts every bash 5.2 option name (`OptSetErr::Unknown` on a bad name) and is what the `set -o`
builtin uses — so no option table needs duplicating.

**Design.**
1. **`parse_cli` collects, does not apply.** Add `-o` and `+o` arms that consume the next token as an
   option name and push `(name, enable)` (enable = true for `-o`, false for `+o`) onto a new ordered
   field `o_options: Vec<(String, bool)>` on `CliOptions` (default empty). If the name token is
   missing (`-o` is the last arg), return `Err("-o: option requires an argument")` (bash's bare-`-o`
   "list current options" behavior is out of scope — documented non-goal). `parse_cli` does not
   validate names (it has no `Shell`); validation happens at apply.
2. **A thin public engine wrapper.** Add `pub fn set_o_option_by_name(shell: &mut Shell, name: &str,
   enable: bool) -> Result<(), ()>` (in `builtins.rs`, re-exported so huck-cli can call it) that calls
   `option_set` and maps `Ok(())` → `Ok(())`, `Err(OptSetErr::Unknown)` → `Err(())`. This is the only
   new public surface; it does not duplicate the option table.
3. **`repl.rs` applies in order at startup.** Immediately after the existing `posix`/`noexec`
   application block (`crates/huck-cli/src/repl.rs:~95-107`, before the `match opts.mode` dispatch),
   iterate `opts.o_options` in order and call `set_o_option_by_name`. On `Err(())`, emit
   `huck: <name>: invalid option name` to stderr and exit with status 2 (bash's rc for a bad `-o`
   name). Applying here — before any program/interactive dispatch — means the option governs the
   whole session, and reusing `option_set` makes `$-`, `posix`, `monitor`, etc. take effect for free.

**Ordering & interaction.** `o_options` is applied in argv order after `posix`/`noexec`, so
`-o posix` and `+o posix` compose left-to-right and a later CLI `-o` wins, matching bash. `-o posix`
sets `shell_options.posix` exactly as `--posix` does (via `option_set("posix", true)`); the two paths
are equivalent (both just set the flag; `startup_posix`'s `sh`/`POSIXLY_CORRECT` triggers are
unaffected).

## Section 2 — #160: `wait -f`

**Behavior (verified):** bash's usage is `wait: usage: wait [-fn] [-p var] [id ...]`; `wait -f %1`
returns 0; `jobs6.sub` uses plain `wait -f %1` (no combined `-nf`). huck currently rejects `-f`:
`wait: -f: invalid option` + the old usage string (which also omits `-f`).

**Mechanism & design.** `-f` means "wait for each ID to fully terminate rather than returning when it
merely changes status" (bash monitor-mode semantics). huck's `wait` has **no return-on-stop path** —
it already blocks to termination — so `-f` is behaviorally the default. The fix is purely to stop
rejecting it:
1. In `parse_wait_args` (`builtins.rs:4312`), add `"-f" => { idx += 1; }` (accept, no state; huck's
   default wait already conforms). Place it beside the `-n` arm.
2. Update the usage string (the `wait: usage: …` line at both emission sites — the invalid-option arm
   and any other) from `wait [-n] [-p var] [id ...]` to `wait [-fn] [-p var] [id ...]`.

Combined single-arg short flags (`-nf`) are NOT split (huck's `wait` parser is one-flag-per-arg, and
`jobs6.sub` only uses plain `-f`); a `-nf` would still error, matching current behavior — a documented
non-goal, tracked separately if it ever surfaces.

## Error handling

- #159: a missing `-o` argument → `Err` from `parse_cli` (printed as huck's CLI error, rc per the
  existing CLI-error path). A bad option name → `huck: <name>: invalid option name`, exit 2, at apply
  time in `repl.rs`. No panic paths; `set_o_option_by_name` is total.
- #160: unchanged error handling for other bad flags (`wait -Z` still errors with the *updated* usage
  string). `-f` no longer errors.

## Testing

- **New `tests/scripts/cli_o_flag_diff_check.sh`** (#159): compare huck vs bash for
  `-o posix -c 'echo $-'`-style state (normalize the shell-name prologue), `+o` disabling a default-on
  option, `-o errexit -c 'false; echo after'` (behavioral: errexit takes effect), and the bad-name
  case `-o badname -c 'echo x'` (message + rc 2, prologue-normalized). Plus a `parse_cli` unit test in
  `shell.rs` asserting `-o`/`+o` populate `o_options` and a missing-arg errors.
- **New `tests/scripts/wait_f_diff_check.sh`** (or extend existing wait coverage) (#160):
  `wait -f %1` on a short bg job (rc 0), `wait -Z` (usage line now shows `[-fn]`), and a regression
  `wait -n -p v` (still works). Compare rc + stderr byte-identically (prologue-normalized).
- **Regression:** engine lib suite stays green; the full diff sweep stays 0-failed on both binaries.

## Scope / non-goals

- `-o`/`+o` require an argument (bash's bare-`-o`-lists-options behavior is out of scope).
- `-f` is accept-and-conform — no new return-on-stop `wait` semantics (huck has none; a real
  monitor-mode return-on-stop behavior would be separate work).
- No combined single-arg `wait` short-flag splitting (`-nf`).
- The merged PR closes #159 and #160.
