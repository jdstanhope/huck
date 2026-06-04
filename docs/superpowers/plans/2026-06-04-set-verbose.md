# `set -v` Verbose Mode Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement `set -v` / `set +v` / `set -o verbose`: when verbose is on, the shell echoes each physical input line to stderr as it is read, before executing it.

**Architecture:** Add a `verbose` flag to `ShellOptions`, wire it through `option_get`/`option_set` + the `set` short-flag cluster loop + `$-`; then echo each physical input line to stderr at huck's two input readers (`read_logical_command` for the REPL/piped stdin, `run_sourced_contents` for scripts/`source`/`-c`/`--rcfile`), gated on the current verbose state at read time.

**Tech Stack:** Rust; huck's `ShellOptions`/`Shell`/`ExecOutcome` types; the existing `set`/`set -o` machinery (`SETO_TABLE`, `option_get`/`option_set`, `OptSetErr`).

**Spec:** `docs/superpowers/specs/2026-06-04-set-verbose-design.md`

**Key facts (verified against bash 5.2):**
- Verbose echoes to **stderr**, the **raw** input line + newline, **before** execution.
- Ordering is read→echo→execute: the enabling `set -v` line is NOT echoed; the disabling `set +v` line IS echoed (then verbose turns off). Continuation lines, comments, and blank lines are all echoed.
- `v` is a short flag → appears in `$-` when on.
- bash also echoes `eval`'s re-parsed argument; huck will NOT (documented divergence — huck echoes only at its two input readers).

**Conventions:**
- Binary crate: unit tests `cargo test --bin huck <filter>`; integration `cargo test --test <name>`; full suite `cargo test`.
- Commit trailer (exact): `Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`
- Baseline: **2463** tests pass, clippy clean. Each task keeps clippy clean + suite green.
- Piped stdin (`printf … | huck`) goes through `read_logical_command` (the rustyline reader returns lines until EOF even for non-tty stdin). `huck FILE` / `huck -c` / `source` / `--rcfile` go through `run_sourced_contents`. Both need the echo.

---

## File Structure

| File | Responsibility | Task |
|------|----------------|------|
| `src/shell_state.rs` | `ShellOptions.verbose`; `dollar_dash_value` appends `v` | 1 |
| `src/builtins.rs` | `option_get`/`option_set` handle `verbose`; `set` `-v`/`+v` cluster arms (T1); echo in `run_sourced_contents` loop (T2) | 1,2 |
| `src/shell.rs` | echo each physical line in `read_logical_command` when verbose | 2 |
| `tests/set_verbose_integration.rs` | NEW — integration tests (stdout+stderr capture) | 2 |
| `tests/scripts/verbose_diff_check.sh` | NEW — huck's 16th bash-diff harness | 3 |
| `docs/bash-divergences.md`, `README.md` | M-08 deferred-list drops verbose; M-08e `[fixed v89]`; changelog; README row | 3 |

---

### Task 1: The `verbose` flag (plumbing, no echo yet)

**Files:**
- Modify: `src/shell_state.rs` (`ShellOptions` struct ~line 107; `dollar_dash_value` ~line 286)
- Modify: `src/builtins.rs` (`option_get` ~3949, `option_set` ~3962, the `set` `-`-cluster `b'e'/b'u'` arms ~4059, the `+`-cluster ~4099)
- Test: `src/builtins.rs` and `src/shell_state.rs` `#[cfg(test)]`

After this task, `set -v`/`+v`/`-o verbose` are accepted and tracked, and `$-` shows `v` — but nothing echoes yet (that's Task 2).

- [ ] **Step 1: Write failing unit tests**

In `src/builtins.rs` `#[cfg(test)]` (the module with the `run(&[...], &mut shell)` helper used by `set_o_errexit_long_form` etc.):

```rust
#[test]
fn set_v_short_flag_toggles_verbose() {
    let mut shell = Shell::new();
    let (oc, _) = run(&["-v"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert!(shell.shell_options.verbose);
    let (oc, _) = run(&["+v"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert!(!shell.shell_options.verbose);
}

#[test]
fn set_o_verbose_long_form_enables() {
    let mut shell = Shell::new();
    let (oc, _) = run(&["-o", "verbose"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert!(shell.shell_options.verbose);
}
```

In `src/shell_state.rs` `#[cfg(test)]`:

```rust
#[test]
fn dollar_dash_includes_v_when_verbose() {
    let mut sh = Shell::new();
    assert!(!sh.dollar_dash_value().contains('v'));
    sh.shell_options.verbose = true;
    assert!(sh.dollar_dash_value().contains('v'));
}

#[test]
fn dollar_dash_v_after_u() {
    let mut sh = Shell::new();
    sh.shell_options.nounset = true;
    sh.shell_options.verbose = true;
    let d = sh.dollar_dash_value();
    assert!(d.find('u').unwrap() < d.find('v').unwrap(), "got {d:?}");
}
```

- [ ] **Step 2: Run them, verify they fail**

Run: `cargo test --bin huck set_v_short_flag 2>&1 | tail` → fails (the `-v` cluster arm currently errors "not yet supported", and `shell_options.verbose` doesn't exist).

- [ ] **Step 3: Add the `verbose` field + `$-` letter**

In `src/shell_state.rs`, add to `ShellOptions`:

```rust
#[derive(Debug, Clone, Default)]
pub struct ShellOptions {
    pub errexit: bool,
    pub nounset: bool,
    pub pipefail: bool,
    pub verbose: bool,
}
```

In `dollar_dash_value`, add the `v` push after the `u` push:

```rust
    pub fn dollar_dash_value(&self) -> String {
        let mut out = String::new();
        if self.shell_options.errexit { out.push('e'); }
        if self.is_interactive { out.push('i'); }
        if self.shell_options.nounset { out.push('u'); }
        if self.shell_options.verbose { out.push('v'); }
        out
    }
```

- [ ] **Step 4: Wire `verbose` into `option_get`/`option_set` + the cluster loops**

In `src/builtins.rs` `option_get`, add a `verbose` arm before `other`:

```rust
        "errexit" => Some(shell.shell_options.errexit),
        "nounset" => Some(shell.shell_options.nounset),
        "pipefail" => Some(shell.shell_options.pipefail),
        "verbose" => Some(shell.shell_options.verbose),
        other => SETO_TABLE.iter().find(|o| o.name == other).map(|o| o.default),
```

In `option_set`, add a `verbose` arm before `other`:

```rust
        "errexit" => { shell.shell_options.errexit = value; Ok(()) }
        "nounset" => { shell.shell_options.nounset = value; Ok(()) }
        "pipefail" => { shell.shell_options.pipefail = value; Ok(()) }
        "verbose" => { shell.shell_options.verbose = value; Ok(()) }
        other => { /* unchanged */ }
```

In the `set` `-`-cluster loop (the `match c { b'e' => …, b'u' => …, b'o' => … }`), add `b'v'` next to `b'u'`:

```rust
                    b'e' => shell.shell_options.errexit = true,
                    b'u' => shell.shell_options.nounset = true,
                    b'v' => shell.shell_options.verbose = true,
                    b'o' => { /* unchanged */ }
```

In the `+`-cluster loop, add the clearing arm:

```rust
                    b'e' => shell.shell_options.errexit = false,
                    b'u' => shell.shell_options.nounset = false,
                    b'v' => shell.shell_options.verbose = false,
                    b'o' => { /* unchanged */ }
```

(`set -o verbose` / `+o verbose` now succeed automatically via the updated `option_set` — no change at those call sites.)

- [ ] **Step 5: Run tests, verify pass**

Run: `cargo test --bin huck set_v_short_flag 2>&1 | tail`, `cargo test --bin huck set_o_verbose 2>&1 | tail`, `cargo test --bin huck dollar_dash 2>&1 | tail` → pass.
Run full suite: `cargo test 2>&1 | grep -E "^test result" | awk '{p+=$4;f+=$6} END{print "PASS="p" FAIL="f}'` → FAIL=0.
Run: `cargo clippy --all-targets 2>&1 | tail -3` → clean.
Smoke (no echo yet, but no error): `printf 'set -v\necho hi\n' | ./target/debug/huck 2>/dev/null` → `hi` (and `printf 'set -v; echo $-\n' | ./target/debug/huck` → output contains `v`).

- [ ] **Step 6: Commit**

```bash
git add src/shell_state.rs src/builtins.rs
git commit -m "v89 task 1: set -v/+v/-o verbose flag plumbing + \$- v letter

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: The echo behavior + integration tests

**Files:**
- Modify: `src/shell.rs` (`read_logical_command`, in the `Ok(raw)` arm after `line` is bound)
- Modify: `src/builtins.rs` (`run_sourced_contents`, top of the `for line in contents.lines()` loop)
- Create: `tests/set_verbose_integration.rs`
- Test: integration

- [ ] **Step 1: Write failing integration tests**

Create `tests/set_verbose_integration.rs` with a stdout+stderr-capturing harness (copy the `run_capture` pattern from `tests/set_options_integration.rs`), plus a file-sourcing helper for the `run_sourced_contents` path:

```rust
//! Integration tests for v89 `set -v` verbose mode (M-08e).
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String { env!("CARGO_BIN_EXE_huck").to_string() }

/// Pipes `script` to huck on stdin (exercises read_logical_command).
/// Returns (stdout, stderr, exit_code).
fn run_capture(script: &str) -> (String, String, i32) {
    let mut child = Command::new(huck_binary())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().expect("spawn huck");
    child.stdin.as_mut().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().expect("wait");
    (String::from_utf8_lossy(&out.stdout).to_string(),
     String::from_utf8_lossy(&out.stderr).to_string(),
     out.status.code().unwrap_or(-1))
}

#[test]
fn verbose_echoes_to_stderr_not_stdout() {
    let (out, err, _) = run_capture("set -v\necho hi\n");
    assert_eq!(out, "hi\n");
    assert_eq!(err, "echo hi\n"); // `set -v` line itself is NOT echoed
}

#[test]
fn verbose_enable_disable_ordering() {
    // read->echo->execute: `set -v` not echoed; `echo b` + `set +v` echoed; `echo c` not.
    let (out, err, _) = run_capture("echo a\nset -v\necho b\nset +v\necho c\n");
    assert_eq!(out, "a\nb\nc\n");
    assert_eq!(err, "echo b\nset +v\n");
}

#[test]
fn verbose_echoes_each_continuation_line() {
    let (_, err, _) = run_capture("set -v\nif true\nthen echo x\nfi\n");
    assert!(err.contains("if true\n"), "stderr: {err:?}");
    assert!(err.contains("then echo x\n"), "stderr: {err:?}");
    assert!(err.contains("fi\n"), "stderr: {err:?}");
}

#[test]
fn verbose_dollar_dash_has_v() {
    let (out, _, _) = run_capture("set -v\necho $-\n");
    assert!(out.contains('v'), "stdout: {out:?}");
}

#[test]
fn verbose_echoes_sourced_file_lines() {
    // Exercises run_sourced_contents: `source FILE` line echoed by the reader,
    // and FILE's own lines echoed by run_sourced_contents.
    let dir = std::env::temp_dir().join(format!("huck_verbose_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let f = dir.join("sourced.sh");
    std::fs::write(&f, "echo sourced\n").unwrap();
    let script = format!("set -v\nsource {}\n", f.display());
    let (out, err, _) = run_capture(&script);
    assert_eq!(out, "sourced\n");
    assert!(err.contains(&format!("source {}", f.display())), "stderr: {err:?}");
    assert!(err.contains("echo sourced\n"), "stderr: {err:?}");
    let _ = std::fs::remove_dir_all(&dir);
}
```

- [ ] **Step 2: Run, verify fail**

Run: `cargo test --test set_verbose_integration 2>&1 | tail -15` → fails (nothing echoed yet; stderr empty).

- [ ] **Step 3: Echo in `read_logical_command` (`src/shell.rs`)**

In the `Ok(raw) => { … }` arm, AFTER the `let line = { … };` block (history expansion) and BEFORE the `match pending.take()`, add:

```rust
                // `set -v` verbose: echo each physical input line to stderr as
                // it is read, before it is parsed/executed.
                if cell.borrow().shell_options.verbose {
                    eprintln!("{line}");
                }
```

(The preceding `let line` block's `cell.borrow_mut()` has been dropped by this point, so `cell.borrow()` here is safe; the following `match pending.take()` touches only local `buffer`/`history`.)

- [ ] **Step 4: Echo in `run_sourced_contents` (`src/builtins.rs`)**

At the very top of the `for line in contents.lines() { … }` loop, before `buf.push_str(line)`, add:

```rust
    for line in contents.lines() {
        // `set -v` verbose: echo each physical input line to stderr as read.
        if shell.shell_options.verbose {
            eprintln!("{line}");
        }
        buf.push_str(line);
        buf.push('\n');
        // ... unchanged ...
```

- [ ] **Step 5: Run tests + bash parity**

Run: `cargo test --test set_verbose_integration 2>&1 | tail -15` → all 5 pass.
bash parity (verbose echoes the raw line, so `2>&1` is byte-identical):
`diff <(printf 'set -v\necho hi\n' | bash 2>&1) <(printf 'set -v\necho hi\n' | ./target/debug/huck 2>&1)` → empty.
`diff <(printf 'echo a\nset -v\necho b\nset +v\necho c\n' | bash 2>&1) <(printf 'echo a\nset -v\necho b\nset +v\necho c\n' | ./target/debug/huck 2>&1)` → empty.
`diff <(printf 'set -v\nif true\nthen echo x\nfi\n' | bash 2>&1) <(printf 'set -v\nif true\nthen echo x\nfi\n' | ./target/debug/huck 2>&1)` → empty.
Full suite: `cargo test 2>&1 | grep -E "^test result" | awk '{p+=$4;f+=$6} END{print "PASS="p" FAIL="f}'` → FAIL=0.
Clippy: `cargo clippy --all-targets 2>&1 | tail -3` → clean.

- [ ] **Step 6: Commit**

```bash
git add src/shell.rs src/builtins.rs tests/set_verbose_integration.rs
git commit -m "v89 task 2: echo input lines to stderr under set -v (both input readers)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: bash-diff harness + docs

**Files:**
- Create: `tests/scripts/verbose_diff_check.sh` (huck's 16th harness)
- Modify: `docs/bash-divergences.md`, `README.md`

- [ ] **Step 1: Write the harness**

Create `tests/scripts/verbose_diff_check.sh`, modeled on `tests/scripts/bang_negation_diff_check.sh`. `chmod +x`. The `check()` compares `2>&1` output (verbose echoes the raw line → byte-identical to bash, unlike `huck:`-prefixed errors).

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v89: set -v verbose mode (M-08e).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
check() {
    local label="$1" frag="$2" b h
    b=$(printf '%s\n' "$frag" | bash 2>&1; echo "EXIT:$?")
    h=$(printf '%s\n' "$frag" | "$HUCK_BIN" 2>&1; echo "EXIT:$?")
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
# NOTE: `eval`/trap re-parsed lines are NOT echoed by huck under -v (bash echoes
# them) — a documented M-08e divergence; excluded from byte-diffing here.
check "v echo basic"        $'set -v\necho hi'
check "v enable not echoed" $'echo a\nset -v\necho b\nset +v\necho c'
check "v multiline if"      $'set -v\nif true\nthen echo x\nfi'
check "v comment+blank"     $'set -v\n# a comment\n\necho done'
check "v dollar-dash"       $'set -v\ncase $- in *v*) echo hasv;; *) echo nov;; esac'
check "v off by default"    $'echo no-verbose'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 2: Run the harness, confirm all PASS**

```bash
cd /home/john/projects/shuck
cargo build 2>&1 | tail -1
chmod +x tests/scripts/verbose_diff_check.sh
bash tests/scripts/verbose_diff_check.sh; echo "rc=$?"
```
Expected: `Fail: 0`, `rc=0`. If a fragment differs only on a stdout/stderr interleaving-order nuance, investigate (the echo must happen before execution); do NOT weaken `check()`. If a fragment genuinely can't byte-match (e.g. `$-` letter-set differs — huck's `$-` has fewer letters than bash's), relocate it to an rc/membership integration test with a NOTE. Report any relocations.

> Note on the `v dollar-dash` fragment: it tests only whether `$-` *contains* `v` (via a `case` glob), not the full `$-` string — so it byte-matches despite huck's `$-` having a different letter set than bash. Keep it in this membership-safe form.

- [ ] **Step 3: Update `docs/bash-divergences.md`**

Read the M-08 entry (~line 134) + the Tier-2 count line (~25) first.

1. In the M-08 entry, edit the "**Still deferred**:" list to remove `-v` (verbose) — change `-v` (verbose) out of the deferred list and note it shipped: append "(`verbose` shipped in v89 — see M-08e)" to that sentence.
2. Add a new **M-08e** sub-entry after the existing M-08 sub-entries (M-08c/M-08d):
```markdown
- **M-08e: `set -v` verbose mode** — `[fixed v89]` low. `set -v`/`set +v`/`set -o
  verbose` now echo each physical input line to stderr as it is read, before
  execution (read→echo→execute, so the enabling `set -v` line isn't echoed but
  `set +v` is; continuation/comment/blank lines all echoed). New
  `ShellOptions.verbose`; `v` appears in `$-`. Echo is wired at huck's two input
  readers — `read_logical_command` (REPL/piped stdin) and `run_sourced_contents`
  (script/`source`/`-c`/`--rcfile`). **Minor divergence**: bash also echoes the
  argument `eval` re-parses (and trap-action bodies); huck echoes only at the two
  input readers, so `eval 'echo x'` under `-v` echoes the `eval` line but not the
  re-parsed `echo x`. Closes the last `set: -v/+v` errors loading a Debian
  `~/.bashrc`. huck's 16th bash-diff harness.
```
3. Update the Tier-2 count line (~25): append `; M-08e fixed by v89`.
4. Update the "Last updated" stamp (line 3) to `2026-06-04 (after v89 set -v verbose; M-08e fixed)`.
5. Add a changelog entry at the END (match the v88 entry's format), dated 2026-06-04: the `ShellOptions.verbose` field, `option_get`/`option_set` + cluster-arm + `$-` wiring, the echo at the two readers, the read→echo→execute ordering, the eval caveat, and the 16th harness.

- [ ] **Step 4: Update `README.md`**

Read the iteration table + v88 row first. Add a v89 row matching the format (escape literal `|` as `\|`):
```markdown
| v89 | `set -v` verbose mode (M-08e) | Echoes each input line to stderr as read (before execution) at both input readers; `v` in `$-`; closes the last `set -v`/`+v` bashrc errors |
```

- [ ] **Step 5: Verify whole branch**

```bash
cargo test 2>&1 | grep -E "^test result" | awk '{p+=$4;f+=$6} END{print "PASS="p" FAIL="f}'   # FAIL=0
cargo clippy --all-targets 2>&1 | tail -3                                                       # clean
for f in tests/scripts/*_diff_check.sh; do printf '%s: ' "$f"; bash "$f" >/dev/null 2>&1 && echo OK || echo FAIL; done  # all 16 OK
```

- [ ] **Step 6: Commit**

```bash
git add tests/scripts/verbose_diff_check.sh docs/bash-divergences.md README.md
git commit -m "v89 task 3: set -v verbose bash-diff harness + docs (M-08e)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Notes for the implementer

- **Binary crate:** `cargo test --bin huck <filter>` for unit; `cargo test --test set_verbose_integration` for integration.
- **Two echo points, gated on current verbose**: `read_logical_command` (REPL/piped stdin) and `run_sourced_contents` (script/source/-c/rcfile). The echo reads the verbose flag at the moment each physical line is read — that is what makes the `set -v` enabling line not echo while `set +v` does. Do NOT move the echo to `process_line` (it would double-echo and break line granularity).
- **stderr, raw line, `eprintln!`** (adds the newline that `.lines()`/`readline` stripped).
- **Verbose off by default ⇒ zero behavior change** — all existing tests unaffected.
- **Don't weaken the harness:** the `$-` fragment uses a `case *v*` membership glob precisely because huck's `$-` letter set differs from bash's; keep it that way.
