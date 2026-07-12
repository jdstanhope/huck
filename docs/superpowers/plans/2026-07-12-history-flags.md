# v284 — `history` builtin: `history N` (#7) + `-d`/`-w`/`-r`/`-a` (#6) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extend `history` to match bash: `history N` prints the last N entries, and `-d`/`-w`/`-r`/`-a` delete/write/read/append (huck currently has only `-c`).

**Architecture:** Add a `-a` marker field + six methods to the `History` struct (data layer, unit-tested), then rewrite `builtin_history` as a small option parser dispatching to them, and fix the row renderer to bash's two-space format. A bash-diff harness populates history with `history -r` (works non-interactively) and compares byte-for-byte.

**Tech Stack:** Rust (huck-engine `history` module + `builtins`), bash diff-check harness.

## Global Constraints

- **Files:** `crates/huck-engine/src/history.rs` (struct + methods + unit tests), `crates/huck-engine/src/builtins.rs` (`builtin_history`), `crates/huck-engine/src/builtins/history_tests.rs` (builtin tests), and a new `tests/scripts/history_diff_check.sh`.
- **Row format:** every listing row is `format!("{number:>5}  {command}")` — 5-wide right-justified number, **two spaces**, command (bash `%5d  %s`). Replaces the current `\t`. No test pins the tab (verified).
- **Run tests per-crate, single-threaded** (box OOMs on `--workspace`): `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`. Build the binary with `cargo build -p huck` (+ `--release` before the sweep).
- **`cargo fmt --all` before each commit**; CI enforces `--check`.
- **Every commit** ends with `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- **Branch `v284-history-flags`; do not push to main / do not merge.** PR (`Closes #6`, `Closes #7`) is for the user.
- `History` fields today: `entries: Vec<String>`, `base_number: usize`, `max: Option<usize>`, `file: Option<PathBuf>`. Absolute number of `entries[i]` = `base_number + i`. `escape_for_save`/`unescape_for_load` are private module fns.

---

### Task 1: `History` — `-a` marker field + six methods + unit tests

**Files:**
- Modify: `crates/huck-engine/src/history.rs` (struct ~32, `new` ~40, `clear` ~116, `enforce_max` ~57; add methods; test module ~481)

**Interfaces:**
- Produces (consumed by Task 2): `tail`, `delete`, `delete_range`, `write_all_to`, `append_new_to`, `read_append_from`, and the `unwritten_start` field.

- [ ] **Step 1: Add the `unwritten_start` field to the struct**

In `pub struct History` (line ~32), add after `file`:
```rust
    /// Index of the first entry not yet written to a file via `-a`/`-w`
    /// (reset on `clear`/session start). `history -a` appends
    /// `entries[unwritten_start..]` then advances this to `entries.len()`.
    unwritten_start: usize,
```

- [ ] **Step 2: Initialize it in `new`, reset in `clear`, decrement on eviction**

In `new()` add `unwritten_start: 0,`. In `clear()` add `self.unwritten_start = 0;` (after resetting `base_number`). In `enforce_max()`, inside the eviction `while` loop, after `self.base_number += 1;` add:
```rust
            self.unwritten_start = self.unwritten_start.saturating_sub(1);
```

- [ ] **Step 3: Fix the ~21 test-module struct literals (they now miss a field)**

Every `History { … }` literal in the `#[cfg(test)] mod tests` block (lines ~481-790) now fails to compile (missing `unwritten_start`). Use the compiler as the checklist — it reports each site precisely:
```bash
cargo build -p huck-engine --tests 2>&1 | grep -E 'missing field|history.rs:' | head -40
```
Add `unwritten_start: 0,` to each reported `History { … }` literal (they set `entries`/`base_number`/`max`/`file`; add the fifth field). Re-run until the missing-field errors are gone. This is the ONLY change to those literals — do not alter their other fields. Prefer this compiler-guided pass over a blind `sed`/`awk`, which can mis-indent or hit a stray `file:`.

- [ ] **Step 4: Add the six methods (inside `impl History`)**

```rust
    /// The last `n` entries as `(absolute_number, command)`, oldest-first.
    /// `n == 0` yields nothing; `n` past the length yields the whole list.
    pub fn tail(&self, n: usize) -> impl Iterator<Item = (usize, &str)> {
        let start = self.entries.len().saturating_sub(n);
        let base = self.base_number;
        self.entries[start..]
            .iter()
            .enumerate()
            .map(move |(i, s)| (base + start + i, s.as_str()))
    }

    /// Deletes the entry with absolute display `number`. Returns `false` if it
    /// is outside `base_number..=last_number`. Remaining entries renumber
    /// contiguously (the base+index model). Adjusts the `-a` marker if an entry
    /// before it is removed.
    pub fn delete(&mut self, number: usize) -> bool {
        if number < self.base_number {
            return false;
        }
        let idx = number - self.base_number;
        if idx >= self.entries.len() {
            return false;
        }
        self.entries.remove(idx);
        if idx < self.unwritten_start {
            self.unwritten_start -= 1;
        }
        true
    }

    /// Deletes every entry with absolute number in `start..=end` (inclusive);
    /// returns the count removed. Reversed/empty range removes nothing.
    /// Deletes high→low so lower indices stay valid.
    pub fn delete_range(&mut self, start: usize, end: usize) -> usize {
        if end < start {
            return 0;
        }
        let mut removed = 0;
        for number in (start..=end).rev() {
            if self.delete(number) {
                removed += 1;
            }
        }
        removed
    }

    /// `history -w`: writes the WHOLE list to `path` (truncating), one escaped
    /// entry per line, and marks everything written.
    pub fn write_all_to(&mut self, path: &std::path::Path) -> std::io::Result<()> {
        let mut out = String::new();
        for e in &self.entries {
            out.push_str(&escape_for_save(e));
            out.push('\n');
        }
        std::fs::write(path, out)?;
        self.unwritten_start = self.entries.len();
        Ok(())
    }

    /// `history -a`: appends only the not-yet-written entries
    /// (`entries[unwritten_start..]`) to `path` (append mode, create if absent),
    /// then advances the marker.
    pub fn append_new_to(&mut self, path: &std::path::Path) -> std::io::Result<()> {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        for e in &self.entries[self.unwritten_start..] {
            writeln!(f, "{}", escape_for_save(e))?;
        }
        self.unwritten_start = self.entries.len();
        Ok(())
    }

    /// `history -r`: reads `path`, unescapes, APPENDS its lines to the list,
    /// enforces the cap, and marks all-written (read lines are on-disk origin,
    /// not re-appended by a later `-a`).
    pub fn read_append_from(&mut self, path: &std::path::Path) -> std::io::Result<()> {
        let contents = std::fs::read_to_string(path)?;
        for line in contents.lines() {
            self.entries.push(unescape_for_load(line));
        }
        self.enforce_max();
        self.unwritten_start = self.entries.len();
        Ok(())
    }
```

- [ ] **Step 5: Add unit tests (test module)**

```rust
    #[test]
    fn tail_returns_last_n_with_numbers() {
        let mut h = empty();
        for c in ["a", "b", "c", "d"] {
            h.add(c.to_string());
        }
        assert_eq!(h.tail(2).collect::<Vec<_>>(), vec![(3, "c"), (4, "d")]);
        assert_eq!(h.tail(0).count(), 0);
        assert_eq!(h.tail(99).collect::<Vec<_>>(), vec![(1, "a"), (2, "b"), (3, "c"), (4, "d")]);
    }

    #[test]
    fn delete_renumbers_and_bounds() {
        let mut h = empty();
        for c in ["a", "b", "c"] {
            h.add(c.to_string());
        }
        assert!(h.delete(2)); // remove "b"
        assert_eq!(h.entries().collect::<Vec<_>>(), vec![(1, "a"), (2, "c")]);
        assert!(!h.delete(9)); // out of range
        assert!(h.delete(1)); // remove "a"
        assert_eq!(h.entries().collect::<Vec<_>>(), vec![(1, "c")]);
    }

    #[test]
    fn delete_range_inclusive_and_reversed_noop() {
        let mut h = empty();
        for c in ["a", "b", "c", "d", "e"] {
            h.add(c.to_string());
        }
        assert_eq!(h.delete_range(2, 3), 2); // remove b,c
        assert_eq!(h.entries().collect::<Vec<_>>(), vec![(1, "a"), (2, "d"), (3, "e")]);
        assert_eq!(h.delete_range(5, 2), 0); // reversed
    }

    #[test]
    fn write_read_append_roundtrip_and_marker() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hf");
        let mut h = empty();
        for c in ["a", "b"] {
            h.add(c.to_string());
        }
        h.write_all_to(&path).unwrap(); // marker -> 2
        // append_new_to now writes nothing (all written)
        let ap = dir.path().join("ap");
        h.append_new_to(&ap).unwrap();
        assert_eq!(std::fs::read_to_string(&ap).unwrap(), "");
        // add a new one; append_new_to writes only it
        h.add("c".to_string());
        h.append_new_to(&ap).unwrap();
        assert_eq!(std::fs::read_to_string(&ap).unwrap(), "c\n");
        // read_append_from appends file lines
        let mut h2 = empty();
        h2.read_append_from(&path).unwrap();
        assert_eq!(h2.entries().collect::<Vec<_>>(), vec![(1, "a"), (2, "b")]);
    }

    #[test]
    fn eviction_decrements_unwritten_marker() {
        let mut h = empty();
        for c in ["a", "b", "c"] {
            h.add(c.to_string());
        }
        h.write_all_to(&std::path::PathBuf::from("/dev/null")).unwrap(); // marker -> 3
        h.set_max(Some(1)); // evict a,b → marker saturating to 1
        h.add("d".to_string());
        let ap = tempfile::tempdir().unwrap().path().join("ap");
        h.append_new_to(&ap).unwrap();
        assert_eq!(std::fs::read_to_string(&ap).unwrap(), "d\n"); // only the truly-new entry
    }
```

- [ ] **Step 6: Run + commit**

```bash
cargo fmt --all && cargo fmt --all --check
cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1
git add crates/huck-engine/src/history.rs
git commit -m "feat(history): add -a marker + tail/delete/delete_range/read/write/append methods (#6, #7)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```
Expected: whole huck-engine suite green (existing literals compile after the field add; new tests pass).

---

### Task 2: Rewrite `builtin_history` — parse & dispatch the flags; fix row format

**Files:**
- Modify: `crates/huck-engine/src/builtins.rs` (`builtin_history` ~5275)
- Test: `crates/huck-engine/src/builtins/history_tests.rs`

**Interfaces:**
- Consumes Task 1's `History` methods; `shell.history` (an `Rc<History>`, mutate via `Rc::make_mut(&mut shell.history)`); `crate::bash_io_error`; the resolved histfile is `History::file` — add a small accessor if needed, or route no-file ops through the existing `save_capped`/`load` (which use `self.file`). Prefer explicit-path methods and read the default path from `History` via a new `pub fn file_path(&self) -> Option<&std::path::Path>`.

- [ ] **Step 1: Add a `file_path` accessor to `History`** (history.rs, `impl History`):
```rust
    /// The resolved default history file (`$HISTFILE` / `~/.huck_history`), if any.
    pub fn file_path(&self) -> Option<&std::path::Path> {
        self.file.as_deref()
    }
```

- [ ] **Step 2: Write the failing builtin tests**

Add to `crates/huck-engine/src/builtins/history_tests.rs`:
```rust
#[test]
fn history_n_prints_last_n() {
    let mut shell = Shell::new();
    for c in ["a", "b", "c"] {
        Rc::make_mut(&mut shell.history).add(c.to_string());
    }
    let mut out: Vec<u8> = Vec::new();
    let outcome = run_builtin("history", &["2".to_string()], &mut out, &mut std::io::stderr(), &mut shell);
    assert!(matches!(outcome, ExecOutcome::Continue(0)));
    assert_eq!(String::from_utf8(out).unwrap(), "    2  b\n    3  c\n");
}

#[test]
fn history_n_zero_prints_nothing() {
    let mut shell = Shell::new();
    Rc::make_mut(&mut shell.history).add("a".to_string());
    let mut out: Vec<u8> = Vec::new();
    run_builtin("history", &["0".to_string()], &mut out, &mut std::io::stderr(), &mut shell);
    assert_eq!(out, b"");
}

#[test]
fn history_d_deletes_and_out_of_range_errors() {
    let mut shell = Shell::new();
    for c in ["a", "b", "c"] {
        Rc::make_mut(&mut shell.history).add(c.to_string());
    }
    let mut err: Vec<u8> = Vec::new();
    let ok = run_builtin("history", &["-d".to_string(), "2".to_string()], &mut Vec::new(), &mut err, &mut shell);
    assert!(matches!(ok, ExecOutcome::Continue(0)));
    assert_eq!(shell.history.entries().collect::<Vec<_>>(), vec![(1, "a"), (2, "c")]);
    let bad = run_builtin("history", &["-d".to_string(), "9".to_string()], &mut Vec::new(), &mut err, &mut shell);
    assert!(matches!(bad, ExecOutcome::Continue(1)));
    assert!(String::from_utf8(err).unwrap().contains("history position out of range"));
}

#[test]
fn history_d_negative_and_range() {
    let mut shell = Shell::new();
    for c in ["a", "b", "c", "d", "e"] {
        Rc::make_mut(&mut shell.history).add(c.to_string());
    }
    run_builtin("history", &["-d".to_string(), "-1".to_string()], &mut Vec::new(), &mut std::io::stderr(), &mut shell);
    assert_eq!(shell.history.last(), Some("d")); // e removed
    run_builtin("history", &["-d".to_string(), "2-3".to_string()], &mut Vec::new(), &mut std::io::stderr(), &mut shell);
    assert_eq!(shell.history.entries().collect::<Vec<_>>(), vec![(1, "a"), (2, "d")]);
}
```

Run: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1 history_n_ history_d_` → FAIL (current builtin rejects these).

- [ ] **Step 3: Rewrite `builtin_history`**

Replace the body of `builtin_history` with a parser handling: no-arg list; bare numeric `N` (list last N); `-c` (clear); `-d <operand>` (delete: single / negative `-K` / range `A-B`); `-w`/`-r`/`-a [file]` (file ops); `-p`/`-s`/`-n` → "not yet implemented" rc 1; unknown `-X` → invalid-option usage rc 2. Full replacement:

```rust
fn builtin_history(
    args: &[String],
    out: &mut dyn Write,
    err: &mut dyn Write,
    shell: &mut Shell,
) -> ExecOutcome {
    // Resolve a -d operand (single offset, negative -K, or the two bounds of a
    // range A-B) to an absolute history number. Negative K counts from the end.
    fn resolve_offset(shell: &Shell, s: &str) -> Option<usize> {
        let last = shell.history.last_number()?;
        if let Some(k) = s.strip_prefix('-') {
            let k: usize = k.parse().ok().filter(|&k| k >= 1)?;
            last.checked_sub(k - 1)
        } else {
            s.parse::<usize>().ok()
        }
    }

    let mut idx = 0;
    // ---- options ----
    while idx < args.len() {
        let a = &args[idx];
        if a == "--" {
            idx += 1;
            break;
        }
        match a.as_str() {
            "-c" => {
                Rc::make_mut(&mut shell.history).clear();
                idx += 1;
            }
            "-d" => {
                let Some(operand) = args.get(idx + 1) else {
                    crate::sh_error_to!(shell, err, None, "history: -d: option requires an argument");
                    return ExecOutcome::Continue(1);
                };
                // Range iff a '-' appears AFTER the first char (so a leading
                // negative sign on a single offset isn't mistaken for a range).
                let split = operand[1..].find('-').map(|i| i + 1);
                let range = match split {
                    Some(i) => Some((&operand[..i], &operand[i + 1..])),
                    None => None,
                };
                if let Some((sa, sb)) = range {
                    match (resolve_offset(shell, sa), resolve_offset(shell, sb)) {
                        (Some(a), Some(b)) => {
                            Rc::make_mut(&mut shell.history).delete_range(a, b);
                        }
                        _ => {
                            crate::sh_error_to!(shell, err, None, "history: {operand}: history position out of range");
                            return ExecOutcome::Continue(1);
                        }
                    }
                } else {
                    match resolve_offset(shell, operand) {
                        Some(n) if Rc::make_mut(&mut shell.history).delete(n) => {}
                        _ => {
                            crate::sh_error_to!(shell, err, None, "history: {operand}: history position out of range");
                            return ExecOutcome::Continue(1);
                        }
                    }
                }
                idx += 2;
            }
            "-w" | "-r" | "-a" => {
                let flag = a.clone();
                // Optional filename operand; else the default histfile.
                let file: std::path::PathBuf = match args.get(idx + 1) {
                    Some(f) if !f.starts_with('-') => {
                        idx += 1;
                        std::path::PathBuf::from(f)
                    }
                    _ => match shell.history.file_path() {
                        Some(p) => p.to_path_buf(),
                        None => {
                            crate::sh_error_to!(shell, err, None, "history: cannot use the history file");
                            return ExecOutcome::Continue(1);
                        }
                    },
                };
                let h = Rc::make_mut(&mut shell.history);
                let res = match flag.as_str() {
                    "-w" => h.write_all_to(&file),
                    "-a" => h.append_new_to(&file),
                    _ => h.read_append_from(&file),
                };
                if let Err(e) = res {
                    crate::sh_error_to!(shell, err, None, "history: {}: {}", file.display(), crate::bash_io_error(&e));
                    return ExecOutcome::Continue(1);
                }
                idx += 1;
            }
            "-p" | "-s" | "-n" => {
                crate::sh_error_to!(shell, err, None, "history: {a}: not yet implemented");
                return ExecOutcome::Continue(1);
            }
            other if other.starts_with('-') && other.len() > 1 => {
                crate::sh_error_to!(shell, err, None, "history: {other}: invalid option");
                e!(err, "history: usage: history [-c] [-d offset] [n] or history -anrw [filename] or history -ps arg [arg...]");
                shell.builtin_usage_error = Some(2);
                return ExecOutcome::Continue(2);
            }
            _ => break, // a non-option operand (the N count)
        }
    }

    // ---- trailing operand: the listing count N (only when no option consumed it) ----
    let rest = &args[idx..];
    match rest.first().map(|s| s.as_str()) {
        None => {
            // No numeric operand: if any option ran, we're done (rc 0); else list all.
            if idx == 0 {
                for (number, command) in shell.history.entries() {
                    if writeln!(out, "{number:>5}  {command}").is_err() {
                        return ExecOutcome::Continue(1);
                    }
                }
            }
            ExecOutcome::Continue(0)
        }
        Some(n_str) => match n_str.parse::<usize>() {
            Ok(n) => {
                for (number, command) in shell.history.tail(n) {
                    if writeln!(out, "{number:>5}  {command}").is_err() {
                        return ExecOutcome::Continue(1);
                    }
                }
                ExecOutcome::Continue(0)
            }
            Err(_) => {
                crate::sh_error_to!(shell, err, None, "history: {n_str}: invalid option");
                e!(err, "history: usage: history [-c] [-d offset] [n] or history -anrw [filename] or history -ps arg [arg...]");
                shell.builtin_usage_error = Some(2);
                ExecOutcome::Continue(2)
            }
        },
    }
}
```

Note: when both an option and a trailing `N` appear (`history -c 5`), bash lists after the option; the code above lists `N` from the (now-empty) list → nothing, which matches bash. When only options ran and no `N`, don't re-list.

- [ ] **Step 4: Run tests + full suite**

Run: `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1`
Expected: the new builtin tests pass; the pre-existing `history_lists_numbered_entries`/`history_dash_c_clears`/`history_invalid_option_errors` still pass (the invalid-option one may need its expected message/rc updated to the new usage text — update it to match if so, keeping it a real assertion).

- [ ] **Step 5: fmt + commit**

```bash
cargo fmt --all && cargo fmt --all --check
git add crates/huck-engine/src/builtins.rs crates/huck-engine/src/builtins/history_tests.rs
git commit -m "feat(history): history N + -d/-w/-r/-a flags; bash two-space row format (#6, #7)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Bash-diff harness + sweep

**Files:**
- Create: `tests/scripts/history_diff_check.sh` (mode 0755)

**Interfaces:**
- Consumes the built `target/debug/huck`.

- [ ] **Step 1: Write the harness**

Use **file-arg execution** (write each fragment to a script file, run `bash file` / `$HUCK_BIN file`) — huck history-expands PIPED stdin (L-27), and these fragments contain `history`/`!`; file-arg avoids that. Set `HISTFILE=/dev/null` inside each fragment to isolate. Populate via `history -r <fixture>`.

```bash
#!/usr/bin/env bash
# v284: byte-identical bash<->huck for `history N` (#7) and -d/-w/-r/-a (#6).
# File-arg execution (L-27: huck history-expands piped stdin). HISTFILE=/dev/null
# isolates; history is populated with `history -r <fixture>` (works non-interactively).
set -u
HUCK_BIN="${HUCK_BIN:-$(pwd)/target/debug/huck}"
[[ -x "$HUCK_BIN" ]] || { echo "build huck first: $HUCK_BIN" >&2; exit 1; }
PASS=0; FAIL=0
WORK=$(mktemp -d); trap 'rm -rf "$WORK"' EXIT
printf 'cmd-one\ncmd-two\ncmd-three\ncmd-four\n' > "$WORK/fix"
norm() { sed -E 's#^([^:]*/)?(bash|huck): (line [0-9]+: )?##'; }
check() {
    local label="$1" frag="$2" b h
    printf '%s\n' "$frag" > "$WORK/frag.sh"
    b=$(cd "$WORK" && bash ./frag.sh 2>&1; echo "rc=$?"); b=$(printf '%s\n' "$b" | norm)
    h=$(cd "$WORK" && "$HUCK_BIN" ./frag.sh 2>&1; echo "rc=$?"); h=$(printf '%s\n' "$h" | norm)
    if [[ "$b" == "$h" ]]; then printf 'PASS: %s\n' "$label"; PASS=$((PASS+1))
    else printf 'FAIL: %s\n' "$label"; diff <(echo "$b") <(echo "$h") | sed 's/^/    /'; FAIL=$((FAIL+1)); fi
}
POP='HISTFILE=/dev/null; history -c; history -r fix;'
check "list all format"   "$POP history"
check "history 2"         "$POP history 2"
check "history 0"         "$POP history 0"
check "history 99"        "$POP history 99"
check "delete single"     "$POP history -d 2; history"
check "delete negative"   "$POP history -d -1; history"
check "delete range"      "$POP history -d 2-3; history"
check "delete oob err"    "$POP history -d 9; echo rc=\$?"
check "delete nonnum err" "$POP history -d abc; echo rc=\$?"
check "read append"       "$POP history -r fix; history"
check "write file"        "$POP history -w out; cat out"
check "append after read" "$POP : > ap; history -a ap; echo \"ap=[\$(cat ap)]\""
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

- [ ] **Step 2: Run the harness**

```bash
cargo build -p huck
chmod +x tests/scripts/history_diff_check.sh
HUCK_BIN="$(pwd)/target/debug/huck" bash tests/scripts/history_diff_check.sh; echo "exit=$?"
```
Expected: every case `PASS:`, `Total: 12, Pass: 12, Fail: 0`, `exit=0`. If any case reveals a real bash-behavior mismatch (e.g. a `-d` error-message wording), fix `builtin_history` to match bash and re-run — the harness is ground truth.

- [ ] **Step 3: Full diff-check sweep**

```bash
cargo build -p huck && cargo build --release -p huck
tests/scripts/run_diff_checks.sh; echo "exit=$?"
```
Expected: `Diff-check sweep: 182 passed, 0 failed`, `exit=0` (181 prior + the new harness).

- [ ] **Step 4: fmt (no rust changes) + commit**

```bash
git add tests/scripts/history_diff_check.sh
git commit -m "test: history_diff_check.sh — byte-identical history N/-d/-w/-r/-a vs bash (#6, #7)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Final verification (after all tasks)

- [ ] `cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1` green.
- [ ] `tests/scripts/history_diff_check.sh` 12/12; full sweep 182/0.
- [ ] Spot-check vs bash:
```bash
HUCK="$(pwd)/target/debug/huck"
printf 'HISTFILE=/dev/null; history -c; history -r <(printf "a\\nb\\nc\\n"); history 2\n' > /tmp/hv.sh
"$HUCK" /tmp/hv.sh   # ->     2  b /     3  c   (two spaces)
```

## Notes for the whole-branch review

- The `-a` genuinely-session-added path is unit-tested (Task 1 Step 5), not diff-tested (needs `history -s`, out of scope).
- Row format changed `\t`→two spaces — a bundled latent-divergence fix; confirm no other output path relied on the tab.
- Both #6 and #7 close on merge; no `docs/bash-divergences.md` change.
- Out of scope (rejected, not silent): `-p`/`-s`/`-n`.
