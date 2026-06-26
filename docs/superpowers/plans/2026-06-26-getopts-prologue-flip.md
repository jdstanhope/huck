# v227 — getopts category flip (error-prologue, targeted) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Flip bash's `getopts` test-suite category FAIL→PASS (PASS 9→10) by routing getopts' error messages through the bash error-prologue and fixing the OPTIND-ordering / invalid-option co-blockers.

**Architecture:** Three independent changes. (1) Convert the shared generic readonly-assignment error in `assign()` to the `error_prefix` prologue (B5). (2) Rewrite `builtin_getopts` so OPTIND binds before the name/OPTARG checks and all error prefixes match bash — usage (no prefix), internal diagnostics (`$0`), invalid-option-to-getopts and invalid-identifier (full builtin prologue) (B1–B4). (3) Add a file-mode `getopts_diff_check.sh` section and confirm the bash-suite `getopts` category PASSes.

**Tech Stack:** Rust (workspace crates `huck-engine` / root `huck`), bash 5.2.21 as oracle, `tests/scripts/*_diff_check.sh` byte-identical harnesses, `tests/bash-test-suite/runner.sh`.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-06-26-getopts-prologue-flip-design.md`. The five blockers are B1–B5 as defined there.
- Commit trailer on EVERY commit: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- GPL posture: never vendor bash source/output into committed files; read/run bash's tests only from the operator tree `/tmp/bash-5.2.21` (and helpers `/tmp/bash-test-helpers`).
- The error PROLOGUE (`<name>: line N:`) only appears in non-interactive FILE/`-c` mode; interactive/stdin keeps `huck:`. Tests that assert the prologue MUST run in file mode (run huck with a script-file path argument), NOT via stdin.
- Run the full suite with `cargo test --workspace` (3733 tests); plain `cargo test` skips most crates.
- Bash usage-error format uses only the builtin name, no shell prologue, in both modes: `getopts: usage: getopts optstring name [arg ...]`.
- `error_prefix(Some("getopts"))` yields `<BASH_SOURCE>: line N: getopts: ` (non-interactive); `error_prefix(None)` yields `<BASH_SOURCE>: line N: ` ; both return `huck: ` interactively.
- `funcnest_diff_check.sh` is RELEASE-only (v224 debug-stack artifact); run it against `target/release/huck`.

---

### Task 1: B5 — generic readonly-assignment error gets the prologue

**Files:**
- Modify: `crates/huck-engine/src/shell_state.rs` (the readonly branch in `assign`, currently lines 1647-1650)
- Test: `tests/readonly_integration.rs`

**Interfaces:**
- Consumes: `Shell::error_prefix(&self, cmd: Option<&str>) -> String` (existing, `shell_state.rs:873`). `error_prefix(None)` → `<BASH_SOURCE[0] or $0>: line N: ` non-interactively, `huck: ` interactively.
- Produces: nothing new for later tasks (this is the B5 fix the getopts category's getopts10.sub line depends on).

- [ ] **Step 1: Write the failing test**

Add to the end of `tests/readonly_integration.rs` (the file already has `huck_binary()` and `run_capture(stdin)`; add a file-mode helper since the prologue needs a real path):

```rust
/// Run huck with a script FILE (not stdin) so non-interactive prologue
/// (`<path>: line N:`) is produced. Returns (stdout, stderr).
fn run_file(script: &str) -> (String, String) {
    let dir = std::env::temp_dir();
    let path = dir.join(format!("huck-ro-{}.sh", std::process::id()));
    std::fs::write(&path, script).unwrap();
    let out = std::process::Command::new(huck_binary())
        .arg(&path)
        .output()
        .expect("run huck file");
    let _ = std::fs::remove_file(&path);
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn readonly_assignment_error_uses_prologue_in_file_mode() {
    // Line 1 = `readonly r=1`, line 2 = `r=2` (the readonly error).
    let (_o, e) = run_file("readonly r=1\nr=2\n");
    assert!(
        e.contains(": line 2: r: readonly variable"),
        "expected bash-style prologue with line number, got: {e:?}"
    );
    // File-mode prologue is the script path, never the literal `huck:`.
    assert!(
        !e.starts_with("huck:"),
        "should not use the interactive `huck:` prologue in file mode: {e:?}"
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test --test readonly_integration readonly_assignment_error_uses_prologue_in_file_mode`
Expected: FAIL — current stderr is `huck: r: readonly variable` (no `: line 2:`), so `contains` fails.

- [ ] **Step 3: Implement the prologue conversion**

In `crates/huck-engine/src/shell_state.rs`, the readonly branch in `assign` currently reads:

```rust
        // The single readonly check, before any store (no partial array
        // writes); the storage primitives do not re-check.
        if self.is_readonly(&name) {
            with_err(|err| e!(err, "huck: {name}: readonly variable"));
            return Err(AssignErr::Readonly);
        }
```

Change it to compute the prologue first (ending the `&self` borrow before `with_err`):

```rust
        // The single readonly check, before any store (no partial array
        // writes); the storage primitives do not re-check.
        if self.is_readonly(&name) {
            // bash prefixes the readonly-assignment error with the
            // non-interactive prologue (`<src>: line N:`); interactive keeps
            // `huck:` (error_prefix handles the mode split).
            let prefix = self.error_prefix(None);
            with_err(|err| e!(err, "{prefix}{name}: readonly variable"));
            return Err(AssignErr::Readonly);
        }
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test --test readonly_integration readonly_assignment_error_uses_prologue_in_file_mode`
Expected: PASS.

- [ ] **Step 5: Run the readonly-touching regression tests**

Run: `cargo test --test readonly_integration --test arrays_integration --test associative_arrays_integration`
Expected: all PASS. `arrays_integration.rs:122` and `associative_arrays_integration.rs:121` assert `err.contains("readonly variable")` (substring), which survives the prefix change.

- [ ] **Step 6: Commit**

```bash
git add crates/huck-engine/src/shell_state.rs tests/readonly_integration.rs
git commit -m "$(cat <<'EOF'
v227 task 1: generic readonly-assignment error uses the error prologue (B5)

assign()'s readonly branch now emits `<src>: line N: NAME: readonly variable`
non-interactively (error_prefix(None)) instead of `huck: NAME: readonly
variable`; interactive output is unchanged. This is the shared site getopts10's
`OPTARG: readonly variable` line routes through.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: B1–B4 — rewrite `builtin_getopts`

**Files:**
- Modify: `crates/huck-engine/src/builtins.rs` (`builtin_getopts`, currently lines 4896-4942)
- Test: `tests/getopts_integration.rs`

**Interfaces:**
- Consumes: `getopts_step(optstring: &str, args: &[String], optind: usize, sp: usize) -> GetoptsStep` (existing, returns `{ name, optarg: Option<String>, optind: usize, sp: usize, error: Option<String>, done: bool }`); `is_valid_name(&str) -> bool`; `Shell::error_prefix(Some("getopts"))`; `Shell::shell_argv0: String`; `Shell::set/try_set/unset/lookup_var`; `Shell::getopts_optind_cache`, `Shell::getopts_sp`. From Task 1: a readonly `try_set("OPTARG", …)` now prints the prologue-prefixed readonly error.
- Produces: the corrected getopts behavior the Task 3 harness and the bash-suite category verify.

- [ ] **Step 1: Write the failing tests**

Append to `tests/getopts_integration.rs` (it already has `run(stdin) -> (String, String, i32)`):

```rust
#[test]
fn usage_error_drops_huck_prefix_and_fixes_arg_ellipsis() {
    // Too few operands → builtin usage error, no shell prologue, rc 2.
    let (_o, e, c) = run("getopts\n");
    assert_eq!(e, "getopts: usage: getopts optstring name [arg ...]\n", "stderr: {e:?}");
    assert_eq!(c, 2);
}

#[test]
fn invalid_option_to_getopts_itself_is_rejected() {
    // getopts has no options of its own; `-a` is invalid → error + usage, rc 2.
    // `echo "rc=$?"` captures getopts' status into stdout; the script's own
    // exit (c) is the echo's success (0).
    let (o, e, c) = run("getopts -a opts name\necho \"rc=$?\"\n");
    assert!(e.contains("-a: invalid option"), "stderr: {e:?}");
    assert!(e.contains("getopts: usage: getopts optstring name [arg ...]"), "stderr: {e:?}");
    assert_eq!(o, "rc=2\n", "stdout: {o:?}");
    assert_eq!(c, 0, "script's own exit is the echo's success");
}

#[test]
fn invalid_name_still_binds_optind() {
    // bash binds OPTIND from the parsed option BEFORE validating the name var,
    // so an invalid name still advances OPTIND (here: parsed `-a` → OPTIND 2).
    let (o, e, _c) = run("set -- -a\ngetopts ab bad-name\necho \"oi=$OPTIND\"\n");
    assert_eq!(o, "oi=2\n", "stdout: {o:?}");
    assert!(e.contains("`bad-name': not a valid identifier"), "stderr: {e:?}");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --test getopts_integration usage_error_drops_huck_prefix invalid_option_to_getopts invalid_name_still_binds_optind`
Expected: FAIL — current code prints `huck: getopts: usage: … [arg]`, silently accepts `-a`, and validates the name before binding OPTIND (so `oi=` is empty/unset).

- [ ] **Step 3: Rewrite `builtin_getopts`**

Replace the entire `builtin_getopts` function (currently `builtins.rs:4896-4942`) with:

```rust
fn builtin_getopts(args: &[String], err: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    const USAGE: &str = "getopts: usage: getopts optstring name [arg ...]";

    // getopts accepts no options of its own. A leading operand starting with
    // '-' (other than "-" or "--") is an invalid option, reported with the
    // builtin-error prologue plus a usage line (bash: internal_getopt("")).
    // A leading "--" is consumed as the option terminator.
    let mut args = args;
    if let Some(first) = args.first() {
        if first.starts_with('-') && first != "-" && first != "--" {
            let c = first.chars().nth(1).unwrap();
            e!(err, "{}-{c}: invalid option", shell.error_prefix(Some("getopts")));
            e!(err, "{USAGE}");
            return ExecOutcome::Continue(2);
        }
        if first == "--" {
            args = &args[1..];
        }
    }

    if args.len() < 2 {
        e!(err, "{USAGE}");
        return ExecOutcome::Continue(2);
    }
    let optstring = args[0].clone();
    let name = args[1].clone();

    // Parse explicit args if given, else the current positional parameters.
    let parse_args: Vec<String> = if args.len() > 2 {
        args[2..].to_vec()
    } else {
        shell.positional_args.clone()
    };
    // Read OPTIND (default 1; clamp <1 to 1).
    let optind = shell
        .lookup_var("OPTIND")
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|&n| n >= 1)
        .unwrap_or(1);
    // Detect an external OPTIND reset → fresh within-word cursor.
    let sp = if optind != shell.getopts_optind_cache { 1 } else { shell.getopts_sp };

    let step = getopts_step(&optstring, &parse_args, optind, sp);

    // Bind OPTIND + cursor cache UNCONDITIONALLY, before the name/OPTARG
    // checks — bash's dogetopts binds OPTIND from the post-parse value
    // regardless of whether the name is a valid identifier, so an invalid
    // name (or readonly OPTARG) still advances OPTIND.
    shell.set("OPTIND", step.optind.to_string());
    shell.getopts_optind_cache = step.optind;
    shell.getopts_sp = step.sp;

    // OPTARG is bound before the name check (bash binds OPTARG in dogetopts
    // before getopts_bind_variable runs the identifier check). A readonly
    // OPTARG prints the prologue-prefixed readonly error (Task 1).
    match step.optarg {
        Some(v) => { let _ = shell.try_set("OPTARG", v); }
        None => shell.unset("OPTARG"),
    }

    // Validate the name AFTER OPTIND/OPTARG are bound. Invalid identifier is a
    // hard error (bash EXECUTION_FAILURE = 1) with the full builtin prologue.
    if !is_valid_name(&name) {
        e!(err, "{}`{name}': not a valid identifier", shell.error_prefix(Some("getopts")));
        return ExecOutcome::Continue(1);
    }

    // Assign the matched letter (or '?' / ':').
    let _ = shell.try_set(&name, step.name.clone());

    // Verbose getopts-internal option diagnostic (suppressed by OPTERR=0),
    // prefixed with $0 (bash sets argv[0] = dollar_vars[0] for sh_getopt).
    if let Some(body) = step.error
        && shell.lookup_var("OPTERR").as_deref() != Some("0")
    {
        e!(err, "{}: {body}", shell.shell_argv0);
    }
    ExecOutcome::Continue(if step.done { 1 } else { 0 })
}
```

- [ ] **Step 4: Run the new tests to verify they pass**

Run: `cargo test --test getopts_integration usage_error_drops_huck_prefix invalid_option_to_getopts invalid_name_still_binds_optind`
Expected: PASS.

- [ ] **Step 5: Run the full getopts integration suite (no regressions)**

Run: `cargo test --test getopts_integration`
Expected: all PASS — the pre-existing tests (`invalid_option_verbose_sets_question`, `missing_arg_silent_mode`, `no_args_uses_positional_params`, `local_optind_resets_per_function`, etc.) are unaffected; in stdin mode `$0` is the binary path so verbose diagnostics keep working and OPTERR=0 still suppresses them.

- [ ] **Step 6: Commit**

```bash
git add crates/huck-engine/src/builtins.rs tests/getopts_integration.rs
git commit -m "$(cat <<'EOF'
v227 task 2: rewrite builtin_getopts for bash error formats + OPTIND order

B1 usage `getopts: usage: … name [arg ...]` (no huck: prefix); B2 internal
diagnostics prefixed with $0; B3 reject a leading invalid option to getopts
itself (`-X: invalid option` + usage, rc 2); B4 bind OPTIND before validating
the name var (invalid name still advances OPTIND) and emit the invalid-
identifier error through the builtin prologue.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: File-mode harness + confirm the `getopts` category flips

**Files:**
- Modify: `tests/scripts/getopts_diff_check.sh` (add a file-mode section)

**Interfaces:**
- Consumes: the Task 1 + Task 2 behavior (prologue prefixes, OPTIND order, invalid-option rejection).
- Produces: byte-identical file-mode verification of all five blockers, and the bash-suite `getopts` PASS that is the iteration deliverable.

- [ ] **Step 1: Add a file-mode section to the harness**

Append to `tests/scripts/getopts_diff_check.sh`, immediately before the final `echo ""; echo "Total: …"` summary lines:

```bash
# --- file-mode checks: run each fragment as a SCRIPT FILE so the non-
# interactive prologue (`<path>: line N:`) is produced, and assert
# byte-identical stdout+stderr+rc against bash 5.2.21. The same temp path is
# used for both shells, so the prologue path matches. ---
checkf() {
    local label="$1" body="$2" tmp b h
    tmp=$(mktemp "${TMPDIR:-/tmp}/huck-getopts.XXXXXX")
    printf '%s\n' "$body" > "$tmp"
    b=$(bash "$tmp" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tmp" 2>&1; echo "EXIT:$?")
    rm -f "$tmp"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

# B1: too few operands → usage, no shell prologue, rc 2
checkf "file: usage too few"      'getopts; echo "rc=$?"'
# B2: getopts-internal diagnostic prefixed with $0 (the script path)
checkf "file: internal diag \$0"  'set -- -z; getopts ab o; echo "o=$o rc=$?"'
# B3: invalid option to getopts itself → error + usage, rc 2
checkf "file: invalid builtin opt" 'getopts -a opts name; echo "rc=$?"'
# B4: invalid name var → builtin prologue error + OPTIND still bound
checkf "file: invalid name optind" 'set -- -a
getopts :ab: bad-name
echo "oi=$OPTIND"
[ "$OPTIND" -gt 1 ] && shift $(( OPTIND - 1 ))
echo "rest=$*"'
# B5: readonly OPTARG → prologue-prefixed readonly error (generic assign site)
checkf "file: readonly OPTARG"     'set -- -a bb
readonly OPTARG
getopts :x x
echo "done x=$x"'
```

- [ ] **Step 2: Run the harness (debug build) and verify all PASS**

```bash
cargo build --bin huck
HUCK_BIN="$(pwd)/target/debug/huck" bash tests/scripts/getopts_diff_check.sh
```
Expected: every line `PASS:` (both the pre-existing stdin checks and the five new `file:` checks), final `Fail: 0`.

- [ ] **Step 3: Confirm the bash-test-suite `getopts` category flips to PASS**

```bash
BASH_SOURCE_DIR=/tmp/bash-5.2.21 HUCK_BASH_TEST_HELPERS=/tmp/bash-test-helpers \
  HUCK_BASH_TEST_CATEGORY=getopts bash tests/bash-test-suite/runner.sh
```
Expected: the Markdown summary shows `| getopts | PASS |` (this auto-builds release). This is the iteration deliverable.

- [ ] **Step 4: Full regression sweep (B5 touches the shared assign path)**

```bash
cargo test --workspace 2>&1 | tail -5
cargo build --release --bin huck
for f in tests/scripts/*_diff_check.sh; do
  if [ "$(basename "$f")" = "funcnest_diff_check.sh" ]; then
    HUCK_BIN="$(pwd)/target/release/huck" bash "$f" >/dev/null 2>&1 && echo "ok $f" || echo "FAIL $f"
  else
    HUCK_BIN="$(pwd)/target/debug/huck" bash "$f" >/dev/null 2>&1 && echo "ok $f" || echo "FAIL $f"
  fi
done | grep -v '^ok ' || echo "all harnesses pass"
```
Expected: `cargo test --workspace` → `3733 passed; 0 failed` (count rises with the new tests); every harness `ok` (printed line `all harnesses pass`). Note `funcnest_diff_check.sh` runs against the release binary (v224 debug-stack artifact).

- [ ] **Step 5: Re-measure the `errors` category did not regress**

```bash
BASH_SOURCE_DIR=/tmp/bash-5.2.21 HUCK_BASH_TEST_HELPERS=/tmp/bash-test-helpers \
  HUCK_BASH_TEST_CATEGORY=errors bash tests/bash-test-suite/runner.sh | grep -E '\| errors \|'
```
Expected: `| errors | FAIL |` still (it has independent blockers) — the point is it must not change from FAIL to TIMEOUT/ERROR; its diff should shrink, not grow. (B5's prologue conversion only improves or leaves its `readonly variable` lines unchanged.)

- [ ] **Step 6: Commit**

```bash
git add tests/scripts/getopts_diff_check.sh
git commit -m "$(cat <<'EOF'
v227 task 3: file-mode getopts_diff_check section; getopts category flips PASS

Adds byte-identical file-mode checks for all five blockers (usage, $0-prefixed
internal diagnostic, invalid-option-to-getopts, invalid-name+OPTIND, readonly
OPTARG prologue). The bash-test-suite `getopts` category now PASSes (9→10).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Notes for the final whole-branch review

- Confirm the `getopts` bash-suite category is PASS and the PASS count is 10.
- Confirm B5's prologue conversion did not regress any `*_diff_check.sh` harness or workspace test (the shared `assign()` site is the only cross-cutting change).
- Untested edge (acceptable, documented in spec): a non-silent optstring that hits BOTH an invalid option and an invalid name prints only the identifier error (bash would print both); not exercised by the suite.
- B4 invalid-name return code is `1` (bash `EXECUTION_FAILURE`), changed from huck's previous `2`; not constrained by the suite, byte-harness confirms parity where observable.
