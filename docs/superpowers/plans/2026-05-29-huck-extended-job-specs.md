# huck v47 — Extended Job Specs Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add bash extended job specs `%cmd` (prefix) and `%?cmd`
(substring) to huck, with bash-faithful "ambiguous job spec" error
on multi-match.

**Architecture:** Three files change. `src/job_spec.rs` extends
`JobSpec` with `Prefix(String)` and `Substring(String)` variants
plus parser updates. `src/jobs.rs` introduces
`JobSpecResolveError { NotFound, Ambiguous }` and changes
`JobTable::resolve` to return `Result<u32, JobSpecResolveError>`.
`src/builtins.rs::resolve_spec_or_error` is the single call-site
that needs the Option→Result migration plus a new arm for
Ambiguous.

**Tech Stack:** Rust. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-05-29-huck-extended-job-specs-design.md`

**Branch:** `v47-extended-job-specs` (created in preamble step P.1).

**Commit trailer convention**:

```
Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
```

---

## Preamble: Create the working branch

- [ ] **Step P.1: Create branch from main and check it out**

```bash
git checkout main
git pull --ff-only
git checkout -b v47-extended-job-specs
```

Expected: `Switched to a new branch 'v47-extended-job-specs'`.

The spec + this plan are committed as the first commit on this
branch (handled by the controller before Task 1 begins).

---

## Task 1: Parser + resolve + caller + unit tests

**Files:**
- Modify: `src/job_spec.rs` — extend enum, parser, tests.
- Modify: `src/jobs.rs` — add `JobSpecResolveError`, rewrite
  `resolve`, migrate 5 existing resolve tests, add 6 new tests.
- Modify: `src/builtins.rs:895-906` — update
  `resolve_spec_or_error`.

### Step 1.1: Extend `JobSpec` enum in `src/job_spec.rs`

Replace the existing enum (currently `#[derive(... Clone, Copy)]`):

```rust
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum JobSpec {
    Id(u32),
    Current,
    Previous,
}
```

With:

```rust
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum JobSpec {
    Id(u32),
    Current,
    Previous,
    Prefix(String),
    Substring(String),
}
```

The `Copy` derive is dropped because `String` isn't `Copy`. If
the build breaks due to a downstream caller that depended on
Copy semantics (e.g. takes the JobSpec by value twice), fix the
caller by `.clone()`ing — but in practice the only caller is
`JobTable::resolve(&JobSpec)` which takes a reference, so this
should be a no-op.

- [ ] **Step 1.1: Extend the enum**

### Step 1.2: Extend `parse_job_spec`

In `src/job_spec.rs`, find `pub fn parse_job_spec`. Replace the
final `Err(JobSpecError::BadSymbol)` line at the end of the
function with the new prefix/substring fallback:

The current end of the function:

```rust
    if rest.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return rest
            .parse::<u32>()
            .map(JobSpec::Id)
            .map_err(|_| JobSpecError::BadNumber);
    }
    Err(JobSpecError::BadSymbol)
}
```

Replace with:

```rust
    if rest.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return rest
            .parse::<u32>()
            .map(JobSpec::Id)
            .map_err(|_| JobSpecError::BadNumber);
    }
    // v47: substring (%?cmd) or prefix (%cmd).
    if let Some(pattern) = rest.strip_prefix('?') {
        if pattern.is_empty() {
            return Err(JobSpecError::BadSymbol);
        }
        return Ok(JobSpec::Substring(pattern.to_string()));
    }
    Ok(JobSpec::Prefix(rest.to_string()))
}
```

- [ ] **Step 1.2: Extend the parser**

### Step 1.3: Build to confirm `src/job_spec.rs` compiles

Run: `cargo build`
Expected: build fails on `src/jobs.rs:195` (existing `resolve`
matches the old 3-variant enum non-exhaustively; the new variants
aren't covered yet). Proceed to step 1.4 to extend `resolve`.

If the build succeeds: Rust may have inferred wildcard fallthrough.
Proceed.

- [ ] **Step 1.3: Confirm enum extension**

### Step 1.4: Adjust the broken parser test

In `src/job_spec.rs`, find the test `parse_percent_letters_is_bad_symbol`
(asserts `%abc` → `BadSymbol`). Under v47, `%abc` is now
`Prefix("abc")`. Repurpose the test:

```rust
    #[test]
    fn parse_percent_letters_is_prefix() {
        assert_eq!(
            parse_job_spec("%abc"),
            Ok(JobSpec::Prefix("abc".to_string()))
        );
    }
```

- [ ] **Step 1.4: Update broken test**

### Step 1.5: Add 4 new parser tests

In `src/job_spec.rs::mod tests`, append:

```rust
    #[test]
    fn parse_percent_word_is_prefix() {
        assert_eq!(
            parse_job_spec("%sleep"),
            Ok(JobSpec::Prefix("sleep".to_string()))
        );
    }

    #[test]
    fn parse_percent_question_word_is_substring() {
        assert_eq!(
            parse_job_spec("%?find"),
            Ok(JobSpec::Substring("find".to_string()))
        );
    }

    #[test]
    fn parse_percent_question_alone_is_bad_symbol() {
        assert_eq!(parse_job_spec("%?"), Err(JobSpecError::BadSymbol));
    }

    #[test]
    fn parse_percent_question_with_spaces_in_pattern() {
        assert_eq!(
            parse_job_spec("%?ab cd"),
            Ok(JobSpec::Substring("ab cd".to_string()))
        );
    }
```

- [ ] **Step 1.5: Append new parser tests**

### Step 1.6: Run parser tests

Run: `cargo test --bin huck job_spec:: -- --nocapture`
Expected: all parser tests pass (existing + 4 new + the repurposed
one).

Build won't fully succeed until `resolve` is updated — if `cargo
build` is blocking the test runner, comment out the `match spec`
arms in `JobTable::resolve` temporarily, OR proceed straight to
Step 1.7.

- [ ] **Step 1.6: Parser tests green**

### Step 1.7: Add `JobSpecResolveError` enum in `src/jobs.rs`

In `src/jobs.rs`, near the top (after the existing
`#[derive(...)] pub enum JobState`), add:

```rust
#[derive(Debug, PartialEq, Eq)]
pub enum JobSpecResolveError {
    NotFound,
    Ambiguous,
}
```

- [ ] **Step 1.7: Add the error enum**

### Step 1.8: Rewrite `JobTable::resolve`

In `src/jobs.rs:195-205`, replace the existing `resolve` method:

```rust
    pub fn resolve(&self, spec: &crate::job_spec::JobSpec) -> Option<u32> {
        match spec {
            crate::job_spec::JobSpec::Id(id) => {
                self.jobs.iter().find(|j| j.id == *id).map(|j| j.id)
            }
            crate::job_spec::JobSpec::Current => self.current_id(),
            crate::job_spec::JobSpec::Previous => {
                let (_, prev) = self.current_and_previous();
                prev
            }
        }
    }
```

With:

```rust
    pub fn resolve(
        &self,
        spec: &crate::job_spec::JobSpec,
    ) -> Result<u32, JobSpecResolveError> {
        use crate::job_spec::JobSpec;
        match spec {
            JobSpec::Id(id) => self
                .jobs
                .iter()
                .find(|j| j.id == *id)
                .map(|j| j.id)
                .ok_or(JobSpecResolveError::NotFound),
            JobSpec::Current => self
                .current_id()
                .ok_or(JobSpecResolveError::NotFound),
            JobSpec::Previous => {
                let (_, prev) = self.current_and_previous();
                prev.ok_or(JobSpecResolveError::NotFound)
            }
            JobSpec::Prefix(p) => {
                let matches: Vec<u32> = self
                    .jobs
                    .iter()
                    .filter(|j| j.command.starts_with(p.as_str()))
                    .map(|j| j.id)
                    .collect();
                match matches.len() {
                    0 => Err(JobSpecResolveError::NotFound),
                    1 => Ok(matches[0]),
                    _ => Err(JobSpecResolveError::Ambiguous),
                }
            }
            JobSpec::Substring(p) => {
                let matches: Vec<u32> = self
                    .jobs
                    .iter()
                    .filter(|j| j.command.contains(p.as_str()))
                    .map(|j| j.id)
                    .collect();
                match matches.len() {
                    0 => Err(JobSpecResolveError::NotFound),
                    1 => Ok(matches[0]),
                    _ => Err(JobSpecResolveError::Ambiguous),
                }
            }
        }
    }
```

- [ ] **Step 1.8: Rewrite resolve**

### Step 1.9: Migrate the 5 existing resolve tests

In `src/jobs.rs::mod tests` (around line 671), find and update the
5 existing resolve tests. Each currently uses `Some(N)` or `None`;
each needs `Ok(N)` or `Err(JobSpecResolveError::NotFound)`.

Find `fn resolve_id_returns_matching_id`. Change:
```rust
        assert_eq!(t.resolve(&spec), Some(2));
```
to:
```rust
        assert_eq!(t.resolve(&spec), Ok(2));
```

Find `fn resolve_id_missing_returns_none`. Rename to
`resolve_id_missing_returns_not_found` and change:
```rust
        assert_eq!(t.resolve(&spec), None);
```
to:
```rust
        assert_eq!(t.resolve(&spec), Err(JobSpecResolveError::NotFound));
```

Find `fn resolve_current_uses_current_id`. Change:
```rust
        assert_eq!(t.resolve(&crate::job_spec::JobSpec::Current), Some(2));
```
to:
```rust
        assert_eq!(t.resolve(&crate::job_spec::JobSpec::Current), Ok(2));
```

Find `fn resolve_previous_returns_second_most_recent`. Change:
```rust
        assert_eq!(t.resolve(&crate::job_spec::JobSpec::Previous), Some(1));
```
to:
```rust
        assert_eq!(t.resolve(&crate::job_spec::JobSpec::Previous), Ok(1));
```

Find `fn resolve_previous_returns_none_when_only_one_job`. Rename
to `resolve_previous_returns_not_found_when_only_one_job` and
change:
```rust
        assert_eq!(t.resolve(&crate::job_spec::JobSpec::Previous), None);
```
to:
```rust
        assert_eq!(t.resolve(&crate::job_spec::JobSpec::Previous), Err(JobSpecResolveError::NotFound));
```

The `JobSpecResolveError` is in scope inside the test mod because
the mod has `use super::*;` (existing).

- [ ] **Step 1.9: Migrate 5 existing tests**

### Step 1.10: Add 6 new resolve tests

In `src/jobs.rs::mod tests`, append after the existing resolve
tests:

```rust
    #[test]
    fn resolve_prefix_unique_match() {
        let mut t = JobTable::new();
        t.add(1234, vec![1234], "sleep 30".to_string());
        let spec = crate::job_spec::JobSpec::Prefix("sleep".to_string());
        assert_eq!(t.resolve(&spec), Ok(1));
    }

    #[test]
    fn resolve_prefix_no_match() {
        let mut t = JobTable::new();
        t.add(1234, vec![1234], "sleep 30".to_string());
        let spec = crate::job_spec::JobSpec::Prefix("xyz".to_string());
        assert_eq!(t.resolve(&spec), Err(JobSpecResolveError::NotFound));
    }

    #[test]
    fn resolve_prefix_ambiguous() {
        let mut t = JobTable::new();
        t.add(1234, vec![1234], "sleep 30".to_string());
        t.add(1235, vec![1235], "sleep 60".to_string());
        let spec = crate::job_spec::JobSpec::Prefix("sleep".to_string());
        assert_eq!(t.resolve(&spec), Err(JobSpecResolveError::Ambiguous));
    }

    #[test]
    fn resolve_substring_unique_match() {
        let mut t = JobTable::new();
        t.add(1234, vec![1234], "find . -name foo".to_string());
        let spec = crate::job_spec::JobSpec::Substring("name".to_string());
        assert_eq!(t.resolve(&spec), Ok(1));
    }

    #[test]
    fn resolve_substring_no_match() {
        let mut t = JobTable::new();
        t.add(1234, vec![1234], "find . -name foo".to_string());
        let spec = crate::job_spec::JobSpec::Substring("xyz".to_string());
        assert_eq!(t.resolve(&spec), Err(JobSpecResolveError::NotFound));
    }

    #[test]
    fn resolve_substring_ambiguous() {
        let mut t = JobTable::new();
        t.add(1234, vec![1234], "find . -name foo".to_string());
        t.add(1235, vec![1235], "grep foo bar".to_string());
        let spec = crate::job_spec::JobSpec::Substring("foo".to_string());
        assert_eq!(t.resolve(&spec), Err(JobSpecResolveError::Ambiguous));
    }
```

- [ ] **Step 1.10: Append 6 new resolve tests**

### Step 1.11: Update `resolve_spec_or_error` in `src/builtins.rs`

In `src/builtins.rs:895-906`, find the function:

```rust
fn resolve_spec_or_error(
    arg: &str,
    builtin: &str,
    shell: &Shell,
) -> Result<u32, ExecOutcome> {
    let spec = crate::job_spec::parse_job_spec(arg).map_err(|_| {
        eprintln!("huck: {builtin}: {arg}: bad job spec");
        ExecOutcome::Continue(1)
    })?;
    shell.jobs.resolve(&spec).ok_or_else(|| {
        eprintln!("huck: {builtin}: {arg}: no such job");
        ExecOutcome::Continue(1)
    })
}
```

Replace with:

```rust
fn resolve_spec_or_error(
    arg: &str,
    builtin: &str,
    shell: &Shell,
) -> Result<u32, ExecOutcome> {
    let spec = crate::job_spec::parse_job_spec(arg).map_err(|_| {
        eprintln!("huck: {builtin}: {arg}: bad job spec");
        ExecOutcome::Continue(1)
    })?;
    match shell.jobs.resolve(&spec) {
        Ok(id) => Ok(id),
        Err(crate::jobs::JobSpecResolveError::NotFound) => {
            eprintln!("huck: {builtin}: {arg}: no such job");
            Err(ExecOutcome::Continue(1))
        }
        Err(crate::jobs::JobSpecResolveError::Ambiguous) => {
            eprintln!("huck: {builtin}: {arg}: ambiguous job spec");
            Err(ExecOutcome::Continue(1))
        }
    }
}
```

- [ ] **Step 1.11: Update the caller**

### Step 1.12: Build

Run: `cargo build`
Expected: clean.

- [ ] **Step 1.12: Build clean**

### Step 1.13: Run job_spec + jobs tests

Run: `cargo test --bin huck job_spec:: jobs::tests::resolve_ -- --nocapture`
Expected: all parser tests (existing + 4 new + 1 repurposed) and
all resolve tests (5 migrated + 6 new) pass.

- [ ] **Step 1.13: Targeted tests green**

### Step 1.14: Full unit suite

Run: `cargo test --bin huck`
Expected: all unit tests pass.

If a builtin test (`disown_with_no_such_spec_errors_status_1`,
`kill_with_no_such_spec_errors_status_1`, similar) fails because
the new resolve returns `Err(NotFound)` instead of `None`: the
error path now goes through the new `match` arm, but the status
code (1) and stderr text ("no such job") are identical. Tests
that assert only on status should still pass.

- [ ] **Step 1.14: Full unit suite passes**

### Step 1.15: Clippy

Run: `cargo clippy --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 1.15: Clippy clean**

### Step 1.16: Commit

```bash
git add src/job_spec.rs src/jobs.rs src/builtins.rs
git commit -m "$(cat <<'EOF'
builtin: extended job specs %cmd / %?cmd (v47 task 1)

Extend JobSpec enum (src/job_spec.rs) with Prefix(String) and
Substring(String) variants. parse_job_spec now recognizes:
- %cmd → Prefix("cmd") (previously rejected as BadSymbol)
- %?cmd → Substring("cmd")
- %? alone → BadSymbol (no pattern after `?`)

The existing parser test parse_percent_letters_is_bad_symbol is
repurposed to assert the new Prefix behavior.

New JobSpecResolveError enum in src/jobs.rs with NotFound and
Ambiguous variants. JobTable::resolve signature changes from
Option<u32> to Result<u32, JobSpecResolveError>. Prefix and
Substring resolution count matching jobs (by job.command
starts_with / contains); 0 = NotFound, 1 = Ok, >=2 = Ambiguous
(bash-faithful).

The single call site (resolve_spec_or_error in src/builtins.rs)
migrates from .ok_or_else to a match that distinguishes the two
error cases. New "huck: <builtin>: <arg>: ambiguous job spec"
stderr line for the Ambiguous arm; "no such job" path
unchanged.

Five existing resolve tests migrated from Some/None to Ok/Err.
Two tests renamed (`*_returns_none*` → `*_returns_not_found*`).
4 new parser tests + 6 new resolve tests cover prefix/substring
unique/no-match/ambiguous.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 1.16: Commit Task 1**

---

## Task 2: Integration tests

**Files:**
- Create: `tests/job_spec_extended_integration.rs`

### Step 2.1: Create the integration test file

Create `tests/job_spec_extended_integration.rs` with this content:

```rust
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_binary() -> String {
    env!("CARGO_BIN_EXE_huck").to_string()
}

fn run_capture(script: &str) -> (String, String) {
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
fn disown_prefix_match() {
    // sleep 30 backgrounded, then `disown %sleep` resolves to it
    // via the new prefix path; exit status 0.
    let script = "sleep 30 >/dev/null 2>&1 &\ndisown %sleep\necho $?\nexit\n";
    let (out, _) = run_capture(script);
    assert!(
        out.lines().any(|l| l == "0"),
        "expected status 0 in: {:?}",
        out
    );
}

#[test]
fn disown_ambiguous_errors() {
    // Two `sleep` jobs; `disown %sleep` should error with "ambiguous
    // job spec" and exit 1. Capture the rc explicitly so the
    // following echo doesn't clobber $?.
    let script = "sleep 30 >/dev/null 2>&1 &\nsleep 60 >/dev/null 2>&1 &\ndisown %sleep\nrc=$?\necho rc=$rc\nexit\n";
    let (out, err) = run_capture(script);
    assert!(
        out.lines().any(|l| l == "rc=1"),
        "expected rc=1 in stdout: {:?}; stderr: {:?}",
        out,
        err
    );
    assert!(
        err.contains("ambiguous"),
        "expected stderr to contain 'ambiguous': {:?}",
        err
    );
}

#[test]
fn jobs_substring_filter_via_spec() {
    // `jobs %?sleep` should find the sleep bg job via substring
    // match. Stdout should include `sleep` (from the command
    // column of the job listing).
    let script = "sleep 30 >/dev/null 2>&1 &\njobs %?sleep\nexit\n";
    let (out, _) = run_capture(script);
    assert!(
        out.contains("sleep"),
        "expected stdout to contain 'sleep': {:?}",
        out
    );
}
```

- [ ] **Step 2.1: Create the test file**

### Step 2.2: Run the integration suite

Run: `cargo test --test job_spec_extended_integration -- --nocapture`
Expected: all 3 tests pass.

If a test fails because `disown %sleep` doesn't resolve: inspect
actual stderr. Most likely culprit is the resolve_spec_or_error
update missing a return path. Do NOT relax assertions — fix
Task 1.

- [ ] **Step 2.2: Tests pass**

### Step 2.3: Full integration suite

Run: `cargo test --tests`
Expected: all integration tests pass. PTY flake tolerated.

- [ ] **Step 2.3: Full integration suite green**

### Step 2.4: Clippy

Run: `cargo clippy --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 2.4: Clippy clean**

### Step 2.5: Commit

```bash
git add tests/job_spec_extended_integration.rs
git commit -m "$(cat <<'EOF'
test: extended job specs integration coverage (v47 task 2)

Three binary-driven tests verifying the new %cmd / %?cmd paths
end-to-end. disown_prefix_match verifies `disown %sleep` resolves
to a unique-match background job. disown_ambiguous_errors verifies
that two `sleep` jobs trigger the "ambiguous job spec" error path
(stderr contains "ambiguous"; rc=1). jobs_substring_filter_via_spec
verifies `jobs %?sleep` substring-matches the bg sleep job.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 2.5: Commit Task 2**

---

## Task 3: Docs

**Files:**
- Modify: `docs/bash-divergences.md` — add new M-62 entry,
  change-log entry.
- Modify: `README.md` — v47 row + trim "extended job specs" from
  "Not yet implemented" stanza.

### Step 3.1: Add M-62 entry in `docs/bash-divergences.md`

Extended job specs don't currently have a tracked M-* entry — we
add M-62 as a NEW entry (M-61 was claimed by v46's brace
expansion).

Find the appropriate section. The "Job control" subsection is the
natural home — search for `### Job control` or similar. If the
file uses a different organizational style, place the entry next
to other job-spec-related M-* entries (likely near M-37/M-38
wait, M-39 kill -l, M-40 kill -s, M-43 disown flags, M-44 disown
bare PID, M-45 jobs flags).

Add this entry:

```markdown
- **M-62: Extended job specs `%cmd` / `%?cmd`** — `[fixed v47]` medium. `%cmd` resolves to the unique job whose command starts with "cmd"; `%?cmd` resolves to the unique job whose command contains "cmd". Bash-faithful "ambiguous job spec" error when multiple jobs match (status 1). Empty pattern (`%?` alone) is a bad-spec parse error. `JobTable::resolve` signature changed from `Option<u32>` to `Result<u32, JobSpecResolveError>`; the single internal caller (`resolve_spec_or_error` in src/builtins.rs) updated. All builtins that take `%spec` args (fg, bg, wait, kill, disown, jobs) gain the new behavior transparently.
```

- [ ] **Step 3.1: Add M-62 entry**

### Step 3.2: Add v47 change-log entry

In `docs/bash-divergences.md`, find `## Change log` and the most
recent `**2026-05-29**` entry (v46, M-61 brace expansion). Add
IMMEDIATELY after it:

```markdown
- **2026-05-29**: M-62 (extended job specs `%cmd` / `%?cmd`) shipped as v47. `JobSpec` enum extended with `Prefix(String)` and `Substring(String)` variants; `parse_job_spec` recognizes the new forms (the previously-error `%abc` becomes a valid `Prefix`). New `JobSpecResolveError { NotFound, Ambiguous }` enum; `JobTable::resolve` signature changed from `Option<u32>` to `Result<u32, JobSpecResolveError>`. Single call site (`resolve_spec_or_error`) updated to surface the Ambiguous arm as `huck: <builtin>: <arg>: ambiguous job spec` + status 1. No new L-* divergences.
```

- [ ] **Step 3.2: Add change-log entry**

### Step 3.3: Add v47 row to README

In `README.md`, find the version table. After the v46 row (search
for `| v46       |`), add IMMEDIATELY after it:

```markdown
| v47       | Extended job specs `%cmd`/`%?cmd` (M-62)                       |
```

Match column padding to v45/v46 (count actual trailing spaces in
the file).

- [ ] **Step 3.3: Add README v47 row**

### Step 3.4: Trim `extended job specs` from "Not yet implemented"

In `README.md`, find the block around lines ~233-238. Post-v46
should read:

```markdown
**Not yet implemented:**
extended job specs (`%cmd`/`%?cmd`), backgrounded multi-pipeline
sequences (`cmd1 && cmd2 &`), aliases.
```

Replace with:

```markdown
**Not yet implemented:**
backgrounded multi-pipeline sequences (`cmd1 && cmd2 &`), aliases.
```

- [ ] **Step 3.4: Trim README stanza**

### Step 3.5: Full suite

Run: `cargo test --all-targets`
Expected: all tests pass (modulo PTY flake).

- [ ] **Step 3.5: Full suite green**

### Step 3.6: Clippy

Run: `cargo clippy --all-targets -- -D warnings`
Expected: zero warnings.

- [ ] **Step 3.6: Clippy clean**

### Step 3.7: Commit

```bash
git add docs/bash-divergences.md README.md
git commit -m "$(cat <<'EOF'
docs: add M-62 (extended job specs) fixed v47; trim stale entry

New M-62 entry in docs/bash-divergences.md tracks extended job
specs as [fixed v47]. Covers %cmd (prefix), %?cmd (substring),
the bash-faithful ambiguous-match error, and the
resolve-signature migration from Option to Result.

Change log: 2026-05-29 v47 entry summarizing the JobSpec enum
extension, JobSpecResolveError, and resolve_spec_or_error
update.

README: v47 row added to the version table; "Not yet implemented"
stanza trimmed to remove extended job specs (shipped this
iteration).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 3.7: Commit Task 3**

---

## Final verification (controller, not a task)

After the three task commits land:

1. Run `cargo test --all-targets` once more.
2. Run `cargo clippy --all-targets -- -D warnings`.
3. Confirm the branch has exactly four commits ahead of `main`:
   docs preamble (spec + plan), task 1, task 2, task 3.
4. Dispatch a final cross-task code-reviewer subagent over the
   full diff (`main..v47-extended-job-specs`).
5. Merge to `main` with `--no-ff`, push, delete the branch, update
   the `huck iterations` memory with v47.
