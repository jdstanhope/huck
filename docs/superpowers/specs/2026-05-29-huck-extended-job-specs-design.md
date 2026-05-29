# huck v47 — Extended job specs `%cmd` / `%?cmd` (M-62)

## Goal

Add bash-style extended job specs to huck:

- `%cmd` — matches the (unique) job whose command starts with `cmd`.
- `%?cmd` — matches the (unique) job whose command contains `cmd`.

Multiple matches → "ambiguous job spec" error (bash-faithful).

This is a new tracked divergence: **M-62: Extended job specs**.
M-61 was the brace-expansion entry added in v46; v47 takes M-62.

## Scope decisions (locked)

1. **Ambiguity handling**: bash-faithful — error with "ambiguous
   job spec" + status 1 when more than one job matches.
2. **Match scope**: full command string (including args), matching
   bash. huck stores the full command in `Job.command`.
3. **Case sensitivity**: case-sensitive substring/prefix match
   (bash-compat).

## Out of scope (deferred)

- Glob or regex matching inside `%cmd` (bash doesn't do this).
- `%cmd` matching argv[0] only (bash matches full string).
- Allowing whitespace between `%?` and the pattern (`%? cmd`); only
  `%?cmd` (no space) is supported.

## Architecture

Three files change:

1. **`src/job_spec.rs`** — extend `JobSpec` enum + `parse_job_spec`.
2. **`src/jobs.rs`** — change `JobTable::resolve` signature from
   `Option<u32>` to `Result<u32, JobSpecResolveError>` and add
   the new error enum + Prefix/Substring resolution logic.
3. **`src/builtins.rs`** — adjust `resolve_spec_or_error` to map
   the new `Result` to the existing error patterns plus the new
   "ambiguous job spec" message.

### Updated `JobSpec` enum

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

Note: dropped the `Copy` derive (was on the old enum) because String
isn't Copy. Verify no callers rely on Copy semantics.

### Updated `parse_job_spec`

```rust
pub fn parse_job_spec(s: &str) -> Result<JobSpec, JobSpecError> {
    let rest = match s.strip_prefix('%') {
        Some(r) => r,
        None => return Err(JobSpecError::BadSymbol),
    };
    if rest.is_empty() {
        return Err(JobSpecError::Empty);
    }
    match rest {
        "+" | "%" => return Ok(JobSpec::Current),
        "-" => return Ok(JobSpec::Previous),
        _ => {}
    }
    if rest.starts_with('-') {
        // `%-1` etc. — anything starting with '-' beyond the plain
        // `%-` above is malformed.
        return Err(JobSpecError::BadSymbol);
    }
    if rest.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return rest
            .parse::<u32>()
            .map(JobSpec::Id)
            .map_err(|_| JobSpecError::BadNumber);
    }
    // v47: substring or prefix.
    if let Some(pattern) = rest.strip_prefix('?') {
        if pattern.is_empty() {
            return Err(JobSpecError::BadSymbol);
        }
        return Ok(JobSpec::Substring(pattern.to_string()));
    }
    Ok(JobSpec::Prefix(rest.to_string()))
}
```

### New `JobSpecResolveError` enum

In `src/jobs.rs`:

```rust
#[derive(Debug, PartialEq, Eq)]
pub enum JobSpecResolveError {
    NotFound,
    Ambiguous,
}
```

### Rewritten `JobTable::resolve`

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
        JobSpec::Current => self.current_id().ok_or(JobSpecResolveError::NotFound),
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

### Updated `resolve_spec_or_error` in `src/builtins.rs`

Current shape:

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

New shape:

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

Note: `JobSpecResolveError` must be `pub` to be accessible from
`src/builtins.rs`.

### Other callers of `resolve`

Search for `.jobs.resolve(` in the codebase. Likely only one site
(`resolve_spec_or_error`). If any other call sites exist, they
each need the same Option → Result migration. Add to plan if
found.

### Error message table

| Condition | Message | Status |
|---|---|---|
| `%sleep` no match | `huck: <builtin>: %sleep: no such job` | 1 |
| `%sleep` multiple matches | `huck: <builtin>: %sleep: ambiguous job spec` | 1 |
| `%?` alone | `huck: <builtin>: %?: bad job spec` | 1 |
| `%?find` no match | `huck: <builtin>: %?find: no such job` | 1 |

## Test plan

### Unit tests in `src/job_spec.rs`

4 new parser tests:

1. `parse_percent_word_is_prefix` — `parse_job_spec("%sleep")` →
   `Ok(JobSpec::Prefix("sleep".to_string()))`.
2. `parse_percent_question_word_is_substring` — `%?find` →
   `Ok(JobSpec::Substring("find".to_string()))`.
3. `parse_percent_question_alone_is_bad_symbol` — `%?` →
   `Err(JobSpecError::BadSymbol)`.
4. `parse_percent_question_with_spaces_in_pattern` — `%?ab cd` →
   `Ok(JobSpec::Substring("ab cd".to_string()))`.

Existing parser tests (`%1`, `%%`, `%-`, etc.) must still pass.
One existing test will break:
`parse_percent_letters_is_bad_symbol` (asserts `%abc` →
`BadSymbol`). Under v47 `%abc` is now `Prefix("abc")`. Repurpose
this test to assert the new behavior, OR delete and add a fresh
test.

### Unit tests in `src/jobs.rs`

6 new resolve tests in the existing `#[cfg(test)] mod tests`
block:

5. `resolve_prefix_unique_match`
6. `resolve_prefix_no_match`
7. `resolve_prefix_ambiguous`
8. `resolve_substring_unique_match`
9. `resolve_substring_no_match`
10. `resolve_substring_ambiguous`

Pattern: `JobTable::new()`, add jobs with known commands, call
`resolve(&JobSpec::Prefix(...))` or `Substring(...)`, assert on
the `Result`.

Existing resolve tests will need adjustment because the signature
changed from `Option` to `Result`. Update each existing test's
assertion form: `assert_eq!(t.resolve(...), Some(1))` becomes
`assert_eq!(t.resolve(...), Ok(1))`; `None` becomes
`Err(JobSpecResolveError::NotFound)`.

### Integration tests in `tests/job_spec_extended_integration.rs`

3 binary-driven tests:

1. `disown_prefix_match` — script:
   `sleep 30 >/dev/null 2>&1 &\necho ready\ndisown %sleep\necho $?\nexit\n`.
   Stdout contains `0` line.
2. `disown_ambiguous_errors` — script:
   `sleep 30 >/dev/null 2>&1 &\nsleep 60 >/dev/null 2>&1 &\ndisown %sleep\necho $?\nexit\n`.
   Stderr contains `ambiguous`; stdout has line `1`.
3. `jobs_substring_filter_via_spec` — script:
   `sleep 30 >/dev/null 2>&1 &\njobs %?sleep\nexit\n`.
   Stdout contains `sleep`.

Note: `jobs %spec` is from v45 (positional `%spec` filter on
`jobs`). Verifies extended-spec resolution flows through that path
as well.

### Smoke

`cargo test --all-targets` must pass. PTY flake tolerated.

## Implementation tasks

1. **Parser + resolve + caller + unit tests**:
   - Extend `JobSpec` and `parse_job_spec` in `src/job_spec.rs`.
   - Add `JobSpecResolveError` in `src/jobs.rs`.
   - Rewrite `JobTable::resolve`.
   - Update `resolve_spec_or_error` in `src/builtins.rs`.
   - Adjust existing `resolve` tests for the signature change.
   - Adjust `parse_percent_letters_is_bad_symbol` test (now
     `%abc` is `Prefix`).
   - Add 4 parser unit tests + 6 resolve unit tests.
2. **Integration tests**: create
   `tests/job_spec_extended_integration.rs` with 3 tests.
3. **Docs**: new M-62 entry; change-log entry; README v47 row;
   trim "extended job specs" from "Not yet implemented" stanza.

Three tasks. TDD within each.

## Acceptance criteria

- All 10 new unit tests pass.
- All 3 integration tests pass.
- All pre-existing tests still pass (after the resolve-signature
  migration and the `%abc` → Prefix repurposing).
- `cargo test --all-targets` passes (modulo PTY flake).
- `cargo clippy --all-targets -- -D warnings` passes.
- `docs/bash-divergences.md` has the new M-62 entry as
  `[fixed v47]`.
- `disown %sleep` works for unique-match cases; errors with
  "ambiguous job spec" when multiple match; errors with "no such
  job" when no match.
