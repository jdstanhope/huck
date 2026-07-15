# `-o`/`+o` CLI flag (#159) + `wait -f` (#160) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Accept `huck -o <option>` / `+o <option>` at the command line (apply like `set -o`), and accept `wait -f` — both currently rejected.

**Architecture:** Reuse existing machinery. #159: `parse_cli` collects `-o`/`+o` into an ordered list; `repl.rs` applies them at startup through the existing `option_set` table via a thin public wrapper. #160: add a `-f` arm to the `wait` arg parser (accept-and-conform; huck's `wait` already blocks to termination) and update its usage string.

**Tech Stack:** Rust — `crates/huck-engine/src/shell.rs` + `builtins.rs`, `crates/huck-cli/src/repl.rs`, bash-diff harnesses.

**Design spec:** `docs/superpowers/specs/2026-07-15-cli-o-flag-and-wait-f-design.md`. Closes [#159](https://github.com/jdstanhope/huck/issues/159), [#160](https://github.com/jdstanhope/huck/issues/160).

## Global Constraints

- **Commit trailer** verbatim on every commit: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- **`cargo fmt --all`** before every commit (CI runs `--check`).
- **Build:** `cargo build -p huck`. **Never** `cargo test --workspace` (OOMs the box). Engine lib tests: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`.
- **bash parity (verified):** `bash -o posix -c 'echo ok'` → `ok`; `bash -o badname -c …` → `bash: badname: invalid option name`, rc 2; bash's wait usage is `wait: usage: wait [-fn] [-p var] [id ...]`; `wait -f %1` → rc 0.
- The two fixes are independent; Task 1 (#159) and Task 2 (#160) touch disjoint code.

---

### Task 1: `-o` / `+o` command-line flag (#159)

**Files:**
- Modify: `crates/huck-engine/src/shell.rs` — `CliOptions` struct + `Default` + `parse_cli` (three `CliOptions` construction sites) + a unit test
- Modify: `crates/huck-engine/src/builtins.rs` — add `pub fn set_o_option_by_name` near `option_set` (~6149)
- Modify: `crates/huck-cli/src/repl.rs` — apply `opts.o_options` after the posix/noexec block (~107)
- Create: `tests/scripts/cli_o_flag_diff_check.sh`

**Interfaces:**
- Produces: `CliOptions.o_options: Vec<(String, bool)>` (option name, enable); `huck_engine::builtins::set_o_option_by_name(shell: &mut Shell, name: &str, enable: bool) -> Result<(), ()>`.
- Consumes: the private `option_set(shell, name, value) -> Result<(), OptSetErr>` (`builtins.rs:6149`).

- [ ] **Step 1: Write the failing harness**

Create `tests/scripts/cli_o_flag_diff_check.sh`:

```bash
#!/usr/bin/env bash
# v300 (#159): huck must accept `-o <option>` / `+o <option>` at the command
# line and apply it like `set -o`, matching bash. Compares stdout+stderr+rc
# byte-identically (shell-name prologue normalized to SH:).
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: huck binary not found at $HUCK (build with: cargo build -p huck)" >&2; exit 1; }

FAIL=0
norm() { sed -e 's#^bash: line [0-9]*: #SH: #' -e 's#^bash: #SH: #' \
             -e "s#^$HUCK: line [0-9]*: #SH: #" -e "s#^$HUCK: #SH: #"; }
# $@ = argv to pass to each shell (already split); compares combined out+err+rc.
check() {
  local label=$1; shift
  local b h
  b=$( { bash    "$@"; echo "rc=$?"; } 2>&1 | norm )
  h=$( { "$HUCK" "$@"; echo "rc=$?"; } 2>&1 | norm )
  if [ "$b" != "$h" ]; then
    echo "FAIL [$label]"; echo "  bash: $b"; echo "  huck: $h"; FAIL=1
  else
    echo "PASS [$label]"
  fi
}

# -o applies the option (errexit takes effect -> no "after", rc 1)
check 'o-errexit'  -o errexit -c 'false; echo after'
# -o posix is accepted and runs
check 'o-posix'    -o posix -c 'echo ok'
# +o syntax accepted and runs
check 'plus-o'     +o errexit -c 'echo ok'
# bad option name -> "<name>: invalid option name", rc 2
check 'o-badname'  -o badname -c 'echo x'

if [ $FAIL -ne 0 ]; then echo "cli_o_flag_diff_check FAILED" >&2; exit 1; fi
echo "cli_o_flag_diff_check OK"
```

- [ ] **Step 2: Run it to confirm it fails (RED)**

```bash
cargo build -p huck && chmod +x tests/scripts/cli_o_flag_diff_check.sh && tests/scripts/cli_o_flag_diff_check.sh
```
Expected: FAIL on all `-o`/`+o` cases — huck currently prints `SH: unrecognized option: -o` (or `+o`) and exits nonzero, while bash applies the option.

- [ ] **Step 3: Add the `o_options` field to `CliOptions`**

In `crates/huck-engine/src/shell.rs`, add the field to the `CliOptions` struct (after `posix`):

```rust
    /// `-o <name>` / `+o <name>` command-line options, in argv order. `bool` is
    /// enable (`-o` = true, `+o` = false). Applied at startup like `set -o`.
    pub o_options: Vec<(String, bool)>,
```

Add it to the `Default` impl (after `posix: false,`):

```rust
            o_options: Vec::new(),
```

- [ ] **Step 4: Collect `-o`/`+o` in `parse_cli`**

In `parse_cli`, add a mutable accumulator next to the others:

```rust
    let mut o_options: Vec<(String, bool)> = Vec::new();
```

Add two match arms in the option loop (next to `"--posix"`):

```rust
            "-o" | "+o" => {
                let enable = args[i] == "-o";
                i += 1;
                if i >= args.len() {
                    return Err(format!(
                        "{}: option requires an argument",
                        if enable { "-o" } else { "+o" }
                    ));
                }
                o_options.push((args[i].clone(), enable));
                i += 1;
            }
```

Populate BOTH `CliOptions` constructions: the `PrintVersion` early return gets `o_options: Vec::new(),` (version printing ignores options), and the final `Ok(CliOptions { … })` gets `o_options,`.

- [ ] **Step 5: Add the public wrapper in `builtins.rs`**

In `crates/huck-engine/src/builtins.rs`, immediately after `option_set` (~line 6180, after its closing brace), add:

```rust
/// Public entry for applying a command-line `-o <name>` / `+o <name>` option
/// (#159). Wraps the private `option_set` table so the CLI layer (huck-cli)
/// doesn't duplicate the option list. `Err(())` means the name is not a
/// recognized `set -o` option (the caller renders `<name>: invalid option name`).
pub fn set_o_option_by_name(shell: &mut Shell, name: &str, enable: bool) -> Result<(), ()> {
    match option_set(shell, name, enable) {
        Ok(()) => Ok(()),
        Err(OptSetErr::Unknown) => Err(()),
    }
}
```

- [ ] **Step 6: Apply `o_options` at startup in `repl.rs`**

In `crates/huck-cli/src/repl.rs`, immediately AFTER the posix-mode application block (the `{ … shell_cell.borrow_mut().shell_options.posix = posix; }` block, ~line 107) and BEFORE the `match opts.mode {` dispatch, insert:

```rust
    // `-o <name>` / `+o <name>` command-line options (#159): apply in argv order
    // through the engine's `set -o` table, before any program/interactive
    // dispatch, so they govern the whole session.
    for (name, enable) in &opts.o_options {
        if huck_engine::builtins::set_o_option_by_name(&mut shell_cell.borrow_mut(), name, *enable)
            .is_err()
        {
            eprintln!("huck: {name}: invalid option name");
            std::process::exit(2);
        }
    }
```

(If a local `shell` binding is more idiomatic than `shell_cell.borrow_mut()` at this point, match the surrounding code's borrow style — the posix block just above uses `shell_cell.borrow_mut()`.)

- [ ] **Step 7: Add a `parse_cli` unit test**

In `shell.rs`'s test module (next to `parse_cli_noexec_flag`), add:

```rust
    #[test]
    fn parse_cli_o_options_collected_in_order() {
        let o = parse_cli(&[
            "-o".into(), "errexit".into(),
            "+o".into(), "posix".into(),
            "-c".into(), "echo hi".into(),
        ])
        .unwrap();
        assert_eq!(
            o.o_options,
            vec![("errexit".to_string(), true), ("posix".to_string(), false)]
        );
    }

    #[test]
    fn parse_cli_o_missing_arg_errors() {
        assert!(parse_cli(&["-o".into()]).is_err());
    }
```

- [ ] **Step 8: Format, build, confirm GREEN**

```bash
cargo fmt --all && cargo build -p huck
tests/scripts/cli_o_flag_diff_check.sh
cargo test -p huck-engine --jobs 1 --lib parse_cli -- --test-threads 1 2>&1 | tail -3
```
Expected: `cli_o_flag_diff_check OK` (all PASS); the two new `parse_cli` tests pass.

- [ ] **Step 9: Commit**

```bash
git add crates/huck-engine/src/shell.rs crates/huck-engine/src/builtins.rs crates/huck-cli/src/repl.rs tests/scripts/cli_o_flag_diff_check.sh
git commit -m "$(cat <<'EOF'
fix(#159): accept -o/+o command-line options

parse_cli collects -o/+o (name, enable) into CliOptions.o_options; repl.rs
applies them in argv order at startup via a new public set_o_option_by_name
wrapper over the existing option_set table. Bad name -> `huck: <name>: invalid
option name`, exit 2; missing arg -> parse error.

Refs #159.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: `wait -f` (#160)

**Files:**
- Modify: `crates/huck-engine/src/builtins.rs` — `parse_wait_args` (`-f` arm + usage string at line 4347)
- Create: `tests/scripts/wait_f_diff_check.sh`

**Interfaces:** none new — a local parser change.

- [ ] **Step 1: Write the failing harness**

Create `tests/scripts/wait_f_diff_check.sh`:

```bash
#!/usr/bin/env bash
# v300 (#160): `wait -f` must be accepted (bash: "wait for full termination";
# huck's wait already blocks to termination, so accept-and-conform). The usage
# string also gains -f. Compares stdout+stderr+rc byte-identically.
set -u
cd "$(dirname "$0")/../.." || exit 1
HUCK=target/debug/huck
[ -x "$HUCK" ] || { echo "FAIL: huck binary not found at $HUCK (build with: cargo build -p huck)" >&2; exit 1; }

FAIL=0
norm() { sed -e 's#^bash: line [0-9]*: #SH: #' -e 's#^bash: #SH: #' \
             -e "s#^$HUCK: line [0-9]*: #SH: #" -e "s#^$HUCK: #SH: #"; }
check() {
  local label=$1 frag=$2 b h
  b=$( { timeout 10 bash    -c "$frag"; echo "rc=$?"; } 2>&1 | norm )
  h=$( { timeout 10 "$HUCK" -c "$frag"; echo "rc=$?"; } 2>&1 | norm )
  if [ "$b" != "$h" ]; then
    echo "FAIL [$label]"; echo "  bash: $b"; echo "  huck: $h"; FAIL=1
  else
    echo "PASS [$label]"
  fi
}

# -f accepted, waits for the bg job to finish (rc 0)
check 'wait-f'        'sleep 0.2 & wait -f %1; echo done'
# usage string now includes -f (a bad flag prints the updated usage)
check 'wait-badflag'  'wait -Z'
# regression: -n -p var still works
check 'wait-n-p'      'sleep 0.1 & wait -n -p WP; echo "rc=$? set=${WP:+yes}"'

if [ $FAIL -ne 0 ]; then echo "wait_f_diff_check FAILED" >&2; exit 1; fi
echo "wait_f_diff_check OK"
```

- [ ] **Step 2: Run it to confirm it fails (RED)**

```bash
cargo build -p huck && chmod +x tests/scripts/wait_f_diff_check.sh && tests/scripts/wait_f_diff_check.sh
```
Expected: FAIL on `wait-f` (huck: `SH: wait: -f: invalid option` + old usage) and `wait-badflag` (huck's usage line omits `-f`).

- [ ] **Step 3: Accept `-f` in `parse_wait_args`**

In `crates/huck-engine/src/builtins.rs`, in `parse_wait_args` (~4312), add a `-f` arm next to `"-n"`:

```rust
            "-f" => {
                // #160: "wait for full termination rather than a status change".
                // huck's wait has no return-on-stop path (it already blocks to
                // termination), so accept-and-conform: no state to record.
                idx += 1;
            }
```

- [ ] **Step 4: Update the usage string**

At the `wait: usage:` emission (line 4347), change:

```rust
                e!(err, "wait: usage: wait [-n] [-p var] [id ...]");
```

to:

```rust
                e!(err, "wait: usage: wait [-fn] [-p var] [id ...]");
```

- [ ] **Step 5: Format, build, confirm GREEN**

```bash
cargo fmt --all && cargo build -p huck && tests/scripts/wait_f_diff_check.sh
```
Expected: `wait_f_diff_check OK` (all PASS).

- [ ] **Step 6: Commit**

```bash
git add crates/huck-engine/src/builtins.rs tests/scripts/wait_f_diff_check.sh
git commit -m "$(cat <<'EOF'
fix(#160): accept `wait -f`

parse_wait_args accepts -f (accept-and-conform: huck's wait already blocks to
termination, with no return-on-stop path), and the usage string becomes
`wait [-fn] [-p var] [id ...]` to match bash.

Refs #160.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: whole-branch verification

**Files:** none (verification only).

- [ ] **Step 1: fmt + build both binaries**

```bash
cargo fmt --all --check && echo "(fmt clean)"
cargo build --locked --bin huck && cargo build --release --locked --bin huck
```

- [ ] **Step 2: Engine lib tests**

```bash
cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 2>&1 | tail -3
```
Expected: all pass.

- [ ] **Step 3: Full diff sweep (both binaries)**

```bash
( ulimit -v 1500000; timeout 1100 tests/scripts/run_diff_checks.sh 2>&1 | tail -5 )
```
Expected: 0 failed; `cli_o_flag_diff_check.sh` and `wait_f_diff_check.sh` both appear and pass.

- [ ] **Step 4: Sanity — the original #159/#160 repros**

```bash
target/debug/huck -o posix -c 'echo ok'          # -> ok
target/debug/huck -c 'sleep 0.1 & wait -f $!; echo rc=$?'   # -> rc=0
```

---

## Self-Review

- **Spec coverage:** §1 (#159: `parse_cli` collect + `set_o_option_by_name` wrapper + repl.rs apply + bad-name/missing-arg errors) → Task 1. §2 (#160: `-f` arm + usage string) → Task 2. Testing (two harnesses + parse_cli unit tests) → Tasks 1-2; sweep → Task 3. Covered.
- **Placeholder scan:** every step has concrete code/commands + expected output; both harnesses complete. No TBD.
- **Type consistency:** `CliOptions.o_options: Vec<(String, bool)>` set in all three construction sites (struct default, PrintVersion return, final return) and the collector; `set_o_option_by_name(&mut Shell, &str, bool) -> Result<(), ()>` wraps `option_set(…) -> Result<(), OptSetErr>`; repl.rs calls it via `huck_engine::builtins::`.
- **Independence:** Task 1 (shell.rs/builtins.rs/repl.rs) and Task 2 (builtins.rs `parse_wait_args`) touch disjoint code; either can be reviewed/reverted alone.
- **Risk:** the only cross-crate surface is the one new `pub fn`; applying `-o` via the shared `option_set` keeps `$-`/posix/monitor consistent with `set -o`. Confirm no existing CLI test asserts `-o` is rejected (none expected).
