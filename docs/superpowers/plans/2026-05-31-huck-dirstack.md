# huck v63 — `pushd`/`popd`/`dirs` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship bash's directory-stack builtins
(`pushd`/`popd`/`dirs`) with full flag set including `+N`/`-N`
rotation.

**Architecture:** New `Shell.dir_stack: Vec<PathBuf>` field
(top = index 0). All three builtins sync `stack[0]` from
current `$PWD` first, then operate. `pushd`/`popd` reuse
`builtin_cd` (promoted to `pub(crate)`). Pure helpers
(`parse_signed_index`, `dir_display`, `print_stack`) are
unit-testable in isolation.

**Tech Stack:** Rust. No new deps.

**Spec:** `docs/superpowers/specs/2026-05-31-huck-dirstack-design.md`

**Branch:** `v63-dirstack`.

**Commit trailer:**

```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Preamble: Create the working branch

- [ ] **Step P.1**

```bash
git checkout main
git pull --ff-only
git checkout -b v63-dirstack
```

Spec + this plan are committed before Task 1.

---

## Task 1: Foundation + 3 builtins + 10 unit tests

**Files:**
- Modify `src/shell_state.rs` — add `dir_stack: Vec<PathBuf>`
  field + empty init.
- Modify `src/builtins.rs` — promote `builtin_cd` to
  `pub(crate)`; add helpers + 3 new builtins + dispatch +
  BUILTIN_NAMES + `mod dirstack_tests`.

### Step 1.1: Add `dir_stack` field to Shell

In `src/shell_state.rs`, find the `pub struct Shell { ... }`
block. Add a new field near other Vec-stack fields:

```rust
    /// Directory stack maintained by the `pushd`/`popd`/`dirs`
    /// builtins. Top is index 0 — always synced with `$PWD` at
    /// the top of each pushd/popd/dirs call.
    pub dir_stack: Vec<std::path::PathBuf>,
```

In `Shell::new`, add the initializer:

```rust
            dir_stack: Vec::new(),
```

- [ ] **Step 1.1**

### Step 1.2: Build

`cargo build`. Expected: clean.

- [ ] **Step 1.2**

### Step 1.3: Promote `builtin_cd` to `pub(crate)`

Find `builtin_cd` in `src/builtins.rs` (currently `fn`). Change
the signature to:

```rust
pub(crate) fn builtin_cd(args: &[String], shell: &mut Shell) -> ExecOutcome {
```

No other changes — just the visibility.

- [ ] **Step 1.3**

### Step 1.4: Append `"pushd"`, `"popd"`, `"dirs"` to `BUILTIN_NAMES`

Current (post-v62), `src/builtins.rs:18-26`. Append the three
names:

```rust
pub const BUILTIN_NAMES: &[&str] = &[
    "cd", "exit", "pwd", "echo", "export", "unset", "jobs",
    "wait", "fg", "bg", "kill", "disown", "history", "test", "[",
    "break", "continue", "return", "trap", "alias", "unalias",
    "set", "shift", ".", "source", "local",
    ":", "true", "false", "command",
    "readonly", "read", "printf", "type", "hash",
    "pushd", "popd", "dirs",
];
```

None of these are in `is_special_builtin` (regular bash builtins).

- [ ] **Step 1.4**

### Step 1.5: Add pure helpers

Insert near other resolver-style helpers (or before
`builtin_pushd`):

```rust
fn parse_signed_index(s: &str, stack_len: usize) -> Result<usize, String> {
    let (sign_plus, digits) = if let Some(d) = s.strip_prefix('+') {
        (true, d)
    } else if let Some(d) = s.strip_prefix('-') {
        (false, d)
    } else {
        return Err(format!("{s}: not a +N or -N specifier"));
    };
    let n: usize = digits
        .parse()
        .map_err(|_| format!("{s}: invalid number"))?;
    if n >= stack_len {
        return Err(format!("{s}: directory stack index out of range"));
    }
    Ok(if sign_plus { n } else { stack_len - 1 - n })
}

fn dir_display(path: &std::path::Path, shell: &Shell, collapse: bool) -> String {
    let s = path.display().to_string();
    if !collapse {
        return s;
    }
    let home = shell
        .lookup_var("HOME")
        .or_else(|| std::env::var("HOME").ok())
        .unwrap_or_default();
    if home.is_empty() {
        return s;
    }
    if s == home {
        return "~".to_string();
    }
    if let Some(rest) = s.strip_prefix(&format!("{home}/")) {
        return format!("~/{rest}");
    }
    s
}

fn sync_stack_top(shell: &mut Shell) {
    let cwd_str = shell
        .lookup_var("PWD")
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .map(|p| p.display().to_string())
        })
        .unwrap_or_default();
    let p = std::path::PathBuf::from(cwd_str);
    if shell.dir_stack.is_empty() {
        shell.dir_stack.push(p);
    } else {
        shell.dir_stack[0] = p;
    }
}

fn print_stack(
    out: &mut dyn std::io::Write,
    shell: &Shell,
    collapse: bool,
    per_line: bool,
    numbered: bool,
) -> ExecOutcome {
    if per_line {
        for (i, p) in shell.dir_stack.iter().enumerate() {
            let disp = dir_display(p, shell, collapse);
            if numbered {
                let _ = writeln!(out, "{i:>2}  {disp}");
            } else {
                let _ = writeln!(out, "{disp}");
            }
        }
    } else {
        let parts: Vec<String> = shell
            .dir_stack
            .iter()
            .map(|p| dir_display(p, shell, collapse))
            .collect();
        let _ = writeln!(out, "{}", parts.join(" "));
    }
    ExecOutcome::Continue(0)
}
```

- [ ] **Step 1.5**

### Step 1.6: Add `builtin_pushd`

Full code in spec §"`builtin_pushd`". Key paths:
- No args + stack ≥ 2 → swap stack[0]/stack[1] + cd to new top + print.
- No args + stack < 2 → "no other directory" + exit 1.
- `+N`/`-N` arg → `rotate_left(idx)` + cd + print. On cd failure, `rotate_right(idx)` to undo.
- Other arg → cd to it + insert new PWD at front + print.

Detection of `+N`/`-N` form: an arg that starts with `+` or starts with `-` and has a digit right after. Otherwise treat as a directory path. (`-DIR` as a literal path is a weird edge case; bash treats anything starting with `-` followed by non-digit as a flag/error — for v63 we'll require explicit `--` separator before such paths, defer the corner case.)

- [ ] **Step 1.6**

### Step 1.7: Add `builtin_popd`

Full code in spec §"`builtin_popd`". Key paths:
- Stack ≤ 1 → "directory stack empty" + exit 1.
- No args → remove top + cd to new top + print.
- `+N`/`-N` → remove that entry. If idx == 0, cd to new top; else no cd. Print.
- Other arg → "invalid argument" + exit 1.

- [ ] **Step 1.7**

### Step 1.8: Add `builtin_dirs`

Full code in spec §"`builtin_dirs`". Flag parsing:
- `-c` → clear (truncate to first entry).
- `-l` → no `~` collapse.
- `-p` → per-line (not numbered).
- `-v` → per-line + numbered.
- `+N`/`-N` → print just that entry.
- Other `-X` → invalid option + exit 2.

Flags don't cluster in `dirs` — bash treats `-l` and `-p` as separate args. Match.

- [ ] **Step 1.8**

### Step 1.9: Add dispatch arms

In `run_builtin`'s match block:

```rust
"pushd" => builtin_pushd(args, out, shell),
"popd" => builtin_popd(args, out, shell),
"dirs" => builtin_dirs(args, out, shell),
```

Position near each other.

- [ ] **Step 1.9**

### Step 1.10: Build

`cargo build`. Expected: clean.

- [ ] **Step 1.10**

### Step 1.11: Append `mod dirstack_tests` (10 tests)

At end of `src/builtins.rs`:

```rust
#[cfg(test)]
mod dirstack_tests {
    use super::*;
    use crate::shell_state::Shell;
    use std::path::PathBuf;

    // ── parse_signed_index ────────────────────────────────────

    #[test]
    fn parse_signed_index_plus() {
        assert_eq!(parse_signed_index("+0", 10).unwrap(), 0);
        assert_eq!(parse_signed_index("+2", 10).unwrap(), 2);
        assert_eq!(parse_signed_index("+5", 10).unwrap(), 5);
    }

    #[test]
    fn parse_signed_index_minus() {
        // length 10: -0 = last (9); -1 = 8; -9 = 0.
        assert_eq!(parse_signed_index("-0", 10).unwrap(), 9);
        assert_eq!(parse_signed_index("-1", 10).unwrap(), 8);
        assert_eq!(parse_signed_index("-9", 10).unwrap(), 0);
    }

    #[test]
    fn parse_signed_index_out_of_range() {
        assert!(parse_signed_index("+10", 10).is_err());
        assert!(parse_signed_index("-10", 10).is_err());
    }

    #[test]
    fn parse_signed_index_invalid() {
        assert!(parse_signed_index("+abc", 10).is_err());
    }

    #[test]
    fn parse_signed_index_no_sign() {
        assert!(parse_signed_index("2", 10).is_err());
    }

    // ── dir_display ───────────────────────────────────────────

    #[test]
    fn dir_display_no_home_unchanged() {
        let mut shell = Shell::new();
        shell.set("HOME", String::new());
        // Also clear process env to be safe.
        let saved = std::env::var("HOME").ok();
        unsafe { std::env::remove_var("HOME"); }
        let out = dir_display(&PathBuf::from("/etc"), &shell, true);
        unsafe {
            if let Some(h) = saved { std::env::set_var("HOME", h); }
        }
        assert_eq!(out, "/etc");
    }

    #[test]
    fn dir_display_home_match_collapses() {
        let mut shell = Shell::new();
        shell.set("HOME", "/h/me".to_string());
        assert_eq!(
            dir_display(&PathBuf::from("/h/me"), &shell, true),
            "~",
        );
    }

    #[test]
    fn dir_display_home_subdir_collapses() {
        let mut shell = Shell::new();
        shell.set("HOME", "/h/me".to_string());
        assert_eq!(
            dir_display(&PathBuf::from("/h/me/x"), &shell, true),
            "~/x",
        );
    }

    #[test]
    fn dir_display_no_collapse_flag() {
        let mut shell = Shell::new();
        shell.set("HOME", "/h/me".to_string());
        assert_eq!(
            dir_display(&PathBuf::from("/h/me/x"), &shell, false),
            "/h/me/x",
        );
    }

    #[test]
    fn dir_display_unrelated_path_passes_through() {
        let mut shell = Shell::new();
        shell.set("HOME", "/h/me".to_string());
        assert_eq!(
            dir_display(&PathBuf::from("/etc/foo"), &shell, true),
            "/etc/foo",
        );
    }
}
```

If a test fails because `Shell::new` already pre-populates HOME
from the process env, the `dir_display_home_match_collapses`
test's `shell.set` overrides the existing value via the
`shell.vars` HashMap — that's correct (sets a fresh value).
The dir_display function reads `shell.lookup_var("HOME")`
first, which gets the just-set value. No race.

- [ ] **Step 1.11**

### Step 1.12: Run unit tests

```bash
cargo test --bin huck dirstack_tests
```

Expected: 10 pass.

- [ ] **Step 1.12**

### Step 1.13: Full unit suite

`cargo test --bin huck`. Expected: green.

- [ ] **Step 1.13**

### Step 1.14: Clippy

`cargo clippy --all-targets -- -D warnings`. Expected: clean.

- [ ] **Step 1.14**

### Step 1.15: Commit Task 1

```bash
git add src/shell_state.rs src/builtins.rs
git commit -m "$(cat <<'EOF'
builtin: pushd/popd/dirs (v63 task 1)

Add bash's directory-stack builtins with full bash flag set
including +N/-N rotation/index.

Foundation:
- src/shell_state.rs: new \`pub dir_stack: Vec<PathBuf>\` field
  on Shell, init empty. Top is index 0; synced with current
  \$PWD at the top of each pushd/popd/dirs call so the stack
  view always reflects the actual cwd.

Builtins (src/builtins.rs):
- builtin_pushd:
  - \`pushd DIR\`: cd to DIR; on success insert new PWD at
    stack front; print new stack. cd failure → stack unchanged
    + exit code propagated.
  - \`pushd\` (no args): swap stack[0] and stack[1]; cd to new
    top. Errors with "no other directory" if stack < 2.
  - \`pushd +N\` / \`pushd -N\`: rotate stack so the indexed
    entry becomes top; cd to it. -N counts from right (-0 =
    last). On cd failure, undo rotation. Out-of-range → exit 1.
- builtin_popd:
  - bare: remove top + cd to new top.
  - +N/-N: remove that entry. cd only if idx 0 was removed.
  - Empty stack (≤1 entry): "directory stack empty" + exit 1.
  - Bare non-flag arg → "invalid argument" + exit 1.
- builtin_dirs: flag-driven listing.
  - bare: print stack space-joined, \`~\`-collapsed.
  - -c: truncate stack to first entry (keep current dir).
  - -l: don't collapse \`~\`.
  - -p: one entry per line.
  - -v: per-line + numbered (\` 0  ~\` etc.).
  - +N/-N: print just that entry.
  - Unknown -X → exit 2.

Helpers:
- parse_signed_index: parses "+N"/"-N" into a left-indexed
  stack position. Out-of-range / invalid number → Err.
- dir_display: returns the printable form (with optional \`~\`
  collapse if path matches HOME exactly or is under HOME/).
- sync_stack_top: keeps stack[0] in sync with \$PWD.
- print_stack: emits per-flag (default = space-joined,
  -p = per-line, -v = numbered).

builtin_cd promoted from \`fn\` to \`pub(crate) fn\` so the
dirstack builtins can call it directly.

"pushd", "popd", "dirs" added to BUILTIN_NAMES; none in
is_special_builtin (POSIX regular). Three dispatch arms added.

10 unit tests in \`mod dirstack_tests\`: parse_signed_index
(plus, minus, out-of-range, invalid, no-sign) + dir_display
(no-home-unchanged, home-match-collapses, home-subdir-collapses,
no-collapse-flag, unrelated-path-passes-through).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

Stage exactly: `src/shell_state.rs src/builtins.rs`.

- [ ] **Step 1.15**

---

## Task 2: Integration tests

**Files:**
- Create `tests/dirstack_integration.rs`.

10 binary-driven tests.

### Step 2.1: Create the test file

Use the standard helper shape (returns `(stdout, stderr,
exit_code)`):

```rust
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run_capture(script: &str) -> (String, String, i32) {
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
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn pushd_dir_then_dirs() {
    let (out, _, _) = run_capture("pushd /tmp\ndirs\nexit\n");
    // After pushd, dirs output starts with /tmp.
    assert!(
        out.lines().any(|l| l.starts_with("/tmp")),
        "stdout: {out:?}",
    );
}

#[test]
fn pushd_then_popd_returns_to_origin() {
    let (out, _, _) = run_capture(
        "ORIG=$PWD\npushd /tmp\npopd\necho \"AT $PWD\"\necho \"WANT $ORIG\"\nexit\n",
    );
    let at = out.lines().find(|l| l.starts_with("AT ")).unwrap_or("?");
    let want = out.lines().find(|l| l.starts_with("WANT ")).unwrap_or("?");
    assert_eq!(
        at.strip_prefix("AT ").unwrap_or(""),
        want.strip_prefix("WANT ").unwrap_or("X"),
        "stdout: {out:?}",
    );
}

#[test]
fn pushd_no_args_swaps_top_two() {
    let (out, _, _) = run_capture(
        "pushd /tmp\npushd /var\npushd\necho \"AT $PWD\"\nexit\n",
    );
    assert!(
        out.lines().any(|l| l == "AT /tmp"),
        "expected pwd == /tmp after swap; stdout: {out:?}",
    );
}

#[test]
fn pushd_only_one_entry_errors() {
    let (_out, err, _) = run_capture("pushd\necho rc=$?\nexit\n");
    assert!(
        err.contains("no other directory"),
        "stderr: {err:?}",
    );
}

#[test]
fn popd_empty_errors() {
    let (_out, err, _) = run_capture("popd\necho rc=$?\nexit\n");
    assert!(
        err.contains("directory stack empty"),
        "stderr: {err:?}",
    );
}

#[test]
fn dirs_default_collapses_home() {
    let (out, _, _) = run_capture(
        "export HOME=$PWD\ndirs\nexit\n",
    );
    // After HOME=cwd, dirs default prints just `~`.
    assert!(
        out.lines().any(|l| l == "~"),
        "stdout: {out:?}",
    );
}

#[test]
fn dirs_v_numbered() {
    let (out, _, _) = run_capture(
        "pushd /tmp\npushd /var\ndirs -v\nexit\n",
    );
    // Expect 3 numbered lines: " 0", " 1", " 2".
    let numbered = out
        .lines()
        .filter(|l| l.trim_start().chars().next().is_some_and(|c| c.is_ascii_digit()))
        .count();
    assert!(
        numbered >= 3,
        "expected at least 3 numbered lines; stdout: {out:?}",
    );
}

#[test]
fn dirs_c_clears() {
    let (out, _, _) = run_capture(
        "pushd /tmp\ndirs -c\ndirs\nexit\n",
    );
    // After -c, dirs should print just one entry (the current dir).
    // Find the last non-empty line printed by `dirs`.
    let lines: Vec<&str> = out.lines().filter(|l| !l.is_empty()).collect();
    let last = lines.last().copied().unwrap_or("");
    assert!(
        !last.contains(' '),
        "expected single entry (no space-join); last line: {last:?}",
    );
}

#[test]
fn pushd_plus_n_rotates() {
    // Stack: [<cwd>, /var, /tmp]  (after the two pushes, /var
    // is on top because pushd inserts to front).
    // Wait — pushd inserts the NEW dir at front. So after
    // `pushd /tmp; pushd /var`: stack is [/var, /tmp, <orig>].
    // `pushd +2` rotates so index 2 (orig cwd) is top.
    let (out, _, _) = run_capture(
        "ORIG=$PWD\npushd /tmp\npushd /var\npushd +2\necho \"AT $PWD\"\necho \"WANT $ORIG\"\nexit\n",
    );
    let at = out.lines().find(|l| l.starts_with("AT ")).unwrap_or("");
    let want = out.lines().find(|l| l.starts_with("WANT ")).unwrap_or("");
    assert_eq!(
        at.strip_prefix("AT ").unwrap_or("x"),
        want.strip_prefix("WANT ").unwrap_or("y"),
        "stdout: {out:?}",
    );
}

#[test]
fn dirs_plus_index_prints_one() {
    let (out, _, _) = run_capture(
        "pushd /tmp\ndirs +1\nexit\n",
    );
    // dirs +1 prints just the second entry (the original cwd,
    // which would have ~ collapse if it matches HOME, or its
    // absolute path otherwise).
    let lines: Vec<&str> = out.lines().filter(|l| !l.is_empty()).collect();
    // Find the line just before `exit` was processed — last non-blank
    // line preceding the final shell-exit cleanup output.
    assert!(
        !lines.is_empty(),
        "expected at least one output line; stdout: {out:?}",
    );
    // The dirs +1 output line shouldn't contain a space (single entry).
    let last_dirs = lines
        .iter()
        .rev()
        .find(|l| !l.contains(' '))
        .copied()
        .unwrap_or("");
    assert!(
        !last_dirs.is_empty(),
        "expected a single-entry dirs +1 line; stdout: {out:?}",
    );
}
```

If some tests are sensitive to which `huck_binary()` executes
(e.g., the `pushd /var` path needs `/var` to exist), all paths
used (`/tmp`, `/var`) exist on standard Linux/macOS.

- [ ] **Step 2.1**

### Step 2.2: Run integration tests

```bash
cargo test --test dirstack_integration -- --nocapture
```

Expected: 10 pass.

- [ ] **Step 2.2**

### Step 2.3: Full integration suite

`cargo test --tests`. Expected: green (PTY flake tolerated).

- [ ] **Step 2.3**

### Step 2.4: Clippy

`cargo clippy --all-targets -- -D warnings`. Expected: clean.

- [ ] **Step 2.4**

### Step 2.5: Commit Task 2

```bash
git add tests/dirstack_integration.rs
git commit -m "$(cat <<'EOF'
test: dirstack integration coverage (v63 task 2)

10 binary-driven tests for pushd/popd/dirs end-to-end through
the huck binary:

- pushd_dir_then_dirs — pushd /tmp; dirs → starts with /tmp.
- pushd_then_popd_returns_to_origin — round trip.
- pushd_no_args_swaps_top_two — push two then bare pushd
  swaps.
- pushd_only_one_entry_errors — bare pushd on fresh shell.
- popd_empty_errors — popd on fresh shell.
- dirs_default_collapses_home — HOME=\$PWD; dirs → ~.
- dirs_v_numbered — 3+ numbered lines after two pushes.
- dirs_c_clears — clear truncates to current dir only.
- pushd_plus_n_rotates — +2 rotation lands on bottom.
- dirs_plus_index_prints_one — single-entry output.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 2.5**

---

## Task 3: Docs

**Files:**
- Modify `docs/bash-divergences.md` — M-78 entry + v63 change-log.
- Modify `README.md` — v63 row.

### Step 3.1: Add M-78 entry

After M-77 (v62 rc file):

```markdown
- **M-78: `pushd` / `popd` / `dirs`** — `[fixed v63]` medium. Bash directory-stack builtins. New `Shell.dir_stack: Vec<PathBuf>` field (top = index 0, synced with `$PWD` at the top of each call). `pushd DIR` cd's to DIR and inserts the new PWD at front; bare `pushd` swaps top two; `pushd +N`/`-N` rotates so that entry becomes top. `popd` removes top + cd; `popd +N`/`-N` removes that entry (cd only when idx 0). `dirs` lists (default = space-joined, `~`-collapsed); `-c` clear, `-l` no collapse, `-p` per-line, `-v` numbered per-line, `+N`/`-N` print just that entry. Reuses `builtin_cd` (promoted to `pub(crate)`) for the actual directory change. None of the three builtins are POSIX special. **Deferred**: `pushd -n DIR` / `popd -n` (push/pop without cd), `DIRSTACK` shell array (huck has no arrays).
```

- [ ] **Step 3.1**

### Step 3.2: Add v63 change-log entry

In `## Change log` after v62:

```markdown
- **2026-05-31**: M-78 (`pushd`/`popd`/`dirs`) shipped as v63. New `Shell.dir_stack: Vec<PathBuf>` field. Three new builtins in `src/builtins.rs` totaling ~300 LOC. Pure helpers (`parse_signed_index` for `+N`/`-N` parsing; `dir_display` for `~`-collapse formatting; `sync_stack_top` for `$PWD` sync; `print_stack` for the per-flag output shapes) are independently unit-testable. `builtin_pushd` handles DIR / bare-swap / `+N`/`-N` rotation (with cd-failure undo). `builtin_popd` handles bare / `+N`/`-N` (cd only when idx 0 was removed). `builtin_dirs` handles `-c`/`-l`/`-p`/`-v`/`+N`/`-N`. Stack top is always `$PWD` after the sync at the top of each call (handles users who `cd`'d outside of pushd/popd between calls). `builtin_cd` promoted from `fn` to `pub(crate) fn` so the dirstack builtins can reuse the existing PWD/OLDPWD/HOME logic. 10 unit tests in `mod dirstack_tests` on pure helpers (parse_signed_index ×5, dir_display ×5) + 10 binary-driven integration tests in `tests/dirstack_integration.rs` for full builtin behavior. Deferred: `pushd -n` / `popd -n` (push/pop without cd) and `DIRSTACK` array (huck has no arrays). No new L-* divergences.
```

- [ ] **Step 3.2**

### Step 3.3: Add v63 row to README

After v62:

```markdown
| v63       | `pushd`/`popd`/`dirs` (M-78)                                   |
```

Match v62 column padding.

- [ ] **Step 3.3**

### Step 3.4: Full suite

`cargo test --all-targets`. Expected: green (PTY flake
tolerated).

- [ ] **Step 3.4**

### Step 3.5: Clippy

`cargo clippy --all-targets -- -D warnings`. Expected: clean.

- [ ] **Step 3.5**

### Step 3.6: Commit Task 3

```bash
git add docs/bash-divergences.md README.md
git commit -m "$(cat <<'EOF'
docs: add M-78 (pushd/popd/dirs) fixed v63

New M-78 entry in docs/bash-divergences.md covers the three
directory-stack builtins with full bash flag set (-c/-l/-p/-v
on dirs, +N/-N on all three for rotation/index). Lists deferred
items (pushd -n / popd -n, DIRSTACK array).

Change log: 2026-05-31 v63 entry summarizing the new
Shell.dir_stack field, the four pure helpers (parse_signed_index,
dir_display, sync_stack_top, print_stack), the three builtins,
and the reuse of builtin_cd (promoted to pub(crate)).

README: v63 row added.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 3.6**

---

## Final verification (controller)

1. `cargo test --all-targets` once more.
2. `cargo clippy --all-targets -- -D warnings`.
3. Branch is four commits ahead of `main`: docs preamble + 3
   task commits.
4. Dispatch a final cross-task reviewer.
5. Merge to `main` with `--no-ff`, push, delete branch, update
   memory.
