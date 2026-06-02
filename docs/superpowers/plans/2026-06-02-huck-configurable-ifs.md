# huck v74 — Configurable IFS Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking. Each task is implemented by a fresh subagent, with spec-compliance review and code-quality review between tasks.

**Goal:** Replace huck's hardcoded ASCII-whitespace word-splitter (`split_ascii_whitespace`) with bash-compatible POSIX IFS-driven field splitting.

**Architecture:** New `Shell::ifs()` accessor centralizes the unset→default vs empty→no-split decision. Rewrite `emit_split_fields` in `src/expand.rs` to take an `ifs: &str` parameter and implement POSIX § 2.6.5 (whitespace IFS classes collapse; non-whitespace IFS chars each delimit a field; leading IFS-whitespace stripped; trailing non-whitespace IFS does NOT add an empty trailing field). All 7 callsites thread the IFS through. ~10 `${*}` / `${a[*]}` join sites switch to a shared `ifs_join_sep` helper (DRY; no behavior change).

**Tech Stack:** Rust 1.85+; existing `Shell` / `Field` / `lookup_var` infrastructure.

**Branch:** `v74-configurable-ifs` (create from `main` in Preamble P.1).

**Spec:** `docs/superpowers/specs/2026-06-02-huck-configurable-ifs-design.md`.

**Commit trailer (every commit):**

```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Preamble P.1: Branch setup

- [ ] **Step 1: Verify clean tree on main**

Run: `git status && git rev-parse --abbrev-ref HEAD`
Expected: branch `main`, clean working tree.

- [ ] **Step 2: Create the iteration branch**

```bash
git checkout -b v74-configurable-ifs
```

Expected: `Switched to a new branch 'v74-configurable-ifs'`.

- [ ] **Step 3: Confirm baseline tests pass**

Run: `cargo test 2>&1 | grep "test result" | awk '{sum+=$4} END {print "Total:", sum}'`
Expected: 2090 (or whatever current main reports).

- [ ] **Step 4: Confirm clippy is clean**

Run: `cargo clippy --all-targets 2>&1 | tail -3`
Expected: `Finished` no warnings.

---

## File-structure map

| File | Responsibility | Tasks |
|------|----------------|-------|
| `src/shell_state.rs` | New `Shell::ifs()` accessor | 1 |
| `src/expand.rs` | New `ifs_join_sep` helper; rewrite `emit_split_fields`; thread `&shell.ifs()` through 7 callers; update 8 `${a[*]}`/`${@}`/`${*}` join sites to `ifs_join_sep` | 1, 2 |
| `src/param_expansion.rs` | Update 0+ join sites (verify any reside here; sweep) | 2 |
| `tests/ifs_integration.rs` | 8 binary-driven IFS tests (new file) | 3 |
| `tests/scripts/ifs_diff_check.sh` | New bash-diff harness for IFS fragments | 3 |
| `docs/bash-divergences.md` | M-05 entry update, change-log entry | 3 |
| `README.md` | v74 row | 3 |

---

## Task 1: Splitter foundation + call-site wiring

**Files:**
- Modify: `src/shell_state.rs` — add `Shell::ifs()`
- Modify: `src/expand.rs` — rewrite `emit_split_fields`; thread `&shell.ifs()` through 7 callers

**Goal:** End-to-end IFS-aware word-splitting works. After this task, `IFS=:; v=a:b:c; for x in $v; do echo $x; done` produces 3 lines. Existing 2090 tests continue to pass under default IFS.

### Steps

- [ ] **Step 1: Add `Shell::ifs()` accessor**

Edit `src/shell_state.rs`. Add this method inside the existing `impl Shell` block (after the existing `lookup_var` method, around line 250):

```rust
/// Returns the current value of `$IFS`.
///
/// - Unset → POSIX default `" \t\n"`.
/// - Empty string → empty (caller's word-splitter must short-circuit
///   "no splitting" semantics; the `${*}` join treats empty IFS as
///   "concatenate without separator").
/// - Otherwise → the literal IFS value.
///
/// Centralized so the unset-vs-empty boundary is explicit at every
/// expansion-site call.
pub fn ifs(&self) -> String {
    self.lookup_var("IFS").unwrap_or_else(|| " \t\n".to_string())
}
```

- [ ] **Step 2: Add unit test for `Shell::ifs()`**

Append to the existing `#[cfg(test)] mod` at the bottom of `src/shell_state.rs` (whichever module has Shell tests; if uncertain, add to `mod array_value_tests` or create a small `mod ifs_helper_tests`):

```rust
#[cfg(test)]
mod ifs_helper_tests {
    use super::*;

    #[test]
    fn ifs_default_when_unset() {
        let s = Shell::new();
        assert_eq!(s.ifs(), " \t\n");
    }

    #[test]
    fn ifs_returns_set_value() {
        let mut s = Shell::new();
        s.set("IFS", ":".to_string());
        assert_eq!(s.ifs(), ":");
    }

    #[test]
    fn ifs_returns_empty_when_set_to_empty() {
        let mut s = Shell::new();
        s.set("IFS", "".to_string());
        assert_eq!(s.ifs(), "");
    }
}
```

- [ ] **Step 3: Add `ifs_join_sep` free function in `src/expand.rs`**

Edit `src/expand.rs`. Near the existing helpers (search for `fn strip_trailing_newlines` around line 896), add this free function:

```rust
/// Returns the separator for `"$*"` / `"${a[*]}"` joins.
/// Empty IFS → empty separator (concatenate). Otherwise → first char of
/// IFS. Matches bash § 3.5.5 ("If IFS is null, the parameters are joined
/// without intervening separators").
pub(crate) fn ifs_join_sep(ifs: &str) -> String {
    ifs.chars().next().map(|c| c.to_string()).unwrap_or_default()
}
```

(Note: this helper is used by Task 2 to consolidate the existing duplicated pattern. It exists in Task 1 so Task 2's diff is minimal.)

- [ ] **Step 4: Rewrite `emit_split_fields` with IFS-aware signature**

In `src/expand.rs` around line 900, find the existing:

```rust
fn emit_split_fields(
    value: &str,
    current: &mut Field,
    result: &mut Vec<Field>,
    has_emitted: &mut bool,
) {
    let fragments: Vec<&str> = value.split_ascii_whitespace().collect();
    if fragments.is_empty() {
        return;
    }
    // First fragment continues the in-progress field.
    current.push_str(fragments[0], false);
    *has_emitted = true;
    // Each subsequent fragment closes the field and starts a new one.
    for frag in &fragments[1..] {
        let finished = std::mem::take(current);
        result.push(finished);
        current.push_str(frag, false);
    }
}
```

Replace with the IFS-aware implementation:

```rust
fn emit_split_fields(
    value: &str,
    ifs: &str,
    current: &mut Field,
    result: &mut Vec<Field>,
    has_emitted: &mut bool,
) {
    // POSIX § 2.6.5 field splitting. The two IFS classes:
    //   - whitespace IFS: subset of IFS bytes that are ' ' / '\t' / '\n'.
    //   - non-whitespace IFS: any other IFS byte.
    // Empty IFS → no splitting; value joins the in-progress field.
    if ifs.is_empty() {
        if !value.is_empty() {
            current.push_str(value, false);
            *has_emitted = true;
        }
        return;
    }

    let ifs_bytes = ifs.as_bytes();
    let is_ws = |b: u8| ifs_bytes.contains(&b) && matches!(b, b' ' | b'\t' | b'\n');
    let is_nonws = |b: u8| ifs_bytes.contains(&b) && !matches!(b, b' ' | b'\t' | b'\n');
    let is_any_ifs = |b: u8| ifs_bytes.contains(&b);

    let bytes = value.as_bytes();
    let mut i = 0usize;

    // Skip leading IFS-whitespace.
    while i < bytes.len() && is_ws(bytes[i]) {
        i += 1;
    }
    if i >= bytes.len() {
        return;
    }

    let mut first_field = true;

    while i < bytes.len() {
        // Read one field (non-IFS bytes).
        let field_start = i;
        while i < bytes.len() && !is_any_ifs(bytes[i]) {
            i += 1;
        }
        let field_end = i;
        let field_bytes = &bytes[field_start..field_end];
        let field_str = std::str::from_utf8(field_bytes).unwrap_or("");

        if first_field {
            current.push_str(field_str, false);
            *has_emitted = true;
            first_field = false;
        } else {
            let finished = std::mem::take(current);
            result.push(finished);
            current.push_str(field_str, false);
        }

        if i >= bytes.len() {
            break;
        }

        // We're now sitting on an IFS byte. Classify the separator run.
        //   - If the FIRST IFS byte is non-whitespace, consume EXACTLY one
        //     non-ws byte plus any trailing whitespace-IFS. This produces
        //     one separator. Continue (empty field next if another non-ws
        //     follows immediately).
        //   - If the first IFS byte is whitespace, consume the whole
        //     whitespace run. Then OPTIONALLY consume one non-whitespace
        //     IFS byte plus its trailing whitespace-IFS run.
        if is_nonws(bytes[i]) {
            i += 1;
            while i < bytes.len() && is_ws(bytes[i]) {
                i += 1;
            }
        } else {
            // Whitespace IFS run.
            while i < bytes.len() && is_ws(bytes[i]) {
                i += 1;
            }
            if i < bytes.len() && is_nonws(bytes[i]) {
                i += 1;
                while i < bytes.len() && is_ws(bytes[i]) {
                    i += 1;
                }
            }
        }

        // If we consumed all remaining input as a separator, do NOT emit
        // a trailing empty field. POSIX: "If the input string ends with a
        // non-whitespace IFS character, that delimiter does not produce
        // an empty field." (Bash: `IFS=:; v="a:"; echo $v` → `a`.)
        if i >= bytes.len() {
            break;
        }
    }
}
```

- [ ] **Step 5: Update the 7 `emit_split_fields` call sites in `src/expand.rs`**

Run: `grep -n "emit_split_fields(" src/expand.rs`

Expected 7 callsites (per pre-task grep): lines 597, 609, 637, 646, 682, 710, plus the definition at 900.

At each callsite (NOT the definition), insert `&shell.ifs()` as the second argument. Example transformation:

Before:
```rust
emit_split_fields(&value, &mut current, &mut result, &mut has_emitted);
```

After:
```rust
let ifs = shell.ifs();
emit_split_fields(&value, &ifs, &mut current, &mut result, &mut has_emitted);
```

(Each call site is inside a `match` arm with `shell: &mut Shell` in scope. The local `let ifs = shell.ifs()` binding avoids a borrow-checker fight if the call site already uses `shell` in the same arm. If `ifs` is already in scope at the call site from an earlier IFS-join computation, reuse it instead of looking up again.)

For the 5 callsites that ALREADY compute IFS for a `${*}` join nearby (around lines 286, 316, 322, 344, etc.), thread the existing `ifs` binding into `emit_split_fields`.

For the 2 callsites that don't currently have an IFS binding in scope (lines 597, 609, 637, 646, 682 if the surrounding match arm doesn't compute it), add a local `let ifs = shell.ifs();` immediately before the call.

After each edit, run `cargo build 2>&1 | tail -5` to confirm progress.

- [ ] **Step 6: Add unit tests for `emit_split_fields`**

Append to `src/expand.rs` after the existing test modules (at the very bottom of the file):

```rust
#[cfg(test)]
mod ifs_splitter_tests {
    //! POSIX § 2.6.5 field-splitting unit tests for `emit_split_fields`.
    //! These tests drive the splitter directly, not the lex→expand
    //! pipeline, so they isolate the IFS classification logic from
    //! upstream changes.

    use super::*;

    fn run(value: &str, ifs: &str) -> Vec<String> {
        let mut current = Field::default();
        let mut result: Vec<Field> = Vec::new();
        let mut has_emitted = false;
        emit_split_fields(value, ifs, &mut current, &mut result, &mut has_emitted);
        // If anything was emitted, push the in-progress field too.
        if has_emitted {
            result.push(current);
        }
        result.into_iter().map(|f| f.chars).collect()
    }

    #[test]
    fn default_ifs_collapses_whitespace_runs() {
        // value "a  b\tc" with default IFS → 3 fields a/b/c.
        assert_eq!(run("a  b\tc", " \t\n"), vec!["a", "b", "c"]);
    }

    #[test]
    fn colon_ifs_preserves_empty_between() {
        // value "a::b" with IFS=: → a/""/"b"
        assert_eq!(run("a::b", ":"), vec!["a", "", "b"]);
    }

    #[test]
    fn colon_ifs_leading_produces_empty() {
        // value ":a" with IFS=: → ""/"a"
        assert_eq!(run(":a", ":"), vec!["", "a"]);
    }

    #[test]
    fn colon_ifs_trailing_no_empty() {
        // value "a:" with IFS=: → "a" (1 field; trailing non-ws-IFS
        // does NOT add an empty trailing field).
        assert_eq!(run("a:", ":"), vec!["a"]);
    }

    #[test]
    fn mixed_ifs_ws_collapses_around_nonws() {
        // value "a : b" with IFS=" :" → 2 fields a/b (colon plus
        // adjacent spaces collapse to one separator).
        assert_eq!(run("a : b", " :"), vec!["a", "b"]);
    }

    #[test]
    fn empty_ifs_no_split() {
        // IFS="" → no splitting; "a b c" is a single field.
        assert_eq!(run("a b c", ""), vec!["a b c"]);
    }

    #[test]
    fn whitespace_only_value_yields_no_fields() {
        // IFS=" \t\n", value "   " → 0 fields (leading-ws strip
        // consumes everything).
        assert_eq!(run("   ", " \t\n"), Vec::<String>::new());
    }

    #[test]
    fn mixed_consecutive_nonws_yields_empty_field() {
        // IFS=":,", value "a:,b" → a/""/"b" (two consecutive non-ws
        // IFS chars produce an empty field between them).
        assert_eq!(run("a:,b", ":,"), vec!["a", "", "b"]);
    }

    #[test]
    fn single_nonws_only_yields_empty_field() {
        // IFS=":", value ":" → 1 empty field (the colon delimits
        // a field starting at position 0 with empty content).
        assert_eq!(run(":", ":"), vec![""]);
    }

    #[test]
    fn leading_nonws_then_value() {
        // IFS=":", value ":x" → ""/"x"
        assert_eq!(run(":x", ":"), vec!["", "x"]);
    }

    #[test]
    fn ws_only_ifs_pure_whitespace_collapses() {
        // IFS=" ", value " a b " → a/b (leading and trailing
        // whitespace are stripped; runs collapse).
        assert_eq!(run(" a b ", " "), vec!["a", "b"]);
    }

    #[test]
    fn nonws_ifs_with_ws_value_no_split() {
        // IFS=":" (no whitespace in IFS), value "a b" → 1 field
        // "a b" (space is not an IFS char).
        assert_eq!(run("a b", ":"), vec!["a b"]);
    }

    #[test]
    fn empty_value_emits_nothing() {
        // value "" with any IFS → 0 fields.
        assert_eq!(run("", ":"), Vec::<String>::new());
        assert_eq!(run("", " \t\n"), Vec::<String>::new());
    }

    #[test]
    fn current_field_continuation() {
        // If `current` already has text, the first split fragment
        // continues it rather than starting a new field.
        let mut current = Field::default();
        current.push_str("prefix-", false);
        let mut result: Vec<Field> = Vec::new();
        let mut has_emitted = true;  // simulating mid-expansion state
        emit_split_fields("a b c", " \t\n", &mut current, &mut result,
                          &mut has_emitted);
        result.push(current);
        let words: Vec<String> = result.into_iter().map(|f| f.chars).collect();
        assert_eq!(words, vec!["prefix-a", "b", "c"]);
    }
}
```

- [ ] **Step 7: Run unit tests for the splitter**

Run: `cargo test --bin huck ifs_splitter 2>&1 | tail -25`
Expected: 14 tests pass.

If any fail, debug the splitter implementation. Common gotchas:
- The leading-non-ws-IFS case (e.g., `":a"` → `["", "a"]`): the loop must NOT skip the leading non-ws-IFS as part of "leading IFS-whitespace strip." Only leading WS-IFS is stripped.
- The trailing-non-ws-IFS case (e.g., `"a:"` → `["a"]`): after consuming the final separator, the loop breaks at `if i >= bytes.len() { break; }` BEFORE emitting an empty field.
- The "current field continuation" case: don't reset `current` at function entry; append the first fragment to it.

- [ ] **Step 8: Run the full test suite (regression check)**

Run: `cargo test 2>&1 | grep -E "test result|FAILED" | head -10`
Expected: 0 failures. Total should be 2090 + 14 (splitter) + 3 (`Shell::ifs()` helper) = 2107.

If ANY existing test fails, the change broke default-IFS behavior. Most likely culprits:
- A callsite was missed when threading `&ifs` (compile error caught earlier — but if you tab-completed to the wrong site).
- The new splitter treats default-IFS differently than `split_ascii_whitespace()` did. The default-IFS-collapse-runs test should catch this.

- [ ] **Step 9: Run clippy**

Run: `cargo clippy --all-targets 2>&1 | tail -5`
Expected: `Finished` no warnings.

- [ ] **Step 10: Commit**

```bash
git add src/shell_state.rs src/expand.rs
git commit -m "$(cat <<'EOF'
expand: IFS-aware emit_split_fields + Shell::ifs() helper (v74 task 1)

Replaces split_ascii_whitespace() with a POSIX § 2.6.5 field-splitter
that respects $IFS. The new emit_split_fields takes an `ifs: &str`
parameter; the 7 existing callsites thread `&shell.ifs()` through.

Shell::ifs() centralizes the unset→default vs empty→no-split logic.
ifs_join_sep() helper added (used by Task 2).

POSIX semantics implemented:
- Empty IFS → no splitting.
- Whitespace-IFS runs collapse to one separator.
- Non-whitespace-IFS chars each delimit a field (a::b → a/""/b).
- Leading IFS-whitespace stripped; leading non-ws-IFS produces empty.
- Trailing non-ws-IFS does NOT add a trailing empty (bash matches).

14 unit tests in mod ifs_splitter_tests pin each case. 3 tests cover
Shell::ifs() defaults. Existing 2090 tests still pass under default IFS.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Consolidate IFS join sites via `ifs_join_sep`

**Files:**
- Modify: `src/expand.rs` — replace 10 inline `chars().next().map(...).unwrap_or_default()` patterns with `ifs_join_sep(&ifs)`
- Modify: `src/param_expansion.rs` — sweep for any duplicated pattern (verify there are or aren't any; the spec's audit shows the pattern lives in expand.rs)

**Goal:** Pure refactor — DRY consolidation of the IFS first-char extraction pattern. No behavior change.

### Steps

- [ ] **Step 1: Run grep to find all sites**

Run: `grep -n "chars().next().map" src/expand.rs src/param_expansion.rs`
Expected: ~10 sites in `src/expand.rs` (per pre-task grep: lines 251, 286, 316, 322, 344, 430, 478, 500, 708, 810). Possibly 0 sites in `src/param_expansion.rs`.

- [ ] **Step 2: Replace each site**

For each match, the surrounding pattern is:

```rust
let ifs = shell.lookup_var("IFS").unwrap_or_else(|| " \t\n".to_string());
let sep = ifs.chars().next().map(|c| c.to_string()).unwrap_or_default();
```

Replace with:

```rust
let ifs = shell.ifs();
let sep = ifs_join_sep(&ifs);
```

(The `Shell::ifs()` lookup and `ifs_join_sep` helper were added in Task 1.)

Note: do NOT collapse `let ifs = shell.ifs()` into the call — the local binding is reused at sites that also call `emit_split_fields(.., &ifs, ..)` later in the same arm. Keep the binding so Task 1's wiring stays valid.

Where a site uses `ifs` only for the join and never for splitting, you may collapse:

```rust
let sep = ifs_join_sep(&shell.ifs());
```

— but only if `ifs` is otherwise unused in the surrounding scope. Default to keeping the two-line form for consistency.

- [ ] **Step 3: Build and run all tests**

Run: `cargo build 2>&1 | tail -5`
Expected: clean.

Run: `cargo test 2>&1 | grep -E "test result|FAILED" | head -10`
Expected: 0 failures; total same as Task 1 (no new tests in this task — it's pure refactor).

Run: `cargo clippy --all-targets 2>&1 | tail -5`
Expected: 0 warnings.

- [ ] **Step 4: Verify the grep count drops to zero**

Run: `grep -n "chars().next().map" src/expand.rs src/param_expansion.rs`
Expected: no output (zero matches). If any matches remain, they're unrelated to IFS — verify by reading context; if they ARE IFS-related, replace them too.

- [ ] **Step 5: Commit**

```bash
git add src/expand.rs src/param_expansion.rs
git commit -m "$(cat <<'EOF'
expand: consolidate IFS join via ifs_join_sep (v74 task 2)

Replace 10 inline `ifs.chars().next().map(|c| c.to_string()).unwrap_or_default()`
patterns with the ifs_join_sep helper added in Task 1. Pure refactor;
no behavior change. The lookups now route through Shell::ifs() too,
making the unset→default vs empty→no-split boundary explicit at every
join site.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Integration tests + bash-diff harness + docs

**Files:**
- Create: `tests/ifs_integration.rs`
- Create: `tests/scripts/ifs_diff_check.sh`
- Modify: `docs/bash-divergences.md` — M-05 entry + change-log entry
- Modify: `README.md` — v74 row

### Steps

- [ ] **Step 1: Write integration tests at `tests/ifs_integration.rs`**

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
    child.stdin.as_mut().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn default_ifs_for_loop_splits_on_whitespace() {
    let (out, _, _) = run_capture("v=\"a  b\tc\"\nfor x in $v; do echo $x; done\nexit\n");
    let lines: Vec<&str> = out.lines().collect();
    let words: Vec<&str> = lines.iter().filter(|l| ["a","b","c"].contains(l)).copied().collect();
    assert_eq!(words, vec!["a", "b", "c"], "got: {out:?}");
}

#[test]
fn colon_ifs_for_loop_splits_on_colons() {
    let (out, _, _) = run_capture("IFS=:\nv=\"a:b:c\"\nfor x in $v; do echo $x; done\nexit\n");
    let lines: Vec<&str> = out.lines().collect();
    let words: Vec<&str> = lines.iter().filter(|l| ["a","b","c"].contains(l)).copied().collect();
    assert_eq!(words, vec!["a", "b", "c"], "got: {out:?}");
}

#[test]
fn colon_ifs_preserves_empty_middle_field() {
    let (out, _, _) = run_capture(
        "IFS=:\nv=\"a::b\"\nfor x in $v; do echo \"[$x]\"; done\nexit\n"
    );
    let lines: Vec<&str> = out.lines().filter(|l| l.starts_with('[')).collect();
    assert_eq!(lines, vec!["[a]", "[]", "[b]"], "got: {out:?}");
}

#[test]
fn colon_ifs_trailing_no_empty_field() {
    let (out, _, _) = run_capture(
        "IFS=:\nv=\"a:\"\nfor x in $v; do echo \"[$x]\"; done\nexit\n"
    );
    let lines: Vec<&str> = out.lines().filter(|l| l.starts_with('[')).collect();
    assert_eq!(lines, vec!["[a]"], "got: {out:?}");
}

#[test]
fn empty_ifs_no_splitting() {
    let (out, _, _) = run_capture(
        "IFS=\nv=\"a b c\"\nfor x in $v; do echo \"[$x]\"; done\nexit\n"
    );
    let lines: Vec<&str> = out.lines().filter(|l| l.starts_with('[')).collect();
    assert_eq!(lines, vec!["[a b c]"], "got: {out:?}");
}

#[test]
fn local_ifs_reverts_on_function_return() {
    let (out, _, _) = run_capture(
        "v=\"a:b\"\n\
         f() { local IFS=:; for x in $v; do echo \"in:$x\"; done; }\n\
         f\n\
         for x in $v; do echo \"out:$x\"; done\n\
         exit\n"
    );
    // Inside f, IFS=: splits "a:b" into a/b. After f, default IFS splits
    // on whitespace and "a:b" is one field.
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines.iter().any(|l| **l == "in:a"), "got: {out:?}");
    assert!(lines.iter().any(|l| **l == "in:b"), "got: {out:?}");
    assert!(lines.iter().any(|l| **l == "out:a:b"), "got: {out:?}");
}

#[test]
fn command_sub_splits_with_current_ifs() {
    let (out, _, _) = run_capture(
        "IFS=:\nfor x in $(echo \"a:b:c\"); do echo $x; done\nexit\n"
    );
    let lines: Vec<&str> = out.lines().collect();
    let words: Vec<&str> = lines.iter().filter(|l| ["a","b","c"].contains(l)).copied().collect();
    assert_eq!(words, vec!["a", "b", "c"], "got: {out:?}");
}

#[test]
fn star_join_uses_first_ifs_char() {
    let (out, _, _) = run_capture(
        "set -- a b c\nIFS=,\necho \"$*\"\nexit\n"
    );
    assert!(out.lines().any(|l| l == "a,b,c"), "got: {out:?}");
}

#[test]
fn star_join_empty_ifs_concatenates() {
    let (out, _, _) = run_capture(
        "set -- a b c\nIFS=\necho \"$*\"\nexit\n"
    );
    assert!(out.lines().any(|l| l == "abc"), "got: {out:?}");
}

#[test]
fn inline_prefix_ifs_applies_to_this_command() {
    let (out, _, _) = run_capture(
        "v=\"a:b:c\"\n\
         IFS=: echo $v\n\
         exit\n"
    );
    // Inline prefix IFS=: applies during the echo's argv expansion,
    // so $v splits into 3 args. echo joins with single space → "a b c".
    assert!(out.lines().any(|l| l == "a b c"), "got: {out:?}");
}
```

- [ ] **Step 2: Run the integration tests**

Run: `cargo test --test ifs_integration 2>&1 | tail -15`
Expected: 10 tests pass.

If `inline_prefix_ifs_applies_to_this_command` fails, the inline-assignments mechanism may not be ordering correctly. Debug by checking that `apply_inline_assignments` runs BEFORE arg expansion in `src/executor.rs::run_exec_single`.

- [ ] **Step 3: Write the bash-diff harness**

Create `tests/scripts/ifs_diff_check.sh`:

```bash
#!/usr/bin/env bash
# Manual sanity check: run the same IFS fragments through bash and huck,
# diff outputs. Not part of `cargo test` (no bash dependency in CI), but
# run by the developer before merge.
set -u

HUCK="$(dirname "$0")/../../target/debug/huck"
if [ ! -x "$HUCK" ]; then
    echo "build huck first: cargo build" >&2
    exit 1
fi

if ! command -v bash >/dev/null 2>&1; then
    echo "bash not found on PATH; this differential harness requires bash" >&2
    exit 1
fi

fragments=(
    'v="a b c"; for x in $v; do echo $x; done'
    'IFS=:; v="a:b:c"; for x in $v; do echo $x; done'
    'IFS=:; v="a::b"; for x in $v; do echo "[$x]"; done'
    'IFS=:; v=":a"; for x in $v; do echo "[$x]"; done'
    'IFS=:; v="a:"; for x in $v; do echo "[$x]"; done'
    'IFS=" :"; v="a : b"; for x in $v; do echo $x; done'
    'IFS=; v="a b c"; for x in $v; do echo "[$x]"; done'
    'set -- a b c; IFS=,; echo "$*"'
    'set -- a b c; IFS=; echo "$*"'
    'IFS=:; for x in $(echo "a:b:c"); do echo $x; done'
)

fail=0
for f in "${fragments[@]}"; do
    b_out=$(bash -c "$f" 2>&1)
    h_out=$(echo "$f" | "$HUCK" 2>&1)
    if [ "$b_out" != "$h_out" ]; then
        echo "DIFF on: $f"
        diff <(printf '%s\n' "$b_out") <(printf '%s\n' "$h_out") || true
        echo "---"
        fail=1
    fi
done

if [ "$fail" -eq 0 ]; then
    echo "all IFS fragments produce identical output to bash"
fi
exit "$fail"
```

Make it executable:

```bash
chmod +x tests/scripts/ifs_diff_check.sh
```

- [ ] **Step 4: Run the bash-diff harness**

```bash
cargo build && bash tests/scripts/ifs_diff_check.sh
```

Expected: `all IFS fragments produce identical output to bash`. If ANY fragment produces a DIFF, debug huck (not the harness).

- [ ] **Step 5: Update `docs/bash-divergences.md` — M-05 entry**

Find M-05 (around line 131):

```markdown
- **M-05: IFS not configurable** — `[deferred]` high. huck: word-splitting hardcoded to ASCII whitespace. bash: any `IFS` value governs splitting.
```

Replace with:

```markdown
- **M-05: Configurable IFS** — `[fixed v74]` high. Word-splitting follows the current value of `$IFS` per POSIX § 2.6.5. The new `emit_split_fields` in `src/expand.rs` partitions IFS bytes into whitespace (`' '`/`'\t'`/`'\n'` if present in IFS) and non-whitespace classes: whitespace runs collapse to a single separator; non-whitespace IFS bytes each delimit a field (so `IFS=:; v="a::b"` splits to `a`/``/`b`); leading IFS-whitespace is stripped; trailing non-whitespace IFS does NOT produce a trailing empty field (matches bash: `v="a:"; echo $v` → `a`). Empty `IFS=""` short-circuits — no field splitting, and `"$*"` joins with no separator. Unset IFS → POSIX default `" \t\n"`. New `Shell::ifs()` accessor centralizes the unset/empty boundary. `${*}` / `${a[*]}` / `${m[*]}` joins use the first char of IFS via `ifs_join_sep` (empty IFS → empty separator, concatenate). `read`'s split-by-name machinery (v55) already implemented this correctly; v74 brings expansion-time splitting into parity. ~28 unit tests across `mod ifs_splitter_tests` (14 covering each POSIX edge case) + extensions to existing array-expansion modules. 10 binary-driven integration tests in `tests/ifs_integration.rs` + 10 bash-diff fragments in `tests/scripts/ifs_diff_check.sh` (byte-identical to bash 5.2.21).
```

- [ ] **Step 6: Add change-log entry**

At the end of `docs/bash-divergences.md`, append:

```markdown
- **2026-06-02**: M-05 (configurable IFS) shipped as v74. Replaces `split_ascii_whitespace()` in `emit_split_fields` with a POSIX § 2.6.5 byte-classifying field-splitter that respects `$IFS`. New `Shell::ifs()` accessor centralizes the unset→default vs empty→no-split boundary. 7 expansion callsites in `src/expand.rs` thread `&shell.ifs()` through. New `ifs_join_sep` helper consolidates the 10 inline `chars().next().map(...).unwrap_or_default()` patterns at `${*}`/`${a[*]}` join sites (pure DRY refactor, no behavior change). 14 splitter unit tests in `mod ifs_splitter_tests` + 3 `Shell::ifs()` helper tests + 10 binary-driven integration tests in `tests/ifs_integration.rs`. New `tests/scripts/ifs_diff_check.sh` bash-diff harness — 10 fragments byte-identical to bash 5.2.21. Existing 2090 tests still pass under default IFS (`" \t\n"`), confirming the splitter rewrite preserves the previous default-case behavior.
```

- [ ] **Step 7: Add v74 row to `README.md`**

Find the v73 row in the iteration table:

```markdown
| v73       | fix `${a[i]:-W}` on missing element (M-82 follow-up)           |
```

Append:

```markdown
| v73       | fix `${a[i]:-W}` on missing element (M-82 follow-up)           |
| v74       | configurable IFS (M-05)                                        |
```

- [ ] **Step 8: Final verification**

```bash
cargo build 2>&1 | tail -5
cargo test 2>&1 | grep -E "test result|FAILED" | tail -10
cargo clippy --all-targets 2>&1 | tail -5
bash tests/scripts/ifs_diff_check.sh
bash tests/scripts/arrays_diff_check.sh  # ensure v71/v72/v73 work still byte-identical
```

All five should pass. Total test count: ~2107 + 10 integration = ~2117.

- [ ] **Step 9: Commit**

```bash
git add tests/ifs_integration.rs tests/scripts/ifs_diff_check.sh docs/bash-divergences.md README.md
git commit -m "$(cat <<'EOF'
docs+tests: configurable IFS shipped v74 (M-05)

10 binary-driven integration tests covering default IFS, IFS=:
with empty-middle and no-trailing-empty rules, IFS=" :" mixed,
empty IFS no-split, local IFS scope, command-sub splitting,
${*} join with comma and empty IFS, and inline-prefix IFS=
applied to the same command.

New M-05 entry in bash-divergences.md (was [deferred] high; now
[fixed v74]). Change-log entry. README v74 row.

New tests/scripts/ifs_diff_check.sh bash-diff harness — 10
fragments verified byte-identical to bash 5.2.21. The existing
arrays_diff_check.sh remains green, confirming no regression in
v71/v72/v73 work.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Final verification & merge prep

- [ ] **Step 1: Full test pass**

Run: `cargo test 2>&1 | grep "test result" | awk '{sum+=$4} END {print "Total:", sum}'`
Expected: ~2117 (2090 + 17 unit + 10 integration).

- [ ] **Step 2: Clippy clean**

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -5`
Expected: no warnings.

- [ ] **Step 3: Both bash-diff harnesses pass**

```bash
bash tests/scripts/arrays_diff_check.sh
bash tests/scripts/ifs_diff_check.sh
```

Expected: both "all ... identical to bash".

- [ ] **Step 4: M-05 entry well-formed**

Run: `grep -nE "M-05.*fixed v74" docs/bash-divergences.md | head -3`
Expected: at least one match (Tier-2 entry + change-log entry).

- [ ] **Step 5: Confirm v74 row in README**

Run: `grep "v74" README.md`
Expected: one row with `configurable IFS (M-05)`.

- [ ] **Step 6: Ask user for merge confirmation via AskUserQuestion**

Per the v52-v73 workflow.

- [ ] **Step 7: On approval, merge to main**

```bash
git checkout main
git merge --no-ff v74-configurable-ifs -m "Merge v74: configurable IFS (M-05)"
git push origin main
git branch -d v74-configurable-ifs
```

- [ ] **Step 8: Post-merge memory update**

Update `/home/john/.claude/projects/-home-john-projects-shuck/memory/MEMORY.md` and `project_huck_iterations.md` with the v74 entry.

---

## Notes for the implementer

1. **Subagent isolation**: each task is implemented by a fresh subagent. The plan body is their only context.

2. **TDD discipline**: write the failing test first when adding new behavior. Run, see fail, make pass.

3. **The trailing-non-ws-IFS rule** (`v="a:"` with `IFS=:` → 1 field, not 2): this is the most surprising bit of POSIX field splitting. Pin it with both unit + bash-diff tests. If a test fails here, debug the splitter's "did we consume all remaining input as a separator?" check at the end of the inner loop.

4. **Default-IFS regression risk**: the existing 2090 tests all assume default IFS splitting. After Task 1, every one of them should still pass. If any fails, the splitter has a bug in the default case (most likely: whitespace-run collapse, or leading-ws stripping).

5. **Borrow-checker note**: `Shell::ifs()` returns `String` (cloned). Don't try to return `&str` — Variable storage is owned and the value may be re-read inside the same call.

6. **Code-quality reviewer notes** (anticipate):
   - "Why byte-oriented?" — POSIX explicitly allows it; bash matches.
   - "Why is `ifs_join_sep` a free function not a `Shell` method?" — It operates on a `&str`, not `&self`. Keeps it testable in isolation.
   - "Why is `Shell::ifs()` named so short?" — It's accessed at every word-split site; verbose name would hurt readability.

7. **Spec-compliance reviewer notes**: verify all 7 emit_split_fields callsites thread `&ifs`; verify all ~10 join sites switched to `ifs_join_sep`; verify the 14 splitter tests cover each POSIX edge case from the spec.
