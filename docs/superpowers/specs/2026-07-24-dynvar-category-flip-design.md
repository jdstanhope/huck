# v332 — Flip the `dynvar` bash-suite category to PASS

Issue: [#286](https://github.com/jdstanhope/huck/issues/286) — `dynvar` category:
`BASH_ARGV0`, `EPOCHREALTIME`, and `BASH_COMMAND` unimplemented.

## Problem

The `dynvar` bash-suite category is a near-miss (diff 15 lines). It exercises
three dynamic (computed) variables that huck does not provide; implementing all
three takes the category to **0-diff → PASS** (Summary PASS 20→21, FAIL 62→61).
Each was verified byte-identical to bash 5.2.21 and the three together flip the
category (prototype-measured 15 → 4 → 0).

### 1. `BASH_ARGV0` — read/write dynamic variable tied to `$0`

Reading `$BASH_ARGV0` returns `$0`; **assigning** it sets `$0`. huck has no
handling, so `BASH_ARGV0=hello` leaves `$0` unchanged.

```console
$ bash -c 'BASH_ARGV0=hello; echo "$0 $BASH_ARGV0"'
hello hello
```
The test also assigns it inside a function (`BASH_ARGV0="$1"`), which likewise
sets `$0`.

### 2. `EPOCHREALTIME` — current time as `SECONDS.MICROSECONDS`

A sibling of the already-implemented `EPOCHSECONDS`, with a 6-digit microsecond
fraction. huck expands it to empty, so the test's `(( … ))` arithmetic over its
parsed seconds/microseconds fields is a syntax error (`operand expected`).

```console
$ bash -c 'echo $EPOCHREALTIME'
1753318000.123456
```
The test splits it on `.` and does integer arithmetic on the microsecond field
(`(( $dmsec < 1000000 ))`), so the format must be exactly `<secs>.<6-digit-micros>`.

### 3. `BASH_COMMAND` — the currently-executing command's source text

The source text of the simple command currently executing (or about to, as seen
by a DEBUG trap). Completely unimplemented in huck (expands to empty). bash sets
it to the raw command as written, before expansion.

```console
$ bash -c 'echo $BASH_COMMAND'
echo $BASH_COMMAND
```
Relevant beyond `dynvar`: the v329 spec flagged `$BASH_COMMAND` as unimplemented
for the DEBUG-trap arc, so this also benefits `dbg-support`.

## Design

Three small, self-contained additions. All in `crates/huck-engine/src/shell_state.rs`
except the `BASH_COMMAND` stamp (executor). Exact prototype code below; all three
are prototype-verified byte-identical to bash and jointly flip `dynvar` to PASS
with `dbg-support2` still PASS.

### 1. `EPOCHREALTIME` + `BASH_ARGV0`/`BASH_COMMAND` reads (`lookup_var`)

In `lookup_var`'s special-name match, next to the `EPOCHSECONDS` arm:

```rust
"EPOCHREALTIME" => {
    return Some(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| format!("{}.{:06}", d.as_secs(), d.subsec_micros()))
            .unwrap_or_else(|_| "0.000000".to_string()),
    );
}
"BASH_ARGV0" => return Some(self.shell_argv0.clone()),
"BASH_COMMAND" => return Some(self.current_command.clone()),
```

### 2. `BASH_ARGV0` write (`reseed_special_on_assign`)

`reseed_special_on_assign` is the hook (returns `true` → the name is computed,
not stored). Add:

```rust
// Assigning BASH_ARGV0 sets $0 (shell_argv0); it is computed in
// lookup_var, never stored as an ordinary var.
"BASH_ARGV0" => {
    self.shell_argv0 = value.to_string();
    true
}
```

### 3. `BASH_COMMAND` — new `current_command` field + executor stamp

- Add `pub current_command: String` to `Shell` (init `String::new()`).
- In `run_single` (executor), before the command dispatch / DEBUG fire, stamp it
  from the command's rendered source text (the existing `render_job_simple`,
  which reconstructs `inline_assignments + program + args + redirects`):
  ```rust
  // $BASH_COMMAND: the source text of the command about to run — stamped
  // before the DEBUG trap fires (bash sets it at the same point, so a DEBUG
  // action reading $BASH_COMMAND sees the command that triggered it).
  shell.current_command = render_job_simple(cmd);
  ```
  `run_single` covers both `SimpleCommand::Exec` and `SimpleCommand::Assign`,
  and precedes the DEBUG fire in each, so a DEBUG action sees the right value —
  matching bash, and confirmed non-regressive for `dbg-support2`.

### 4. Registration

Add `"EPOCHREALTIME"`, `"BASH_ARGV0"`, `"BASH_COMMAND"` to `DYNAMIC_SPECIAL_VARS`
(so they complete via `compgen -v` / `$<TAB>` like the other computed dynamics).

## Testing

Gate = bash 5.2.21 fidelity + `dynvar` at 0 diff.

1. **Bash-diff harness** `tests/scripts/dynvar_vars_diff_check.sh` (model on an
   existing `-c` harness), byte-identical incl. stderr + exit:
   - `BASH_ARGV0=hello; echo "$0 $BASH_ARGV0"` → `hello hello`; and a function
     `setarg0(){ BASH_ARGV0="$1"; }; setarg0 arg0; echo $0` → `arg0`.
   - `echo $EPOCHREALTIME` format is `<digits>.<6 digits>` (assert the SHAPE, not
     the value — it changes; e.g. `[[ $EPOCHREALTIME =~ ^[0-9]+\.[0-9]{6}$ ]]`),
     and `(( ${EPOCHREALTIME%.*} > 0 ))` succeeds.
   - `echo $BASH_COMMAND` → `echo $BASH_COMMAND`; inside a function and after an
     assignment (`x=1; echo $BASH_COMMAND` → `echo $BASH_COMMAND`).
   - a DEBUG trap reading `$BASH_COMMAND` sees the command that triggered it
     (`set -T; trap 'echo D:$BASH_COMMAND' DEBUG; true` shape).
2. **`dynvar` category** flips: `HUCK_BASH_TEST_CATEGORY=dynvar` → PASS, 0 diff
   (was 15 lines; prototype-measured 15 → 4 after vars 1-2 → 0 with BASH_COMMAND).
3. **Regression**: huck-engine lib green; `dbg-support2` stays PASS (DEBUG actions
   now read a non-empty `$BASH_COMMAND` — must not regress); the DEBUG / xtrace /
   job-display harnesses stay green (BASH_COMMAND stamps in the hot `run_single`
   path via `render_job_simple` — the same renderer the job display uses); full
   `run_diff_checks.sh` sweep green; previously-flipped categories
   (parser/rhs-exp/procsub/posix2/dbg-support2) stay PASS; the `-p huck`
   var/trace/trap integration bins green.

Per repo constraints: build with `cargo build -p huck`; per-crate tests
single-threaded; NEVER `cargo test --workspace`; guard sweeps with
`ulimit -v 1500000` + `timeout`; run the `-p huck` integration bins
single-threaded before push; NO GPL bash text.

## Scope

**In scope.** The three dynamic variables (reads + BASH_ARGV0 write + the
`current_command` field/stamp); registration; the harness; the category flip;
regressions.

**Out of scope.** The #48 computed-var edge cases (`set`/`declare -p` listing,
`[[ -v ]]`, assignment-shadow, inline-assignment scoping) — they apply equally to
these three and stay tracked in #48. `BASH_COMMAND` for compound-command /
pipeline-stage granularity beyond what `dynvar` and `dbg-support2` exercise (the
`run_single` stamp covers simple commands, which is what both need); broader
per-construct `BASH_COMMAND` stamping is a follow-up if a later category needs it.

## Documentation

- Removes a divergence (no new intentional one). #286 auto-closes via the PR
  (`Closes #286`). `docs/bash-divergences.md` unchanged.
- Update `docs/bash-test-suite-baseline.md` ("Updated by v332": `dynvar` PASS,
  Summary PASS 20→21, FAIL 62→61); record the iteration in
  `project_huck_iterations.md` + `MEMORY.md`.
