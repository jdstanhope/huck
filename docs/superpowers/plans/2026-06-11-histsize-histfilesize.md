# huck v139 — HISTSIZE / HISTFILESIZE Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Honor `$HISTSIZE` (in-memory history cap) and `$HISTFILESIZE` (history file cap) read from huck's variable table with bash semantics, replacing the fixed compile-time `HISTORY_MAX = 1000`.

**Architecture:** `History.max` becomes `Option<usize>` (`None`=unlimited, `Some(n)`=cap, `Some(0)`=empty), with new `set_max` (sets + evicts) and `save_capped(file_cap)`. Two `Shell` resolvers read the variable table (`resolve_histsize`/`resolve_histfilesize`) with full bash semantics, wired in via `Shell::record_history` (per-command) and `Shell::save_history` (at save) plus a post-rc re-cap.

**Tech Stack:** Rust; huck's `History` (`src/history.rs`), `Shell` (`src/shell_state.rs`), REPL (`src/shell.rs`); `tempfile` dev-dependency; the huck test binary (`env!("CARGO_BIN_EXE_huck")`).

**Reference:** spec at `docs/superpowers/specs/2026-06-11-histsize-histfilesize-design.md`.

**GIT SAFETY:** Do NOT `git checkout <sha>` (a detached HEAD lost commits in a prior iteration). Stay on `v139-histsize-histfilesize`. Every commit ends with `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

**Build note:** the repo is large; `cargo build`/`cargo test` take a few minutes — be patient.

---

### Task 1: `History` — `Option<usize>` cap, `set_max`, `save_capped`

**Files:**
- Modify: `src/history.rs` (the `History` struct ~32-145; the `HISTORY_MAX` const ~5; the test module's struct literals)

- [ ] **Step 1: Write the failing tests** — add to the `#[cfg(test)] mod tests` in `src/history.rs`:

```rust
#[test]
fn set_max_some_evicts_to_cap() {
    let mut h = History { entries: Vec::new(), base_number: 1, max: Some(1000), file: None };
    for c in ["c1", "c2", "c3", "c4", "c5"] { h.add(c.to_string()); }
    h.set_max(Some(3));
    let got: Vec<(usize, &str)> = h.entries().collect();
    assert_eq!(got, vec![(3, "c3"), (4, "c4"), (5, "c5")]);
}

#[test]
fn set_max_none_keeps_all() {
    let mut h = History { entries: Vec::new(), base_number: 1, max: Some(2), file: None };
    h.set_max(None);
    for c in ["c1", "c2", "c3", "c4"] { h.add(c.to_string()); }
    let got: Vec<(usize, &str)> = h.entries().collect();
    assert_eq!(got, vec![(1, "c1"), (2, "c2"), (3, "c3"), (4, "c4")]);
}

#[test]
fn set_max_zero_empties() {
    let mut h = History { entries: Vec::new(), base_number: 1, max: Some(10), file: None };
    h.add("c1".to_string());
    h.set_max(Some(0));
    assert_eq!(h.last(), None);
    h.add("c2".to_string()); // add under cap 0 keeps nothing
    assert_eq!(h.last(), None);
}

#[test]
fn save_capped_writes_last_n() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hist");
    let mut h = History { entries: Vec::new(), base_number: 1, max: None, file: Some(path.clone()) };
    for c in ["c1", "c2", "c3"] { h.add(c.to_string()); }
    h.save_capped(Some(2));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "c2\nc3\n");
}

#[test]
fn save_capped_none_writes_all_and_zero_writes_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hist");
    let mut h = History { entries: Vec::new(), base_number: 1, max: None, file: Some(path.clone()) };
    for c in ["c1", "c2"] { h.add(c.to_string()); }
    h.save_capped(None);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "c1\nc2\n");
    h.save_capped(Some(0));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
}
```

- [ ] **Step 2: Run — verify failure (compile errors: `max: Some(...)` mismatch, missing methods)**

Run: `cargo test --lib history 2>&1 | tail -20`
Expected: FAILS to compile — `max` is `usize` but the new tests pass `Some(...)`, and `set_max`/`save_capped` don't exist. Record the errors.

- [ ] **Step 3: Change the field type + const visibility**

`src/history.rs` line ~5: make the const crate-visible:
```rust
pub(crate) const HISTORY_MAX: usize = 1000;
```
Line ~35: change the field:
```rust
    max: Option<usize>,
```
Line ~44 (`new()`):
```rust
            max: Some(HISTORY_MAX),
```

- [ ] **Step 4: Update `add` and `load` to honor `Option`, add `set_max` and `save_capped`**

Replace `add` (~50-56):
```rust
    /// Appends a command, evicting the oldest entries past the cap (no eviction
    /// when `max` is `None` = unlimited).
    pub fn add(&mut self, line: String) {
        self.entries.push(line);
        self.enforce_max();
    }

    /// Evicts oldest entries until the list fits `self.max` (no-op when `None`).
    fn enforce_max(&mut self) {
        if let Some(cap) = self.max {
            while self.entries.len() > cap {
                self.entries.remove(0);
                self.base_number += 1;
            }
        }
    }

    /// Sets the in-memory cap and immediately evicts entries past it. (v139)
    pub fn set_max(&mut self, max: Option<usize>) {
        self.max = max;
        self.enforce_max();
    }
```

In `load` (~111-128), replace the truncation block:
```rust
                let mut lines: Vec<String> =
                    contents.lines().map(unescape_for_load).collect();
                if let Some(cap) = self.max
                    && lines.len() > cap
                {
                    lines.drain(0..lines.len() - cap);
                }
                self.entries = lines;
                self.base_number = 1;
```

Add `save_capped` and make `save` delegate to it (replace the existing `save` ~134-144):
```rust
    /// Writes the last `file_cap` entries to the histfile (all when `None`,
    /// empty when `Some(0)`), overwriting. (v139)
    pub fn save_capped(&self, file_cap: Option<usize>) {
        let Some(path) = &self.file else { return };
        let start = match file_cap {
            Some(cap) => self.entries.len().saturating_sub(cap),
            None => 0,
        };
        let mut out = String::new();
        for entry in &self.entries[start..] {
            out.push_str(&escape_for_save(entry));
            out.push('\n');
        }
        if let Err(e) = std::fs::write(path, out) {
            eprintln!("huck: warning: could not write history file: {e}");
        }
    }

    /// Back-compat: write all in-memory entries (capped by the in-memory `max`).
    pub fn save(&self) {
        self.save_capped(self.max);
    }
```

- [ ] **Step 5: Fix the pre-existing test struct literals (compiler-guided)**

`cargo build 2>&1 | tail -30`. The compiler flags every `History { … max: <int> … }` in the test module (lines ~430, 457, 507, 515, 530, 546, 556, 570, 579, 595, 604, 620, 629). Change each `max: 1000` → `max: Some(1000)` and `max: 3` → `max: Some(3)`. (The `load_truncates_to_max_most_recent` test at ~546 keeps `Some(3)` and its assertion is unchanged — load still truncates to 3.)

- [ ] **Step 6: Run the history tests**

Run: `cargo test --lib history 2>&1 | tail -20`
Expected: all history tests PASS (the 5 new + all pre-existing).

- [ ] **Step 7: Commit**

```bash
git add src/history.rs
git commit -m "$(printf 'feat: History cap becomes Option<usize> with set_max + save_capped\n\nNone=unlimited, Some(n)=cap, Some(0)=empty. Groundwork for HISTSIZE/\nHISTFILESIZE (M-59).\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 2: `Shell::resolve_histsize` / `resolve_histfilesize`

**Files:**
- Modify: `src/shell_state.rs` (add two methods + their unit tests)

- [ ] **Step 1: Write the failing tests** — add to the `#[cfg(test)] mod tests` in `src/shell_state.rs` (use the existing test-Shell constructor pattern — look at a nearby test such as the `IFS` test at ~2084 for how a `Shell` is built and `set` is called):

```rust
#[test]
fn resolve_histsize_bash_semantics() {
    let mut s = Shell::new();
    assert_eq!(s.resolve_histsize(), Some(1000)); // unset -> default
    s.set("HISTSIZE", "".to_string());
    assert_eq!(s.resolve_histsize(), Some(1000)); // empty -> default
    s.set("HISTSIZE", "abc".to_string());
    assert_eq!(s.resolve_histsize(), Some(1000)); // non-numeric -> default
    s.set("HISTSIZE", "0".to_string());
    assert_eq!(s.resolve_histsize(), Some(0));    // zero -> empty
    s.set("HISTSIZE", "200".to_string());
    assert_eq!(s.resolve_histsize(), Some(200));  // positive -> cap
    s.set("HISTSIZE", "-1".to_string());
    assert_eq!(s.resolve_histsize(), None);       // negative -> unlimited
}

#[test]
fn resolve_histfilesize_bash_semantics() {
    let mut s = Shell::new();
    s.set("HISTSIZE", "200".to_string());
    assert_eq!(s.resolve_histfilesize(), Some(200)); // unset -> effective HISTSIZE
    s.set("HISTFILESIZE", "50".to_string());
    assert_eq!(s.resolve_histfilesize(), Some(50));  // positive -> cap
    s.set("HISTFILESIZE", "0".to_string());
    assert_eq!(s.resolve_histfilesize(), Some(0));   // zero -> empty file
    s.set("HISTFILESIZE", "-1".to_string());
    assert_eq!(s.resolve_histfilesize(), None);      // negative -> inhibit
    s.set("HISTFILESIZE", "abc".to_string());
    assert_eq!(s.resolve_histfilesize(), None);      // non-numeric -> inhibit
}
```

- [ ] **Step 2: Run — verify failure**

Run: `cargo test --lib resolve_hist 2>&1 | tail -15`
Expected: FAILS to compile (methods don't exist).

- [ ] **Step 3: Implement the two methods** — add to the `impl Shell` block in `src/shell_state.rs` (near `lookup_var`/`set`, ~483-540):

```rust
    /// Resolves `$HISTSIZE` to the in-memory history cap. `None` = unlimited.
    /// unset/empty/non-numeric -> default 1000; negative -> unlimited; else n.
    /// (v139, M-59)
    pub fn resolve_histsize(&self) -> Option<usize> {
        match self.lookup_var("HISTSIZE") {
            Some(v) => match v.trim().parse::<i64>() {
                Ok(n) if n < 0 => None,
                Ok(n) => Some(n as usize),
                Err(_) => Some(crate::history::HISTORY_MAX),
            },
            None => Some(crate::history::HISTORY_MAX),
        }
    }

    /// Resolves `$HISTFILESIZE` to the history-file cap. `None` = no truncation.
    /// unset -> effective HISTSIZE; negative/non-numeric -> inhibit; else n.
    /// (v139, M-59)
    pub fn resolve_histfilesize(&self) -> Option<usize> {
        match self.lookup_var("HISTFILESIZE") {
            Some(v) => match v.trim().parse::<i64>() {
                Ok(n) if n < 0 => None,
                Ok(n) => Some(n as usize),
                Err(_) => None,
            },
            None => self.resolve_histsize(),
        }
    }
```

- [ ] **Step 4: Run the tests**

Run: `cargo test --lib resolve_hist 2>&1 | tail -15`
Expected: both PASS. (If `lookup_var` returns `None` rather than `Some("")` for an explicitly-set-empty var, the `""`→default assertion still holds because the `None` branch also returns the default — confirm by the test passing; if it does NOT, adjust the test's empty-string expectation to match — the behaviour is equivalent.)

- [ ] **Step 5: Commit**

```bash
git add src/shell_state.rs
git commit -m "$(printf 'feat: Shell::resolve_histsize/resolve_histfilesize (bash semantics)\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 3: Wire HISTSIZE/HISTFILESIZE into the REPL + integration tests

**Files:**
- Modify: `src/shell_state.rs` (add `record_history` + `save_history`)
- Modify: `src/shell.rs` (replace the add site + 6 save sites; add the post-rc re-cap)
- Create: `tests/histsize_integration.rs`

- [ ] **Step 1: Write the failing integration tests** — create `tests/histsize_integration.rs`:

```rust
//! v139: HISTSIZE/HISTFILESIZE honored from the variable table (M-59). Deterministic
//! via piped stdin + a temp HISTFILE; the vars are set in the spawn env (huck
//! imports the env into its variable table at startup, so they're visible from the
//! first recorded command). Asserts exact histfile contents.
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

/// Run huck with piped `script` on stdin and the given extra env vars; return the
/// resulting HISTFILE contents.
fn run_hist(envs: &[(&str, &str)], script: &str) -> String {
    let dir = tempfile::tempdir().unwrap();
    let hf = dir.path().join("hist");
    let mut cmd = Command::new(huck_bin());
    cmd.env("HISTFILE", &hf);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
    let mut child = cmd.spawn().expect("spawn huck");
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    child.wait().unwrap();
    std::fs::read_to_string(&hf).unwrap_or_default()
}

#[test]
fn histsize_caps_in_memory_list() {
    // HISTSIZE=2 -> the in-memory list keeps the last 2; HISTFILESIZE unset ->
    // defaults to HISTSIZE (2); the saved file holds the last 2 commands.
    let out = run_hist(&[("HISTSIZE", "2")], "echo a\necho b\necho c\n");
    assert_eq!(out, "echo b\necho c\n", "out={out:?}");
}

#[test]
fn histfilesize_caps_file_below_histsize() {
    let out = run_hist(&[("HISTSIZE", "100"), ("HISTFILESIZE", "1")], "echo a\necho b\n");
    assert_eq!(out, "echo b\n", "out={out:?}");
}

#[test]
fn histsize_negative_is_unlimited() {
    let out = run_hist(&[("HISTSIZE", "-1")], "echo a\necho b\necho c\necho d\n");
    assert_eq!(out, "echo a\necho b\necho c\necho d\n", "out={out:?}");
}

#[test]
fn histsize_zero_empties() {
    let out = run_hist(&[("HISTSIZE", "0")], "echo a\necho b\n");
    assert_eq!(out, "", "out={out:?}");
}

#[test]
fn histfilesize_zero_empties_file() {
    let out = run_hist(&[("HISTFILESIZE", "0")], "echo a\n");
    assert_eq!(out, "", "out={out:?}");
}

#[test]
fn default_unset_keeps_all_small() {
    let out = run_hist(&[], "echo a\necho b\n");
    assert_eq!(out, "echo a\necho b\n", "out={out:?}");
}
```

- [ ] **Step 2: Run — verify failure**

Run: `cargo test --test histsize_integration 2>&1 | tail -20`
Expected: `histsize_caps_in_memory_list`, `histfilesize_caps_file_below_histsize`, `histsize_zero_empties`, `histfilesize_zero_empties_file` FAIL (pre-wiring: no cap is applied — all commands written); `histsize_negative_is_unlimited` and `default_unset_keeps_all_small` PASS. Record.

- [ ] **Step 3: Add the `Shell` wiring helpers** — in `src/shell_state.rs` `impl Shell`:

```rust
    /// Records a command in history, applying the current `$HISTSIZE` cap. (v139)
    pub fn record_history(&mut self, line: String) {
        let cap = self.resolve_histsize();
        let h = std::rc::Rc::make_mut(&mut self.history);
        h.set_max(cap);
        h.add(line);
    }

    /// Saves history to the histfile, applying the `$HISTFILESIZE` cap. (v139)
    pub fn save_history(&self) {
        self.history.save_capped(self.resolve_histfilesize());
    }
```
(Confirm `Rc` is in scope in `shell_state.rs`; if not, use the fully-qualified `std::rc::Rc` as written.)

- [ ] **Step 4: Replace the REPL add site** — `src/shell.rs` (~322-324). Change:
```rust
                    if !history.trim().is_empty() {
                        Rc::make_mut(&mut shell.history).add(history.clone());
                        let _ = editor.add_history_entry(history.as_str());
                    }
```
to:
```rust
                    if !history.trim().is_empty() {
                        shell.record_history(history.clone());
                        let _ = editor.add_history_entry(history.as_str());
                    }
```

- [ ] **Step 5: Replace the six save sites** — `src/shell.rs`. Every `shell.history.save();` (at ~298, 314, 341, 356, 377, 385) becomes `shell.save_history();`. Use `grep -n "shell.history.save()" src/shell.rs` to find them all; replace each. (They are each preceded by a `shell` binding in scope; `save_history(&self)` works on it.)

- [ ] **Step 6: Add the post-rc re-cap** — `src/shell.rs`, INSIDE the existing rc block (lines ~293-300). That block already holds `let mut shell = shell_cell.borrow_mut();` and an `if let Some(exit_code) = maybe_source_rc_file(&mut shell, &opts) { … return exit_code; }`. Do NOT take a new `shell_cell.borrow()`/`borrow_mut()` (that would double-borrow-panic while `shell` is live). Add the re-cap using the EXISTING `shell` binding, right after the `if let` (so it runs on the non-early-return path) and before the block's closing brace:
```rust
    {
        let mut shell = shell_cell.borrow_mut();
        if let Some(exit_code) = maybe_source_rc_file(&mut shell, &opts) {
            crate::traps::fire_exit_trap(&mut shell);
            shell.hangup_jobs();
            shell.save_history();   // (updated from shell.history.save() in Step 5)
            return exit_code;
        }
        // v139: re-apply the in-memory cap now that ~/.huckrc may have set HISTSIZE
        // (history was loaded before rc). Nets out to bash's rc-then-history effect.
        let cap = shell.resolve_histsize();
        Rc::make_mut(&mut shell.history).set_max(cap);
    }
```
(`let cap = shell.resolve_histsize();` ends its immutable borrow before the `Rc::make_mut(&mut shell.history)` mutable borrow — sequential, no conflict.)

- [ ] **Step 7: Build, run integration tests + history/resolve unit tests**

Run: `cargo build 2>&1 | tail -5`
Run: `cargo test --test histsize_integration 2>&1 | tail -15` → all 6 PASS.
Run: `cargo test --lib history resolve_hist 2>&1 | tail -10` → still green.
Run: `cargo clippy --all-targets 2>&1 | tail -8` → no new warnings.

- [ ] **Step 8: Commit**

```bash
git add src/shell_state.rs src/shell.rs tests/histsize_integration.rs
git commit -m "$(printf 'feat: honor HISTSIZE/HISTFILESIZE in the REPL history path (M-59)\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 4: Docs — resolve M-59

**Files:**
- Modify: `docs/bash-divergences.md`

- [ ] **Step 1: Delete the M-59 entry**

Find the `M-59` bullet under Tier 2 "History" (`grep -n "M-59" docs/bash-divergences.md`) and DELETE the entire bullet (it reads: `**M-59: HISTSIZE / HISTFILESIZE env vars** — [deferred] medium. huck: compile-time HISTORY_MAX = 1000. bash: reads env vars.`).

- [ ] **Step 2: Decrement the Tier-2 count**

In the Summary table, change the "Missing features (Tier 2)" count from `21` to `20`. (Verify the current number first with `grep -n "Missing features (Tier 2)" docs/bash-divergences.md` — set it to one less than whatever it currently reads.)

- [ ] **Step 3: Commit**

```bash
git add docs/bash-divergences.md
git commit -m "$(printf 'docs: resolve M-59 (HISTSIZE/HISTFILESIZE) — Tier-2 21->20\n\nCo-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>')"
```

---

### Task 5: Full regression

**Files:** none (verification only)

- [ ] **Step 1: Full test suite**

Run: `cargo test 2>&1 | tail -30`
Expected: ALL pass (baseline after v138 was 3036 tests; v139 adds ~5 History unit tests + 2 resolve unit tests + 6 integration tests). Zero failures. Paste any failure.

- [ ] **Step 2: History + interactive PTY suites (the paths v139 touches)**

Run: `cargo test history 2>&1 | tail -15` (history unit + integration + expansion tests).
Run: `cargo test --test pty_interactive 2>&1 | tail -10` (history is loaded/saved in the interactive path — must not regress; graceful skip without a PTY is acceptable).

- [ ] **Step 3: All bash-diff harnesses**

Run: `cargo build 2>&1 | tail -2 && for f in tests/scripts/*_diff_check.sh; do printf '== %s == ' "$f"; bash "$f" | tail -1; done`
Expected: every harness ends with `Fail: 0` (no new harness this iteration — non-interactive bash records no history, so a histfile bash-diff is N/A).

- [ ] **Step 4: Clippy**

Run: `cargo clippy --all-targets 2>&1 | tail -8`
Expected: clean.

- [ ] **Step 5: Manual interactive sanity (optional, recommended)**

Build: `cargo build --release 2>&1 | tail -2`. Interactively (or describe to the controller): `HISTSIZE=3` then run 5 commands and confirm `history` lists 3; set `HISTFILESIZE=2`, exit, and confirm the histfile has 2 lines. (Compare to bash if convenient — but note non-interactive bash records nothing.)

- [ ] **Step 6: Commit (only if a verification-driven fix was needed)**

If Steps 1-4 surfaced a real issue, make the SMALLEST fix, re-run, commit with the trailer. Otherwise no commit — verification only.

---

## Notes for the implementer
- **Read the vars from the variable table** (`self.lookup_var`), never `std::env::var` — HISTSIZE/HISTFILESIZE are usually non-exported.
- **`None` = unlimited / no truncation** is the load-bearing convention; `Some(0)` = empty (a real cap of zero, distinct from `None`).
- **`save()` is kept** as `save_capped(self.max)` for back-compat of direct callers/tests; the production save path goes through `Shell::save_history` → `save_capped(resolve_histfilesize())`.
- **Do not add a bash-diff harness** — non-interactive bash records no history, so a histfile diff is not comparable; the integration tests assert huck's behavior directly.
- **histappend is out of scope** — `save_capped` overwrites from the in-memory list; a file growing beyond the in-memory list (bash `shopt -s histappend`) is M-46 territory.
