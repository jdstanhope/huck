# huck v41 — `kill -l` + README cleanup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close bash divergence M-39 (`kill -l` listing + name/number /
status-decode lookups) and trim the stale README "Not yet implemented"
paragraph.

**Architecture:** All code changes confined to `src/traps.rs` (one new
table + helper) and `src/builtins.rs` (kill dispatcher signature
extension to accept `out: &mut dyn Write`; new `handle_kill_l` +
`print_killable_table` helpers; deduplicate `signal_by_name` to use
the new `killable_signals()`). One new integration test file. README
gets a surgical content edit.

**Tech Stack:** Rust. `libc::SIG*` constants for the signal table.

**Spec:** `docs/superpowers/specs/2026-05-28-huck-kill-l-and-readme-design.md`

**Branch:** `v41-kill-l-readme` (to be created in preamble step P.1).

**Commit trailer convention** (every commit in this iteration):

```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Preamble: Create the working branch

- [ ] **Step P.1: Create branch from main and check it out**

```bash
git checkout main
git pull --ff-only
git checkout -b v41-kill-l-readme
```

Expected: `Switched to a new branch 'v41-kill-l-readme'`.

The spec + this plan are committed as the first commit on this branch
(handled by the controller before Task 1 begins).

---

## Task 1: `killable_signals` + builtin core + tests

**Files:**
- Modify: `src/traps.rs` — add `KILLABLE` table + `killable_signals()` helper.
- Modify: `src/builtins.rs` — `builtin_kill` signature change, `-l` dispatch, two new helpers, `signal_by_name` body replacement, 10 unit tests.

### Step 1.1: Add `KILLABLE` table and `killable_signals()` in `src/traps.rs`

Locate `const TRAPPABLE: &[(&str, i32)] = &[ ... ];` at `src/traps.rs:229-244`. **Immediately below** the closing `];` of `TRAPPABLE`, insert:

```rust
/// All signals huck knows how to SEND via `kill`. This is the
/// trappable list plus KILL and STOP, which can be sent but not
/// trapped.
const KILLABLE: &[(&str, i32)] = &[
    ("HUP",   libc::SIGHUP),
    ("INT",   libc::SIGINT),
    ("QUIT",  libc::SIGQUIT),
    ("KILL",  libc::SIGKILL),
    ("USR1",  libc::SIGUSR1),
    ("USR2",  libc::SIGUSR2),
    ("PIPE",  libc::SIGPIPE),
    ("ALRM",  libc::SIGALRM),
    ("TERM",  libc::SIGTERM),
    ("CHLD",  libc::SIGCHLD),
    ("CONT",  libc::SIGCONT),
    ("STOP",  libc::SIGSTOP),
    ("TSTP",  libc::SIGTSTP),
    ("TTIN",  libc::SIGTTIN),
    ("TTOU",  libc::SIGTTOU),
    ("WINCH", libc::SIGWINCH),
];
```

Then locate `pub fn name_table()` at `src/traps.rs:247` and **immediately below its closing `}`**, add:

```rust
/// Returns the table of signal names huck knows how to SEND via
/// `kill`. This is the 14-entry trappable table plus KILL and STOP,
/// which are not trappable but ARE sendable. Used by `kill -l` and
/// the `signal_by_name` helper in `builtins.rs`.
pub fn killable_signals() -> &'static [(&'static str, i32)] {
    KILLABLE
}
```

- [ ] **Step 1.1: Insert the table and helper**

### Step 1.2: Build to confirm `src/traps.rs` compiles

Run: `cargo build`
Expected: clean. The new symbols are unused at this point — Rust treats unused `const` and `pub fn` as non-errors, so no warnings are expected. If clippy is run alongside, you may see `dead_code` warnings; we use them in Task 1.3+ and 1.5+ so they'll be silent by end of task.

- [ ] **Step 1.2: Build clean**

### Step 1.3: Change `builtin_kill` signature to accept `out`

In `src/builtins.rs`, find the `match name {` dispatch at `src/builtins.rs:46-61`. Change the kill row from:

```rust
        "kill" => builtin_kill(args, shell),
```

to:

```rust
        "kill" => builtin_kill(args, out, shell),
```

Then find `fn builtin_kill(args: &[String], shell: &mut Shell) -> ExecOutcome {` at `src/builtins.rs:753`. Change the signature to:

```rust
fn builtin_kill(args: &[String], out: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
```

Inside the body, before the existing `let (sig, targets) = ...` block, insert the `-l` short-circuit:

```rust
    if matches!(args.first().map(|s| s.as_str()), Some("-l")) {
        return handle_kill_l(&args[1..], out);
    }
```

Build to confirm: `cargo build`. The new `out` parameter is currently unused inside `builtin_kill`'s existing body (it's passed only to the new `handle_kill_l`). Rust may emit an unused-parameter warning — silence by renaming to `_out` for the existing path, but DON'T do that yet because Task 1.5's tests verify the writer threads through. Instead, accept a single transient `unused_variables` warning for this commit; it'll be resolved when `handle_kill_l` actually writes to it.

If clippy/CI fails the build on the warning, prefix the parameter in the dispatch row only (the existing kill body doesn't use it) — that is, keep the function param named `out` but mark unused branches with `let _ = out;` at the top of the non-`-l` branch as a temporary measure. Cleanest is to thread `out` into nothing for the non-`-l` path; the warning is acceptable for one commit.

- [ ] **Step 1.3: Signature change + `-l` short-circuit**

### Step 1.4: Add `handle_kill_l` and `print_killable_table`

Find `fn signal_by_name` at `src/builtins.rs:712-734`. **Immediately above** that function, add:

```rust
fn print_killable_table(out: &mut dyn Write) {
    let table = crate::traps::killable_signals();
    let mut sorted: Vec<&(&str, i32)> = table.iter().collect();
    sorted.sort_by_key(|(_, n)| *n);
    let cols = 4;
    for chunk in sorted.chunks(cols) {
        let mut line = String::new();
        for (i, (name, num)) in chunk.iter().enumerate() {
            if i > 0 { line.push(' '); }
            line.push_str(&format!("{num:>2}) {name:<5}"));
        }
        let _ = writeln!(out, "{line}");
    }
}

fn handle_kill_l(args: &[String], out: &mut dyn Write) -> ExecOutcome {
    if args.is_empty() {
        print_killable_table(out);
        return ExecOutcome::Continue(0);
    }

    for arg in args {
        if let Ok(n) = arg.parse::<i32>() {
            let lookup = if n >= 128 { n - 128 } else { n };
            match crate::traps::killable_signals()
                .iter()
                .find(|(_, num)| *num == lookup)
            {
                Some((name, _)) => {
                    let _ = writeln!(out, "{name}");
                }
                None => {
                    eprintln!("huck: kill: {arg}: invalid signal specification");
                    return ExecOutcome::Continue(1);
                }
            }
        } else {
            let upper = arg.to_ascii_uppercase();
            let name = upper.strip_prefix("SIG").unwrap_or(&upper);
            match crate::traps::killable_signals()
                .iter()
                .find(|(table_name, _)| *table_name == name)
            {
                Some((_, num)) => {
                    let _ = writeln!(out, "{num}");
                }
                None => {
                    eprintln!("huck: kill: {arg}: invalid signal specification");
                    return ExecOutcome::Continue(1);
                }
            }
        }
    }
    ExecOutcome::Continue(0)
}
```

- [ ] **Step 1.4: Insert the two helpers**

### Step 1.5: Replace `signal_by_name` body

Find `fn signal_by_name(s: &str) -> Option<i32>` at `src/builtins.rs:712-734`. Replace the entire function body (the 22-line `Some(match name { ... })` block) so the function reads:

```rust
fn signal_by_name(s: &str) -> Option<i32> {
    let upper = s.to_ascii_uppercase();
    let name = upper.strip_prefix("SIG").unwrap_or(&upper);
    crate::traps::killable_signals()
        .iter()
        .find_map(|(table_name, num)| {
            if *table_name == name {
                Some(*num)
            } else {
                None
            }
        })
}
```

This is the deduplication: `signal_by_name` now consults the same table as `kill -l`, which means `kill -WINCH pid` works (WINCH wasn't in the old hardcoded 15-name table).

- [ ] **Step 1.5: Replace `signal_by_name` body**

### Step 1.6: Build

Run: `cargo build`
Expected: clean. The `out` parameter on `builtin_kill` is now used (via `handle_kill_l`). No warnings.

- [ ] **Step 1.6: Build clean**

### Step 1.7: Add the 10 new unit tests

Find the `mod kill_tests` block in `src/builtins.rs` (it begins at line 2102 — `#[cfg(test)] mod kill_tests { use super::*; use crate::shell_state::Shell; ... }`). Append these tests **inside** that mod block, before its closing `}`. The mod already has `use super::*;` so `signal_by_name`, `run_builtin`, `ExecOutcome`, `Shell`, and `libc` are all in scope:

```rust
    #[test]
    fn kill_l_no_args_lists_all_16_signals() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin("kill", &["-l".to_string()], &mut buf, &mut shell);
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let s = String::from_utf8(buf).unwrap();
        // Each entry is `NN) NAME`. Count occurrences of `)` to verify 16.
        assert_eq!(s.matches(')').count(), 16, "output: {s}");
        // Spot-check known names appear.
        assert!(s.contains("KILL"), "output missing KILL: {s}");
        assert!(s.contains("TERM"), "output missing TERM: {s}");
        assert!(s.contains("WINCH"), "output missing WINCH: {s}");
    }

    #[test]
    fn kill_l_with_name_returns_number() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "kill",
            &["-l".to_string(), "TERM".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(s.trim(), libc::SIGTERM.to_string());
    }

    #[test]
    fn kill_l_with_sig_prefix_returns_number() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "kill",
            &["-l".to_string(), "SIGTERM".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(s.trim(), libc::SIGTERM.to_string());
    }

    #[test]
    fn kill_l_lowercase_name_returns_number() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "kill",
            &["-l".to_string(), "term".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(s.trim(), libc::SIGTERM.to_string());
    }

    #[test]
    fn kill_l_with_number_returns_name() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "kill",
            &["-l".to_string(), libc::SIGTERM.to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(s.trim(), "TERM");
    }

    #[test]
    fn kill_l_status_decode() {
        // 128 + SIGKILL → "KILL"
        let arg = (128 + libc::SIGKILL).to_string();
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "kill",
            &["-l".to_string(), arg],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(s.trim(), "KILL");
    }

    #[test]
    fn kill_l_unknown_name_errors_status_1() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "kill",
            &["-l".to_string(), "xyz".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn kill_l_invalid_number_errors_status_1() {
        // 99 is not in our table (and 99-128 is negative).
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "kill",
            &["-l".to_string(), "99".to_string()],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(1)));
    }

    #[test]
    fn kill_l_multiple_args_decodes_each() {
        let mut shell = Shell::new();
        let mut buf: Vec<u8> = Vec::new();
        let outcome = run_builtin(
            "kill",
            &[
                "-l".to_string(),
                libc::SIGHUP.to_string(),
                libc::SIGKILL.to_string(),
                libc::SIGTERM.to_string(),
            ],
            &mut buf,
            &mut shell,
        );
        assert!(matches!(outcome, ExecOutcome::Continue(0)));
        let s = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = s.lines().collect();
        assert_eq!(lines, vec!["HUP", "KILL", "TERM"]);
    }

    #[test]
    fn signal_by_name_resolves_winch() {
        // Regression: WINCH wasn't in the old hardcoded 15-name table.
        // After deduplication via killable_signals(), it should resolve.
        assert_eq!(signal_by_name("WINCH"), Some(libc::SIGWINCH));
        assert_eq!(signal_by_name("SIGWINCH"), Some(libc::SIGWINCH));
        assert_eq!(signal_by_name("winch"), Some(libc::SIGWINCH));
    }
```


- [ ] **Step 1.7: Append the 10 tests**

### Step 1.8: Run the new tests

Run: `cargo test kill_l_ signal_by_name -- --nocapture`
Expected: all 10 new tests pass. The existing `signal_by_name_table_recognizes_common_signals` test should also still pass (the dedup preserves the recognized names).

- [ ] **Step 1.8: New tests pass**

### Step 1.9: Run the full unit suite

Run: `cargo test --bin huck`
Expected: all unit tests pass.

If the project doesn't expose a `--bin` target name, fall back to plain `cargo test --lib` or `cargo test` filtered by `--tests`.

- [ ] **Step 1.9: Full unit suite passes**

### Step 1.10: Clippy

Run: `cargo clippy --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 1.10: Clippy clean**

### Step 1.11: Commit

```bash
git add src/traps.rs src/builtins.rs
git commit -m "$(cat <<'EOF'
builtin: kill -l with all bash forms (v41 task 1)

Add `killable_signals()` in src/traps.rs (16-entry table = TRAPPABLE
+ KILL + STOP). Extend `builtin_kill` to detect `-l` as the first
arg and dispatch to a new `handle_kill_l` helper.

`handle_kill_l` supports all four bash forms:
- bare `kill -l`: 4-column NUM) NAME listing (mirrors v35's
  print_signal_table format)
- `kill -l NAME` (with optional SIG prefix, case-insensitive)
  → number
- `kill -l NUM` → name (no SIG prefix)
- `kill -l <status>` where status ≥ 128 → name via N-128 decode
- multiple args produce one decode per line; stop at first error

Unknown names / out-of-range numbers print
"huck: kill: <arg>: invalid signal specification" and exit 1.

Deduplicate `signal_by_name` to look up via `killable_signals()`.
Fixes a latent bug: `kill -WINCH pid` failed previously because
WINCH wasn't in `signal_by_name`'s old hardcoded 15-name table.

builtin_kill signature extended to accept `out: &mut dyn Write`
(passed through to handle_kill_l); the dispatcher row updated.

10 new unit tests cover bare listing (count = 16, spot-check names),
name→num (3 forms: bare, SIG-prefix, lowercase), num→name,
status-decode, two error paths, multi-arg decoding, and the WINCH
regression for the dedup.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 1.11: Commit Task 1**

---

## Task 2: Integration tests

**Files:**
- Create: `tests/kill_l_integration.rs`

Two binary-driven tests using the established harness pattern (mirrors `tests/wait_integration.rs` and `tests/ansi_c_quoting_integration.rs`).

### Step 2.1: Create the integration test file

Create `tests/kill_l_integration.rs` with this exact content:

```rust
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run(script: &str) -> (String, String) {
    let mut child = Command::new(huck_binary())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn huck");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(script.as_bytes())
        .unwrap();
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn kill_l_bare_lists_signals() {
    // `kill -l` then exit; stdout should contain both TERM and KILL.
    let (out, _) = run("kill -l\nexit\n");
    assert!(out.contains("TERM"), "stdout missing TERM: {:?}", out);
    assert!(out.contains("KILL"), "stdout missing KILL: {:?}", out);
}

#[test]
fn kill_l_name_to_number() {
    // `kill -l TERM` should print SIGTERM's number on its own line.
    let (out, _) = run("kill -l TERM\nexit\n");
    let expected = format!("{}", libc::SIGTERM);
    assert!(
        out.lines().any(|l| l == expected),
        "expected line {expected:?} in stdout: {out:?}"
    );
}
```

Note: this file uses `libc::SIGTERM`. The integration-tests target needs `libc` as a dev-dependency. Check `Cargo.toml` — huck's main binary already depends on `libc`, and integration tests inherit it via dev-dependencies inheritance only if it's listed under `[dev-dependencies]`. Verify by running the test in step 2.2; if compilation fails with `unresolved import libc`, add `libc = "*"` to `[dev-dependencies]` in `Cargo.toml` (match the existing version used elsewhere).

- [ ] **Step 2.1: Create the integration test file**

### Step 2.2: Run the new integration suite

Run: `cargo test --test kill_l_integration -- --nocapture`
Expected: both 2 tests pass.

If `libc::SIGTERM` doesn't resolve at the integration-test crate level, add to `Cargo.toml`:

```toml
[dev-dependencies]
libc = "0.2"
```

(Match the version of the existing `libc` dep in `[dependencies]` — read `Cargo.toml`'s top section first.)

If you do need to modify `Cargo.toml`, the commit should include it alongside the test file.

- [ ] **Step 2.2: Integration tests pass**

### Step 2.3: Run the full integration suite

Run: `cargo test --tests`
Expected: all integration tests pass. Known PTY flake `pty_compound_stage_pipeline_stops_and_resumes` may flake under load — re-run in isolation if hit.

- [ ] **Step 2.3: Full integration suite green**

### Step 2.4: Clippy

Run: `cargo clippy --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 2.4: Clippy clean**

### Step 2.5: Commit

```bash
git add tests/kill_l_integration.rs Cargo.toml Cargo.lock
git commit -m "$(cat <<'EOF'
test: kill -l integration coverage (v41 task 2)

Two binary-driven tests: `kill -l` lists signals (verifies TERM and
KILL appear in stdout), `kill -l TERM` writes the numeric value on
its own line. The latter uses libc::SIGTERM at test time so the
expected value tracks the host's signal numbering.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

If Cargo.toml wasn't modified, drop it from the `git add` list. If `Cargo.lock` is gitignored, drop that too — read `.gitignore` if unsure.

- [ ] **Step 2.5: Commit Task 2**

---

## Task 3: Docs + README cleanup + full-suite verify

**Files:**
- Modify: `docs/bash-divergences.md` — flip M-39, add change-log entry.
- Modify: `README.md` — add v41 row, trim "Not yet implemented" paragraph.

### Step 3.1: Flip M-39 in the Job-control section

In `docs/bash-divergences.md`, find the M-39 entry. After v40 it should currently read:

```markdown
- **M-39: `kill -l` (list signals)** — `[deferred]` medium. huck: rejects. bash: lists all signal names.
```

Replace with:

```markdown
- **M-39: `kill -l` (list signals)** — `[fixed v41]` medium. All four bash forms supported: bare `kill -l` (4-column `NUM) NAME` listing of the 16 sendable signals = 14 trappable + KILL + STOP), `kill -l NAME` → number (case-insensitive, optional `SIG` prefix), `kill -l NUM` → name (no `SIG` prefix), `kill -l <status≥128>` → name via N-128 decode. Multiple args produce one decode per line, stopping at the first invalid arg. `kill -l` also fixed a latent bug where `kill -WINCH pid` was rejected because WINCH wasn't in the old hardcoded `signal_by_name` table.
```

- [ ] **Step 3.1: Flip M-39**

### Step 3.2: Add the v41 change-log entry

In `docs/bash-divergences.md`, find `## Change log` and the most recent `**2026-05-28**` entry (about v40). Add IMMEDIATELY after it:

```markdown
- **2026-05-28**: M-39 (`kill -l` with all bash forms) shipped as v41. New `killable_signals()` table in `src/traps.rs` (14 trappable + KILL + STOP). `builtin_kill` extended with `-l` short-circuit + new `handle_kill_l` / `print_killable_table` helpers; `signal_by_name` deduplicated to share the same table (also fixes `kill -WINCH pid`). README "Not yet implemented" paragraph trimmed to remove items shipped in v33, v37, v40, and v41. No new L-* divergences.
```

- [ ] **Step 3.2: Add change-log entry**

### Step 3.3: Add the v41 row to the README version table

In `README.md`, find the version table (search for the existing `| v40       | \`wait -n\`` line). Add IMMEDIATELY after it:

```markdown
| v41       | `kill -l` (M-39) + README cleanup                              |
```

So the final block reads:

```markdown
| v39       | ANSI-C quoting `$'…'` (M-28)                                   |
| v40       | `wait -n` + multi-arg `wait` (M-37 + M-38)                    |
| v41       | `kill -l` (M-39) + README cleanup                              |
```

Match the column padding of v39/v40. Count the spaces before the closing `|` so the right pipe lines up visually.

- [ ] **Step 3.3: Add README v41 row**

### Step 3.4: Trim the "Not yet implemented" paragraph

In `README.md`, find the block at lines ~233-238:

```markdown
**Not yet implemented:**
substring parameter expansion (`${var:off:len}`),
case modification (`${var^^}`/`${var,,}`),
brace expansion (`{a,b,c}`), extended job specs
(`%cmd`/`%?cmd`), `wait -n`, `kill -l`/`-s`, `disown -a`/`-r`/`-h`,
backgrounded multi-pipeline sequences (`cmd1 && cmd2 &`), aliases.
```

Replace with:

```markdown
**Not yet implemented:**
brace expansion (`{a,b,c}`), extended job specs
(`%cmd`/`%?cmd`), `kill -s`, `disown -a`/`-r`/`-h`,
backgrounded multi-pipeline sequences (`cmd1 && cmd2 &`), aliases.
```

Removed items: `${var:off:len}` (shipped v33), `${var^^}`/`${var,,}` (v37), `wait -n` (v40), `kill -l` (v41, this iteration). Kept items: brace expansion, extended job specs, `kill -s`, `disown -a`/`-r`/`-h`, backgrounded multi-pipelines, aliases.

- [ ] **Step 3.4: Trim README paragraph**

### Step 3.5: Run the full suite

Run: `cargo test --all-targets`
Expected: all tests pass (modulo the known PTY flake — re-run in isolation if hit).

- [ ] **Step 3.5: Full suite green**

### Step 3.6: Clippy

Run: `cargo clippy --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 3.6: Clippy clean**

### Step 3.7: Commit Task 3

```bash
git add docs/bash-divergences.md README.md
git commit -m "$(cat <<'EOF'
docs: mark M-39 fixed; v41 in README; trim stale paragraph

Job-control section: M-39 (`kill -l`) flipped from [deferred] to
[fixed v41] with descriptive text covering all four bash forms (bare
list, name→num, num→name, status-decode) and the
signal_by_name/WINCH dedup fix.

Change log: 2026-05-28 v41 entry summarizing the killable_signals()
table addition and the README cleanup.

README: v41 row added to the version table; "Not yet implemented"
paragraph trimmed to remove items shipped in v33 (${var:off:len}),
v37 (${var^^}/${var,,}), v40 (wait -n), and v41 (kill -l). Kept the
genuinely-deferred items: brace expansion, extended job specs,
kill -s, disown -a/-r/-h, backgrounded multi-pipelines, aliases.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 3.7: Commit Task 3**

---

## Final verification (controller, not a task)

After the three task commits land, the controller should:

1. Run `cargo test --all-targets` once more.
2. Run `cargo clippy --all-targets -- -D warnings`.
3. Confirm the branch has exactly four commits ahead of `main`: the docs preamble (spec + plan), task 1, task 2, task 3.
4. Dispatch a final cross-task code-reviewer subagent over the full diff (`main..v41-kill-l-readme`).
5. Merge to `main` with `--no-ff`, push, delete the branch, update the `huck iterations` memory entry with v41.
