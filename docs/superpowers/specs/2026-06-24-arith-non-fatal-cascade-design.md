# v215: Arith expansion errors are non-fatal in file mode

## Goal

Close the cascade that v214's bash test-suite sweep surfaced in the `arith`
category: huck halts script execution on the FIRST arithmetic error,
producing 2 lines of output where bash produces 264. Two fixes ship together:

1. `set +o posix` (and `-o posix`) becomes a no-op accept instead of an
   "unimplemented" error.
2. Arithmetic expansion errors (e.g. `$((1/0))`, `$((x+=2))` where `x` is
   a non-variable string) print the error to stderr but no longer set the
   fatal-PE flag that halts subsequent statements — matching bash's
   script-file behavior.

After the fix, `arith.tests` runs end-to-end and the remaining diff exposes
the actual semantic divergences for future iterations (v216+).

## Background

v178 (commit `4fb1801`) made arithmetic expansion errors fatal via
`shell.pending_fatal_pe_error = Some(1)` with the rationale "matching bash".
That was an over-correction. Empirical bash behavior:

| Mode | Behavior |
|---|---|
| `bash -c 'y=$((1/0)); echo POST'` | error to stderr; `POST` NOT printed; rc=1 |
| `bash file.sh` with `y=$((1/0)); echo POST` | error to stderr; `POST` IS printed; rc=0 |

So bash's arith-fatal-ness only applies to `-c` mode and stdin command
streams; script-file execution prints the error and continues to the next
statement.

The v214 bash test suite runs `arith.tests` via `${THIS_SH} ./arith.tests` —
file mode. Bash continues past the first error; huck halts. Result: the
v214 `arith` row reports FAIL with a 200-line diff dominated by a single
cascade.

Separately, `arith.tests` begins with `set +o posix`. huck currently
rejects this with `huck: set: posix: not yet supported in this version`
because `posix` is one of `SETO_TABLE`'s `Unimplemented` entries
(`crates/huck-engine/src/builtins.rs:5000`). bash silently accepts it
(POSIX mode is off by default; the `+o` toggle is a no-op).

## Scope

**In scope:**

- `crates/huck-engine/src/builtins.rs::option_set` — special-case `posix`
  as a silent accept (Ok(()) without state change).
- `crates/huck-engine/src/expand.rs` — drop `shell.pending_fatal_pe_error =
  Some(1)` from the two arith-error arms (lines 1121 and 1569). Print the
  error to stderr; the expansion contributes an empty/0 value; the
  surrounding statement continues. The `with_err` print path is preserved.
- Update existing test `expand_arith_part_division_by_zero_is_fatal` to
  `_is_nonfatal`: assert `pending_fatal_pe_error` STAYS `None`.
- New unit test `expand_arith_invalid_lhs_assignment_is_nonfatal`.
- New unit tests for `option_set("posix", true/false)` and end-to-end
  `set +o posix` / `set -o posix` returning rc=0.
- New integration test exercising script-file arith error continuation.
- `docs/bash-divergences.md` — add a low-priority `[deferred]` entry for
  the `-c` mode divergence (huck will continue past arith errors where
  bash exits).
- Re-run `tests/bash-test-suite/runner.sh` against bash 5.2.21; refresh
  `docs/bash-test-suite-baseline.md` with new counts and refreshed Notes
  for any rows that flip status.

**Out of scope:**

- Distinguishing `-c` mode from file mode in the arith fatal-ness rule.
  Both modes treated identically (non-fatal). Tracked as a minor
  divergence.
- Fixing other semantic arith divergences exposed once the cascade is
  broken (base-N number parsing, ternary precedence quirks, error message
  format). Each is its own v216+ iteration.
- Other bash test suite categories (`arith-for`, `arith2`, `arith3`,
  etc.) — they may share root cause and improve "for free", but the v215
  acceptance criterion is the v214 sweep refresh showing concrete delta,
  not specific row flips.

## Implementation

### Fix 1: `option_set` accepts `posix`

In `crates/huck-engine/src/builtins.rs`, find `fn option_set` (around line
4987). Today:

```rust
fn option_set(shell: &mut Shell, name: &str, value: bool) -> Result<(), OptSetErr> {
    match name {
        "errexit" => { shell.shell_options.errexit = value; Ok(()) }
        // ... 8 more implemented options ...
        other => {
            if SETO_TABLE.iter().any(|o| o.name == other) {
                Err(OptSetErr::Unimplemented)
            } else {
                Err(OptSetErr::Unknown)
            }
        }
    }
}
```

Insert one new arm BEFORE the catchall:

```rust
        "posix" => {
            // Accept as a silent no-op. huck is POSIX-respecting by default;
            // `set +o posix` is a no-op against that default, and `set -o
            // posix` does not unlock additional strict-POSIX semantics.
            // Scripts that toggle the option for bash compatibility now
            // pass through cleanly. The "huck doesn't actually implement
            // strict POSIX mode" gap is a known divergence — see
            // bash-divergences.md.
            let _ = value;
            Ok(())
        }
```

`option_get` already returns `Some(false)` for `posix` via the SETO_TABLE
default; no change there.

### Fix 2: drop the fatal-PE flag on arith errors

In `crates/huck-engine/src/expand.rs`, two arms set `pending_fatal_pe_error`
on arith error:

- Line ~1119 in the main `expand` function (`WordPart::Arith` branch).
- Line ~1565 in the assignment-RHS path (also `WordPart::Arith`).

Both become:

```rust
WordPart::Arith { body, quoted: _ } => {
    match eval_arith_word(body, shell) {
        Ok(n) => {
            // existing Ok-path code, unchanged.
        }
        Err(e) => {
            with_err(|err| e!(err, "huck: arithmetic: {}", e));
            // Empty contribution to the expansion result; the surrounding
            // statement's $? will reflect the failure, but the script
            // continues — matches bash's script-file behavior. (The `-c`
            // divergence is documented in bash-divergences.md.)
            has_emitted = true;  // line 1121 site only; the line 1565 site
                                  // does not have `has_emitted`.
        }
    }
}
```

The exact "do nothing on Err" lines differ between the two sites because
their accumulator shapes differ (one builds `current` + emits to `result`,
the other appends to `result` directly). Keep both arms' Err branches
parallel: print the error, push nothing, mark `has_emitted` where the
site uses it, and DO NOT `return` early.

### Test updates

In `crates/huck-engine/src/expand.rs::mod tests`:

1. Rename `expand_arith_part_division_by_zero_is_fatal` →
   `expand_arith_part_division_by_zero_is_nonfatal`. Update the assertion:

   ```rust
   #[test]
   fn expand_arith_part_division_by_zero_is_nonfatal() {
       // Arith errors print to stderr but no longer halt — matches bash's
       // script-file behavior. `-c` mode divergence is documented.
       let mut shell = Shell::new();
       let word = Word(vec![arith_part("1 / 0")]);
       let _ = expand(&word, &mut shell);
       assert_eq!(shell.pending_fatal_pe_error, None);
   }
   ```

2. Add a sibling test for the invalid-LHS case:

   ```rust
   #[test]
   fn expand_arith_invalid_lhs_assignment_is_nonfatal() {
       let mut shell = Shell::new();
       // Force a parse-time arith error: assignment to a non-lvalue.
       let word = Word(vec![arith_part("1 + 2 = 3")]);
       let _ = expand(&word, &mut shell);
       assert_eq!(shell.pending_fatal_pe_error, None);
   }
   ```

   (Adapt the literal to match whatever the parser actually rejects with
   `assignment requires variable on LHS`. The v214 cascade was triggered
   by `1 ? 20 : x+=2` where `x` is a string; a parse-time-rejected form
   works equivalently.)

In `crates/huck-engine/src/builtins.rs::mod tests`, add 2 tests:

```rust
#[test]
fn set_posix_option_is_accepted_as_noop() {
    let mut shell = Shell::new();
    assert!(option_set(&mut shell, "posix", true).is_ok());
    assert!(option_set(&mut shell, "posix", false).is_ok());
}

#[test]
fn set_posix_via_set_command_is_accepted() {
    // End-to-end: `set +o posix` invoked via the set builtin returns 0.
    // (Adapt to the existing builtin-test invocation helper in this
    // file.)
    // … invoke the set builtin with args = ["set", "+o", "posix"] …
    // assert rc == 0; no stderr.
}
```

Add an integration test in a new file
`tests/arith_nonfatal_integration.rs`:

```rust
//! Integration test: arith errors in a script file print to stderr but
//! don't halt subsequent statements.

#[path = "common.rs"]
mod common;

#[test]
fn arith_error_does_not_halt_script_file() {
    let script = "y=$((1/0))\necho POST\n";
    let out = common::run_script_capture(script);
    // POST is on stdout because the script continued past the arith error.
    assert!(out.stdout.contains("POST"));
    // The arith error appeared on stderr.
    assert!(out.stderr.contains("arithmetic") || out.stderr.contains("division"));
}
```

(Adapt to the existing integration-test helper signature in `tests/common.rs`.)

### bash-divergences.md entry

Add a new low-priority `[deferred]` entry:

```markdown
- **L-XX: arithmetic expansion errors in `-c` mode are non-fatal** —
  `[deferred]`, low. bash: an arithmetic expansion error in `bash -c
  '...'` halts the command list (`echo POST` after a `y=$((1/0))` does
  not run). huck: the same error prints to stderr but the command list
  continues. huck's script-file behavior matches bash (both continue;
  v215 corrected huck's previously-fatal behavior to match). The `-c`
  divergence is the inverse: huck under-halts where bash over-halts.
  Detected during the v215 cascade fix; flagging here so future work
  can distinguish run modes if needed.
```

Assign the next available `L-XX` number based on the current
`docs/bash-divergences.md` state. Update the Tier 4 count.

### Baseline regeneration

After the fix lands, re-run the v214 harness:

```bash
export BASH_SOURCE_DIR=/tmp/bash-5.2.21
bash tests/bash-test-suite/runner.sh > /tmp/v215-sweep.md
```

Compare the new summary counts against the committed
`docs/bash-test-suite-baseline.md`. Update the baseline doc with:
- New header date and huck commit SHA.
- New summary counts.
- Refreshed Status for any flipped rows (`arith`, possibly `arith-for`,
  `arith2`, others).
- Refreshed Note for newly-PASSing rows (clear the prose; empty cell).
- Refreshed Note for rows still failing but with different root cause
  (the cascade was hiding the real divergence; now it shows).

## Risks

1. **Other tests depend on the fatal-PE flag.** Search for other callers of
   `pending_fatal_pe_error` that may have implicitly relied on arith errors
   setting it. Most likely none — the flag is also set by `nounset` errors
   (which we keep) and a few other paths.

2. **`-c` mode behavioral change.** Users running `huck -c 'y=$((1/0));
   echo POST'` will now see `POST` printed where they previously saw the
   error and exit 1. The behavior change is documented as a new L-XX
   divergence; no published consumer of `-c` mode is known to depend on
   the previous behavior.

3. **Sibling arith categories may not flip.** `arith-for`, `arith2`,
   `arith3` may have other failure modes independent of the cascade.
   v215's success criterion is the v214 sweep refresh — concrete delta,
   not specific row flips. The baseline doc updates regardless.

4. **`set -o posix` user expectation.** A user runs `set -o posix` and
   expects strict POSIX mode (no bash extensions). Today they'd get an
   error and know. After v215 they'd silently get non-POSIX semantics.
   Mitigation: the doc comment in `option_set` notes the gap; the
   bash-divergences L-XX entry doesn't cover this because there's no
   clear bash divergence to cite — `set -o posix` works in bash and
   changes runtime behavior; in huck it's a no-op. Document in the
   architecture cheatsheet.

## Acceptance

- `set +o posix` / `set -o posix` exits 0 with no stderr.
- huck script-file mode: `y=$((1/0))\necho POST\n` prints `POST` on
  stdout; arith error on stderr; rc=0.
- huck `-c` mode: same behavior as file mode (continues past arith
  errors) — divergence documented as L-XX.
- All new unit tests pass.
- Renamed `expand_arith_part_division_by_zero_is_nonfatal` passes.
- New integration test `arith_error_does_not_halt_script_file` passes.
- `cargo test --workspace --quiet` green.
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- All 132 `*_diff_check.sh` harnesses still pass.
- `docs/bash-test-suite-baseline.md` regenerated with new counts and
  refreshed Notes.
- New L-XX entry added to `docs/bash-divergences.md`.

## Documentation updates

- `docs/bash-divergences.md`: one new L-XX entry (the `-c` divergence).
- `docs/bash-test-suite-baseline.md`: regenerated.
- `docs/architecture.md`: no change (no architectural shift).
