# v228 — command-not-found word order Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Match bash's `<prologue> <name>: command not found` format for the bare-command not-found error (huck currently prints `huck: command not found: <name>`), a broad-shrink across alias/builtins/execscript/errors (no category flip).

**Architecture:** One emission site in `crates/huck-engine/src/executor.rs::run_subprocess` (the spawn-`NotFound` branch) is rerouted through the existing `Shell::error_prefix(None)` prologue with the command name moved before the phrase. A file-mode integration test drives the change; a file-mode byte-identical `*_diff_check.sh` harness plus the full regression sweep verify it.

**Tech Stack:** Rust (workspace crate `huck-engine`, root `huck` integration tests), bash 5.2.21 as oracle, `tests/scripts/*_diff_check.sh` byte-identical harnesses.

## Global Constraints

- Spec: `docs/superpowers/specs/2026-06-26-command-not-found-word-order-design.md`.
- Commit trailer on EVERY commit, verbatim: `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- Only **site 5327** (`run_subprocess` spawn-`NotFound`) is changed. **Do NOT touch site 3443** (`resolve`, `prog_fields.is_empty()`) — its zero-field cases ($empty / $empty arg / $empty >redir) are a deferred empty-command-word bug where bash no-ops or promotes, never emits `: command not found`.
- The error PROLOGUE (`<name>: line N:`) appears only in non-interactive FILE/`-c` mode; interactive/stdin keeps `huck:`. Prologue tests MUST run in file mode (huck given a script-file path), NOT stdin.
- `error_prefix(None)` yields `<BASH_SOURCE[0] or $0>: line N: ` non-interactively and `huck: ` interactively. Compute the prefix from `shell` into a local BEFORE acquiring the `err_writer`, so the `&shell` borrow ends first (the v227 `assign()` pattern).
- Run the full suite with `cargo test --workspace` (plain `cargo test` skips most crates). `funcnest_diff_check.sh` is RELEASE-only (v224 artifact); all other harnesses use `target/debug/huck`.

---

### Task 1: Reroute the command-not-found message through the prologue

**Files:**
- Modify: `crates/huck-engine/src/executor.rs` (the spawn-`NotFound` branch in `run_subprocess`, currently line 5327)
- Test: `tests/command_not_found_integration.rs` (new)

**Interfaces:**
- Consumes: `Shell::error_prefix(&self, cmd: Option<&str>) -> String` (existing, `shell_state.rs:873`); `err_writer(err_sink, sink)`; in `run_subprocess`, `cmd: &ResolvedCommand` (so `cmd.program` is the resolved program string) and `shell: &mut Shell` are in scope.
- Produces: the corrected message format the Task 2 harness verifies.

- [ ] **Step 1: Write the failing tests**

Create `tests/command_not_found_integration.rs`:

```rust
//! v228: command-not-found error format (word order + non-interactive prologue).
use std::process::Command;

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

/// Run huck with a script FILE (not stdin) so the non-interactive prologue
/// (`<path>: line N:`) is produced. Returns (stdout, stderr, exit_code).
fn run_file(script: &str) -> (String, String, i32) {
    let path = std::env::temp_dir().join(format!("huck-cnf-{}.sh", std::process::id()));
    std::fs::write(&path, script).unwrap();
    let out = Command::new(huck_bin()).arg(&path).output().expect("run huck file");
    let _ = std::fs::remove_file(&path);
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn missing_command_uses_bash_word_order_and_prologue() {
    let (_o, e, c) = run_file("nosuch_cmd_xyz\n");
    assert!(
        e.contains(": line 1: nosuch_cmd_xyz: command not found"),
        "expected bash word order + prologue, got: {e:?}"
    );
    assert!(!e.contains("command not found: nosuch_cmd_xyz"), "old format still present: {e:?}");
    assert!(!e.starts_with("huck:"), "file mode should not use the huck: prologue: {e:?}");
    assert_eq!(c, 127);
}

#[test]
fn missing_command_reports_its_line_number() {
    // Missing command on line 3 → the prologue must say line 3.
    let (_o, e, c) = run_file("x=1\n: ok\nnosuch_cmd_xyz\n");
    assert!(e.contains(": line 3: nosuch_cmd_xyz: command not found"), "stderr: {e:?}");
    assert_eq!(c, 127);
}

#[test]
fn quoted_empty_command_uses_bash_format() {
    // `''` is a real empty FIELD → site 5327 with an empty program name.
    // bash: `<path>: line 1: : command not found`.
    let (_o, e, c) = run_file("''\n");
    assert!(e.contains(": line 1: : command not found"), "stderr: {e:?}");
    assert!(!e.contains("command not found: "), "old format still present: {e:?}");
    assert_eq!(c, 127);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --test command_not_found_integration`
Expected: FAIL — current stderr is `huck: command not found: nosuch_cmd_xyz` (wrong word order, `huck:` prefix, no `line N:`), so the `contains(": line N: … command not found")` assertions fail.

- [ ] **Step 3: Implement the fix**

In `crates/huck-engine/src/executor.rs`, the spawn-`NotFound` branch in `run_subprocess` currently reads:

```rust
        Err(e) if e.kind() == ErrorKind::NotFound => {
            // Spawn failed: reap any heredoc writers so they don't linger.
            for wpid in heredoc_writers {
                let mut st = 0;
                unsafe { libc::waitpid(wpid, &mut st, 0); }
            }
            { let mut err = err_writer(err_sink, sink); e!(&mut *err, "huck: command not found: {}", cmd.program); }
            ExecOutcome::Continue(127)
        }
```

Change the message line so the name precedes the phrase and the bash prologue is used (compute the prefix before taking the writer, so the `&shell` borrow ends first):

```rust
        Err(e) if e.kind() == ErrorKind::NotFound => {
            // Spawn failed: reap any heredoc writers so they don't linger.
            for wpid in heredoc_writers {
                let mut st = 0;
                unsafe { libc::waitpid(wpid, &mut st, 0); }
            }
            // bash format: `<src>: line N: <name>: command not found` (the name
            // precedes the phrase; error_prefix supplies the prologue + mode split).
            {
                let prefix = shell.error_prefix(None);
                let mut err = err_writer(err_sink, sink);
                e!(&mut *err, "{prefix}{}: command not found", cmd.program);
            }
            ExecOutcome::Continue(127)
        }
```

Do not modify any other line (in particular, leave site 3443 in `resolve` unchanged).

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test --test command_not_found_integration`
Expected: PASS (3/3).

- [ ] **Step 5: Commit**

```bash
git add crates/huck-engine/src/executor.rs tests/command_not_found_integration.rs
git commit -m "$(cat <<'EOF'
v228 task 1: command-not-found message uses bash word order + prologue

run_subprocess's spawn-NotFound branch now emits
`<src>: line N: <name>: command not found` (name before the phrase, via
error_prefix(None)) instead of `huck: command not found: <name>`. Only site 5327
is changed; site 3443 (zero-field command word) is the deferred empty-command-word
bug and is left as-is.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: Byte-identical file-mode harness + regression sweep

**Files:**
- Create: `tests/scripts/command_not_found_diff_check.sh`

**Interfaces:**
- Consumes: the Task 1 message-format change.
- Produces: byte-identical file-mode verification vs bash 5.2.21, plus the broad regression evidence.

- [ ] **Step 1: Create the file-mode harness**

Create `tests/scripts/command_not_found_diff_check.sh`:

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v228: command-not-found error format
# (word order + non-interactive prologue). Runs each fragment as a SCRIPT FILE
# (file mode) on the SAME temp path for both shells, so the `<path>: line N:`
# prologue matches byte-for-byte. Compares stdout+stderr+rc.
#
# Scope: only the spawn-NotFound path (a resolved-but-missing external command,
# including the quoted-empty `''` real-field case). The zero-field command-word
# cases ($empty / $empty arg / $empty >redir) are a separate deferred divergence
# (bash no-ops or promotes; huck errors) and are NOT asserted here.
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
checkf() {
    local label="$1" body="$2" tmp b h
    tmp=$(mktemp "${TMPDIR:-/tmp}/huck-cnf.XXXXXX")
    printf '%s\n' "$body" > "$tmp"
    b=$(bash "$tmp" 2>&1; echo "EXIT:$?")
    h=$("$HUCK_BIN" "$tmp" 2>&1; echo "EXIT:$?")
    rm -f "$tmp"
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}

checkf "missing on line 1"      'nosuch_cmd_xyz'
checkf "missing reports line"   'x=1
: ok
nosuch_cmd_xyz'
checkf "missing then continues" 'nosuch_cmd_xyz
echo after'
checkf "missing with args"      'nosuch_cmd_xyz -a b c'
checkf "quoted-empty command"   "''"

echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 2: Build huck (debug) and run the new harness — all PASS**

```bash
cargo build --bin huck
HUCK_BIN="$(pwd)/target/debug/huck" bash tests/scripts/command_not_found_diff_check.sh
```
Expected: every line `PASS:`, final `Fail: 0`.

- [ ] **Step 3: Confirm the two message-adjacent harnesses stay green**

```bash
for h in command_bare_form assign_redirect; do
  HUCK_BIN="$(pwd)/target/debug/huck" bash tests/scripts/${h}_diff_check.sh >/dev/null 2>&1 \
    && echo "ok $h" || echo "FAIL $h"
done
```
Expected: `ok command_bare_form` and `ok assign_redirect` (they don't assert the not-found message, but its text changed — confirm no regression).

- [ ] **Step 4: Full regression sweep**

```bash
cargo test --workspace 2>&1 | tail -3
cargo build --release --bin huck
for f in tests/scripts/*_diff_check.sh; do
  if [ "$(basename "$f")" = "funcnest_diff_check.sh" ]; then
    HUCK_BIN="$(pwd)/target/release/huck" bash "$f" >/dev/null 2>&1 && echo "ok $f" || echo "FAIL $f"
  else
    HUCK_BIN="$(pwd)/target/debug/huck" bash "$f" >/dev/null 2>&1 && echo "ok $f" || echo "FAIL $f"
  fi
done | grep -v '^ok ' || echo "all harnesses pass"
```
Expected: `cargo test --workspace` → `0 failed` (count rose by the 3 new Task 1 tests); `all harnesses pass`.

- [ ] **Step 5: Confirm the broad shrink — command-not-found lines drop from the categories**

```bash
# Re-measure four categories; confirm they no longer show the OLD `command not
# found: NAME` ordering and have not regressed to TIMEOUT/ERROR.
for c in alias execscript errors builtins; do
  BASH_SOURCE_DIR=/tmp/bash-5.2.21 HUCK_BASH_TEST_HELPERS=/tmp/bash-test-helpers \
    HUCK_BASH_TEST_CATEGORY=$c bash tests/bash-test-suite/runner.sh 2>/dev/null | grep -E "\| $c \|"
done
```
Expected: each row is `| <cat> | FAIL |` (still failing — they have other blockers; no flip is expected). The point is they remain FAIL (not TIMEOUT/ERROR). Record in the report, for at least one category (e.g. builtins), a before/after note that the `command not found: NAME` lines are gone from its diff (the runner prints `Scratch dir (full diffs):`; grep that category's `.diff` for `command not found` — bash's `NAME: command not found` lines now match huck and no longer appear as diffs).

- [ ] **Step 6: Commit**

```bash
git add tests/scripts/command_not_found_diff_check.sh
git commit -m "$(cat <<'EOF'
v228 task 2: file-mode command_not_found_diff_check harness + regression sweep

Byte-identical file-mode checks (missing command, line-number tracking, with
args, quoted-empty) vs bash 5.2.21. Full sweep: workspace 0 failed, all
*_diff_check.sh harnesses pass; the command-not-found ordering lines drop out of
the alias/builtins/execscript/errors category diffs (no flip — other blockers
remain).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Notes for the final whole-branch review

- Only site 5327 changed; confirm site 3443 (`resolve`) was left untouched.
- The change alters the command-not-found message shell-wide (every missing command). Confirm the workspace sweep is clean and no test pinned the old `command not found: NAME` string (a grep at branch start found only the two diff-check harnesses, which don't assert it).
- No category flips (expected — this is a broad-shrink iteration); the deliverable is the format correctness + diff shrink, recorded as a deferred-divergence resolution.
- The deferred empty-command-word bug (site 3443: `$empty` no-op / `$empty arg` promote / `$empty >redir` redirection-only — huck errors 127) should get a new `[deferred]` entry in `bash-divergences.md` at merge.
