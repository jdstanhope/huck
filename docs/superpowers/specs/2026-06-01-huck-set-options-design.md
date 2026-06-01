# huck v69 — `set -e`/`-u`/`-o` (M-08 cont.)

## Goal

Finish M-08's deferred half: ship the `set` option flags
`-e` (errexit) and `-u` (nounset), the `-o`/`+o` long-form
syntax, and the `$-` special parameter. Defer `-x` (xtrace),
`pipefail`, and other less-common options to later iterations.

After v69:

- `set -e` / `set -o errexit` — enable errexit. Subsequent
  simple commands that exit non-zero cause the shell to exit
  immediately, EXCEPT when the command is in a context huck
  already tracks as `err_suppressed_depth > 0` (if/elif/while/
  until conditions, LHS of `&&`/`||`, and `!`-negated
  pipelines).
- `set +e` / `set +o errexit` — disable.
- `set -u` / `set -o nounset` — enable. Subsequent expansion
  of an unset variable as bare `$VAR` (no `${VAR:-…}`-style
  modifier) becomes a fatal parameter-expansion error and the
  shell exits in non-interactive mode (or returns to prompt
  in interactive, per the existing fatal-PE path).
- `set +u` / `set +o nounset` — disable.
- `set -o` (no args) — list all known options and their
  current state (e.g., `errexit         off`).
- `set +o` (no args) — list in re-input form (e.g.,
  `set +o errexit\nset +o nounset`).
- `$-` — current short-flag letters (`e`, `u`, plus `i` when
  `is_interactive`).
- Cluster: `set -eu` enables both.
- `--` ends flag parsing; the rest are positional args
  (existing v50 behavior).

This closes the deferred half of **M-08**.

## Scope decisions (locked via AskUserQuestion)

**Just `-e`, `-u`, `-o` long-form** (per the user's explicit
list). Defer `-x` (xtrace), `pipefail`, `-n` (noexec), `-f`
(noglob), `-a` (allexport), `-C` (noclobber), `-b` (notify),
`-v` (verbose), `-h`, monitor, etc. Each is a separate future
iteration.

## Out of scope (deferred)

- `-x` (xtrace) — print each simple command to stderr before
  running, prefixed by PS4.
- `pipefail` — pipeline exit status = last non-zero stage's
  status.
- All other bash set options.

## Architecture

### `ShellOptions` struct (`src/shell_state.rs`)

```rust
/// Persistent shell-option state controlled by `set -X`/`set -o NAME`.
/// New options are added by extending this struct + the
/// option-name table in `src/builtins.rs`.
#[derive(Debug, Clone, Default)]
pub struct ShellOptions {
    pub errexit: bool,
    pub nounset: bool,
    // Future expansion: xtrace, pipefail, noexec, noglob, allexport, etc.
}
```

New field on `Shell`:

```rust
pub shell_options: ShellOptions,
```

Init via `ShellOptions::default()` in `Shell::new`.

### Option-name registry (`src/builtins.rs`)

Centralized list of known options so both `-o`/`+o` and the
listing forms stay in sync:

```rust
struct OptionInfo {
    name: &'static str,
    short: Option<char>,
}

const SHELL_OPTIONS: &[OptionInfo] = &[
    OptionInfo { name: "errexit", short: Some('e') },
    OptionInfo { name: "nounset", short: Some('u') },
];

fn option_get(shell: &Shell, name: &str) -> Option<bool> {
    match name {
        "errexit" => Some(shell.shell_options.errexit),
        "nounset" => Some(shell.shell_options.nounset),
        _ => None,
    }
}

fn option_set(shell: &mut Shell, name: &str, value: bool) -> Result<(), ()> {
    match name {
        "errexit" => { shell.shell_options.errexit = value; Ok(()) }
        "nounset" => { shell.shell_options.nounset = value; Ok(()) }
        _ => Err(()),
    }
}
```

Adding a future option (e.g., `xtrace`) means: add the field,
extend `SHELL_OPTIONS`, add cases to `option_get`/`option_set`,
and wire the behavior.

### `builtin_set` extension

Currently in v50, `builtin_set` rejects `-e`/`-u`/`-x`/`-o`
with "not yet supported in this version". Replace that
rejection with real handling.

```rust
fn builtin_set(args, out, shell) -> ExecOutcome {
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" { i += 1; break; }

        if arg == "-o" {
            i += 1;
            if i >= args.len() {
                print_options_table(out, shell);
                return ExecOutcome::Continue(0);
            }
            if option_set(shell, &args[i], true).is_err() {
                eprintln!(
                    "huck: set: -o: invalid option name: {}", args[i]
                );
                return ExecOutcome::Continue(2);
            }
            i += 1;
            continue;
        }
        if arg == "+o" {
            i += 1;
            if i >= args.len() {
                print_options_reinput(out, shell);
                return ExecOutcome::Continue(0);
            }
            if option_set(shell, &args[i], false).is_err() {
                eprintln!(
                    "huck: set: +o: invalid option name: {}", args[i]
                );
                return ExecOutcome::Continue(2);
            }
            i += 1;
            continue;
        }

        if arg.starts_with('-') && arg.len() >= 2 {
            // Cluster: -eu sets e and u.
            for &c in &arg.as_bytes()[1..] {
                match c {
                    b'e' => shell.shell_options.errexit = true,
                    b'u' => shell.shell_options.nounset = true,
                    // Existing v50 rejected -x and others. Keep
                    // rejecting un-shipped flags for now.
                    other => {
                        eprintln!(
                            "huck: set: -{}: not yet supported in this version",
                            other as char
                        );
                        return ExecOutcome::Continue(2);
                    }
                }
            }
            i += 1;
            continue;
        }
        if arg.starts_with('+') && arg.len() >= 2 {
            for &c in &arg.as_bytes()[1..] {
                match c {
                    b'e' => shell.shell_options.errexit = false,
                    b'u' => shell.shell_options.nounset = false,
                    other => {
                        eprintln!(
                            "huck: set: +{}: not yet supported in this version",
                            other as char
                        );
                        return ExecOutcome::Continue(2);
                    }
                }
            }
            i += 1;
            continue;
        }
        // Non-flag arg — break to positional-replacement (existing v50 path).
        break;
    }
    // Remainder: positional args replacement (existing v50 logic preserved
    // from index `i` onward).
    let rest: Vec<String> = args[i..].to_vec();
    shell.positional_args = rest;
    ExecOutcome::Continue(0)
}
```

Note: bare `set` (no args) currently lists all variables per
v50's M-65 implementation. That path is reached when `i == 0`
and no flags were seen. Keep it (use the existing v50 branching
or restructure to detect "no args at all" up front).

### Listing format

`set -o` (no args), one option per line:

```
errexit         off
nounset         off
```

(Two columns: name padded to ~16 chars, then `on`/`off`.)

`set +o` (no args), re-input form:

```
set +o errexit
set +o nounset
```

Or `set -o NAME` when an option is currently on.

### Errexit wire-in

The errexit check fires AFTER each simple command's
completion. Reuse `Shell::err_suppressed_depth` (v36) as the
gate — same suppression contexts apply (if/elif/while/until
conditions, `&&`/`||` LHS, `!`-negated pipelines).

New helper in `src/executor.rs`:

```rust
/// If `errexit` is on, the last simple command exited non-zero,
/// and we're not in an err-suppressed context, return an `Exit`
/// outcome so the shell terminates. Otherwise return None.
fn maybe_errexit(shell: &Shell, status: i32) -> Option<ExecOutcome> {
    if shell.shell_options.errexit
        && shell.err_suppressed_depth == 0
        && status != 0
    {
        Some(ExecOutcome::Exit(status))
    } else {
        None
    }
}
```

Call this immediately after `set_last_status` in the executor's
command-completion paths. The natural locations:
- `run_command`'s SimpleCommand arms (after the command runs).
- After each statement in a Sequence/AndOr body (so a failing
  command in `cmd1; cmd2; cmd3` exits before cmd2).
- After function-body commands.

The implementer should grep for `fire_err_trap` (v36's ERR
trap callsite) — every place that fires the ERR trap is also a
natural place for the errexit check. They can ride together.

Concrete: in `run_command`, after the existing
`shell.set_last_status(status)` + ERR-trap fire, add:

```rust
if let Some(out) = maybe_errexit(shell, status) {
    return out;
}
```

### Nounset wire-in

Expanding a BARE `$VAR` (no modifier) when nounset is on and
VAR is unset must:
1. Print `huck: VAR: unbound variable` to stderr.
2. Set the fatal-PE-error sentinel so the surrounding command
   aborts and (in non-interactive) the shell exits.

Find the bare-`$VAR` expansion path in `src/expand.rs` (likely
in `expand_word_internal` or wherever `lookup_var` is called
directly for plain variable substitution). Add a guard:

```rust
let value = match shell.lookup_var(name) {
    Some(v) => v,
    None => {
        if shell.shell_options.nounset {
            eprintln!("huck: {name}: unbound variable");
            shell.pending_fatal_pe_error = Some(1);
            return /* propagation pattern matching the existing fatal-PE flow */;
        }
        String::new()
    }
};
```

Modifier expansions like `${VAR:-default}`, `${VAR:=default}`,
`${VAR:?msg}`, `${VAR:+alt}`, `${VAR#pat}`, etc. are exempt
because they have explicit fallback semantics for unset vars.
This matches bash.

Confirm by re-reading `src/param_expansion.rs::expand_modifier`
— it already short-circuits the "unset" path for the
default-family modifiers via `condition_is_null`. No change
needed there.

For `${VAR}` (braced form with NO modifier), the path goes
through whatever handles "plain braced variable expansion".
That's also a bare-lookup site that should respect nounset.

### `$-` special parameter

In `Shell::lookup_var`, add `"-"` to the special-parameter
match:

```rust
match name {
    "0" => return Some(...),
    "$" => return Some(...),
    "!" => return Some(...),
    "-" => return Some(self.dollar_dash_value()),
    _ => {}
}
```

New method on Shell:

```rust
pub fn dollar_dash_value(&self) -> String {
    let mut out = String::new();
    if self.shell_options.errexit { out.push('e'); }
    if self.is_interactive { out.push('i'); }
    if self.shell_options.nounset { out.push('u'); }
    // Future: x (xtrace), h (hashing), etc.
    out
}
```

Order: alphabetical to match bash output convention.

## Behavior table

| Input | Behavior |
|---|---|
| `set -e; false; echo X` | shell exits with code 1; "X" NOT printed |
| `set -e; if false; then :; fi; echo X` | "X" printed (failure in if-condition is exempt) |
| `set -e; false \|\| true; echo X` | "X" printed (failure on LHS of `\|\|` is exempt) |
| `set -e; ! false; echo X` | "X" printed (failure under `!`-negation is exempt) |
| `set -e; f() { false; echo a; }; f; echo b` | exits after `false` in f; neither "a" nor "b" printed |
| `set +e; false; echo X` | "X" printed (errexit off) |
| `set -u; echo $UNSET` | "huck: UNSET: unbound variable" + fatal PE error |
| `set -u; echo "${UNSET:-default}"` | "default" printed (modifier exempts) |
| `set -u; echo "${UNSET}"` | unbound-variable error (braced no-modifier is NOT exempt) |
| `set -eu` (cluster) | both flags enabled |
| `set -o errexit` | enable errexit (long form) |
| `set +o errexit` | disable |
| `set -o` (no name) | list options + on/off |
| `set +o` (no name) | list options in `set +o NAME` re-input form |
| `set -o nope` | "set: -o: invalid option name: nope" + exit 2 |
| `echo $-` (no flags) | "i" if interactive, else empty |
| `set -e; echo $-` | "ei" (interactive) or "e" |
| `set -eu; echo $-` | "eiu" or "eu" |
| `set --` | clears positional args (existing v50 behavior) |
| `set -e --` | enable errexit AND clear positional args |

## Test plan

### Unit tests in `src/builtins.rs::mod set_options_tests` (10 tests)

1. `set_e_enables_errexit` — `run_builtin("set", &["-e"], ...)` → `shell.shell_options.errexit == true`.
2. `set_plus_e_disables` — enable then `+e` → false.
3. `set_o_errexit_long_form` — `set -o errexit` → `errexit == true`.
4. `set_plus_o_errexit_disables` — `set +o errexit` → false.
5. `set_dollar_dash_reflects_flags` — after `set -e`, `shell.lookup_var("-")` contains 'e'; after `set -u` also has 'u'. Order alphabetical (`e` < `i` < `u`).
6. `set_invalid_o_name_errors` — `set -o nope` → exit 2.
7. `set_o_listing_shows_state` — `set -o` (no args) → output has "errexit" line and "nounset" line.
8. `set_plus_o_listing_reinput_form` — `set +o` (no args) → output has "set +o errexit" and "set +o nounset" lines (both off by default).
9. `set_eu_cluster` — `set -eu` enables both flags in one call.
10. `set_dash_dash_resets_positional` — `set -e -- a b c` → errexit on AND positional_args == ["a", "b", "c"].

### Unit tests in `src/shell_state.rs::tests` (none needed; just struct + helper)

### Integration tests in `tests/set_options_integration.rs` (9 tests)

1. `set_e_exits_on_failure` — `set -e; false; echo X; exit` → exit 1, no "X".
2. `set_e_exempt_in_if` — `set -e; if false; then :; fi; echo X; exit` → "X" printed.
3. `set_e_exempt_in_or_chain` — `set -e; false || true; echo X; exit` → "X" printed.
4. `set_e_in_function_exits` — function with failing first command → exits before reaching subsequent commands.
5. `set_u_unset_errors` — `set -u; echo $XYZ_UNSET; echo X; exit` → non-zero exit, no "X" in non-interactive (script-mode).
6. `set_u_default_modifier_ok` — `set -u; echo "${XYZ_UNSET:-default}"; echo X; exit` → "default" + "X" both printed.
7. `set_o_errexit_works_as_dash_e` — `set -o errexit; false; echo X; exit` → exit 1, no "X".
8. `dollar_dash_includes_e_after_set_e` — `set -e; echo "[$-]"; exit` → output contains `[e`.
9. `set_minus_o_lists_options` — `set -o; exit` → stdout has `errexit` and `nounset` lines.

### Smoke

`cargo test --all-targets` green (PTY flake tolerated).

## Implementation tasks

1. **Foundation + builtin_set extension + errexit + nounset + $- + 10 unit tests** — modify `src/shell_state.rs` (add ShellOptions + field + Default + dollar_dash_value method + lookup_var "-" case); modify `src/builtins.rs` (replace v50's rejection with real flag/option handling; add OptionInfo registry; add `print_options_table` + `print_options_reinput`; `mod set_options_tests`); modify `src/executor.rs` (`maybe_errexit` helper + call after each command completion); modify `src/expand.rs` (nounset check on bare-$VAR lookup).

2. **Integration tests** — `tests/set_options_integration.rs` with 9 scenarios.

3. **Docs** — update M-08 from `[deferred]` to `[fixed v69 partial]` (still defers -x/pipefail/etc.); change-log; README v69 row.

## Acceptance criteria

- 10 unit tests pass.
- 9 integration tests pass.
- `cargo test --all-targets` green (PTY flake tolerated).
- `cargo clippy --all-targets -- -D warnings` clean.
- `set -e` exits on simple-command failure; exemptions match
  v36's ERR-trap suppression rules.
- `set -u` errors on bare-$VAR expansion of unset names;
  default-family modifiers remain exempt.
- `set -o`/`+o` with NAME works for `errexit` and `nounset`;
  unknown NAME → exit 2.
- `set -o` and `set +o` without NAME list current state.
- `$-` reflects current flags.
- M-08 doc entry updated to note v69 ships -e/-u/-o partial;
  -x/pipefail/etc. remain deferred.
