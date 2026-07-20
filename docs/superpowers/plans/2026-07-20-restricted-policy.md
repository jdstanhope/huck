# v319 — Restricted Shell Policy Abstraction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace huck's ~9 hand-placed `is_restricted() && check_X()` sites with a `Policy`/`Op` enum pair on `Shell`, correct the restricted-mode semantics to match bash 5.2.21, and add the missing `rbash` / `-r` / `set -r` entry points.

**Architecture:** A new `policy.rs` module owns a `Policy` enum (`Unrestricted` | `Rbash` | `Sandbox`) and an `Op` enum of guarded operations. Every enforcement site collapses to `shell.policy.check(op)?`, with `Unrestricted` returning `Ok(())` from the first match arm. Variable restriction is *not* an `Op` — it is implemented by marking `SHELL`/`PATH`/`HISTFILE`/`ENV`/`BASH_ENV` readonly on entry, reusing `Shell::mark_readonly`.

**Tech Stack:** Rust (2024 edition, let-chains in use), `crates/huck-engine` + `crates/huck-cli`, bash-diff shell harnesses under `tests/scripts/`.

**Spec:** `docs/superpowers/specs/2026-07-20-restricted-policy-design.md`
**Issue:** [#222](https://github.com/jdstanhope/huck/issues/222)

## Global Constraints

- Every commit ends with the trailer `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.
- Run `cargo fmt --all` before every commit — CI enforces `cargo fmt --all --check`.
- **Never** run `cargo test --workspace` — this box (1 core / 1.9GB) OOM-kills the session. Always `cargo test -p <crate> --jobs 1 --lib -- --test-threads 1`.
- Build the binary with `cargo build -p huck --bin huck`.
- Guard harness sweeps with `ulimit -v 1500000` and a `timeout`.
- All error messages are **body-only** (no `huck:` / invocation-name prefix) and are emitted via `sh_error!` / `sh_error_to!`. The "callers translate" contract is unchanged.
- Target fidelity is **bash 5.2.21** (`ubuntu-24.04`, huck's compat target). Where `man bash` and the real shell disagree, the real shell wins — see the spec's "Ground truth" section.
- Do not attempt `>& file` coverage: huck does not implement it (issue [#223](https://github.com/jdstanhope/huck/issues/223), filed during planning). The other five output-redirect operators are in scope.

## Codebase Orientation

Read this before Task 1. It is the result of an audit done during planning — trust it over grepping from scratch.

**The 9 existing enforcement sites**, all of shape `if is_restricted(shell) && let Err(msg) = check_X()`:

| # | Site | Op |
|---|---|---|
| 1 | `builtins.rs:468` (`builtin_cd`) | cd |
| 2 | `builtins.rs:6327` (`builtin_set_inner`) | `set +r` |
| 3 | `builtins.rs:7367` (`source_in_sink`) | source path |
| 4 | `executor.rs:55` (`check_restricted_redirect`) | redirect |
| 5 | `executor.rs:4619` | exec |
| 6 | `executor.rs:4661` | command name |
| 7 | `shell_state.rs:1427` | special assign |
| 8 | `shell_state.rs:2145` | special assign |
| 9 | `restricted.rs` itself | the helpers |

**Redirect enforcement is already centralized** — this is important and better than the spec assumed. `check_restricted_redirect` has exactly two callers, both inside `lower_one_redirect` (`executor.rs:5237` for the `{var}`-fd arm, `executor.rs:5413` for the numbered-fd arm). The child-side path funnels through it too: `build_child_redir_plan` → `lower_redirects` → `lower_one_redirect`. So there is **no** fg/bg/subshell/capture duplication to chase here; fixing those two arms covers every path.

**`FileMode`** (`crates/huck-syntax/src/command.rs:231`) has five variants: `ReadOnly`, `Truncate`, `Append`, `Clobber`, `ReadWrite`. The first is input-only and must stay permitted. `RedirOp::Dup` is the `>&N` / `<&N` fd-duplication form and never reaches the file gate — that is what makes `>&2` and `2>&1` allowed for free.

**Readonly machinery already exists**: `Shell::mark_readonly(&mut self, name: &str)` at `shell_state.rs:2350`, `Shell::is_readonly(&self, name: &str) -> bool` at `shell_state.rs:1854`. The write paths that consult it are at `shell_state.rs:2153` and `shell_state.rs:2326`.

**Entry-point precedent**: `repl.rs:101` calls `huck_engine::shell::startup_posix(opts.posix, &argv0, POSIXLY_CORRECT)`. Task 5 adds `startup_restricted` right beside it in the same shape.

**`shopt restricted_shell`** already exists as a table entry at `shell_state.rs:527` (`default: false`).

---

## File Structure

- **Create** `crates/huck-engine/src/policy.rs` — the `Policy` + `Op` enums, `check`, the readonly-marking entry helper, and the policy matrix test. Sole owner of restricted-mode decisions.
- **Delete** `crates/huck-engine/src/restricted.rs` — fully replaced.
- **Modify** `crates/huck-engine/src/shell_state.rs` — `restricted: bool` → `policy: Policy`; drop the two `check_special_assign` sites; `shopt restricted_shell` reads the policy.
- **Modify** `crates/huck-engine/src/executor.rs` — redirect, exec, command-name sites.
- **Modify** `crates/huck-engine/src/builtins.rs` — cd, source, `set -r`/`set +r` sites.
- **Modify** `crates/huck-engine/src/shell.rs` — `startup_restricted` helper.
- **Modify** `crates/huck-cli/src/repl.rs` — `-r` flag parsing + argv0 detection.
- **Modify** `crates/huck-engine/src/exec_builder.rs` — `restricted()` sets `Policy::Sandbox`.
- **Create** `tests/scripts/rbash_diff_check.sh` — the bash-diff harness.

---

## Task 1: The policy module

**Files:**
- Create: `crates/huck-engine/src/policy.rs`
- Modify: `crates/huck-engine/src/lib.rs` (add `pub mod policy;`)

**Interfaces:**
- Consumes: nothing (first task).
- Produces: `policy::Policy` (`Unrestricted`, `Rbash`, `Sandbox`; `Clone + Copy + PartialEq + Eq + Debug`), `policy::Op<'a>` (`Cd`, `Exec`, `CommandName(&'a str)`, `SourcePath(&'a str)`, `RedirectFile { path: &'a str }`, `DisableRestricted`), `Policy::check(&self, op: Op<'_>) -> Result<(), String>`, `Policy::is_restricted(&self) -> bool`, `policy::RESTRICTED_READONLY_VARS: [&str; 5]`.

This task creates the module standalone with its full test matrix. No call sites change yet — the old `restricted.rs` keeps working until Task 3.

- [ ] **Step 1: Write the failing test**

Create `crates/huck-engine/src/policy.rs` containing *only* the test module below (the implementation lands in Step 3). This is the policy matrix: every `Op` × every `Policy`.

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// The full decision matrix. Adding an `Op` variant should force a new
    /// row here — that is the point of the enum over scattered conditionals.
    #[test]
    fn matrix_unrestricted_allows_everything() {
        let p = Policy::Unrestricted;
        assert!(p.check(Op::Cd).is_ok());
        assert!(p.check(Op::Exec).is_ok());
        assert!(p.check(Op::CommandName("/bin/echo")).is_ok());
        assert!(p.check(Op::SourcePath("/etc/profile")).is_ok());
        assert!(p.check(Op::RedirectFile { path: "/tmp/x" }).is_ok());
        assert!(p.check(Op::DisableRestricted).is_ok());
    }

    #[test]
    fn matrix_rbash_denies_all_guarded_ops() {
        let p = Policy::Rbash;
        assert!(p.check(Op::Cd).is_err());
        assert!(p.check(Op::Exec).is_err());
        assert!(p.check(Op::CommandName("/bin/echo")).is_err());
        assert!(p.check(Op::SourcePath("/etc/profile")).is_err());
        assert!(p.check(Op::DisableRestricted).is_err());
        // Rbash denies EVERY file-target redirect, relative ones included.
        assert!(p.check(Op::RedirectFile { path: "log" }).is_err());
        assert!(p.check(Op::RedirectFile { path: "/tmp/x" }).is_err());
    }

    #[test]
    fn matrix_rbash_allows_bare_names_and_slashless_source() {
        let p = Policy::Rbash;
        assert!(p.check(Op::CommandName("echo")).is_ok());
        assert!(p.check(Op::CommandName("my-cmd")).is_ok());
        assert!(p.check(Op::SourcePath("profile")).is_ok());
    }

    #[test]
    fn matrix_sandbox_denies_escaping_redirects_only() {
        let p = Policy::Sandbox;
        // Escape attempts refused.
        assert!(p.check(Op::RedirectFile { path: "/tmp/x" }).is_err());
        assert!(p.check(Op::RedirectFile { path: "../escape" }).is_err());
        assert!(p.check(Op::RedirectFile { path: "foo/../bar" }).is_err());
        // Local work permitted — this is the one behavioral difference from Rbash.
        assert!(p.check(Op::RedirectFile { path: "log" }).is_ok());
        assert!(p.check(Op::RedirectFile { path: "sub/log" }).is_ok());
        assert!(p.check(Op::RedirectFile { path: "./log" }).is_ok());
    }

    #[test]
    fn matrix_sandbox_matches_rbash_on_non_redirect_ops() {
        let p = Policy::Sandbox;
        assert!(p.check(Op::Cd).is_err());
        assert!(p.check(Op::Exec).is_err());
        assert!(p.check(Op::CommandName("/bin/echo")).is_err());
        assert!(p.check(Op::CommandName("echo")).is_ok());
        assert!(p.check(Op::SourcePath("/etc/profile")).is_err());
        assert!(p.check(Op::SourcePath("profile")).is_ok());
        assert!(p.check(Op::DisableRestricted).is_err());
    }

    /// Message bodies are bash's, verbatim. These strings are asserted
    /// byte-for-byte by tests/scripts/rbash_diff_check.sh against the real
    /// shell; if you change one, change it there too.
    #[test]
    fn messages_match_bash_wording() {
        let p = Policy::Rbash;
        assert_eq!(p.check(Op::Cd).unwrap_err(), "cd: restricted");
        assert_eq!(p.check(Op::Exec).unwrap_err(), "exec: restricted");
        assert_eq!(
            p.check(Op::CommandName("/bin/echo")).unwrap_err(),
            "/bin/echo: restricted: cannot specify `/' in command names"
        );
        assert_eq!(
            p.check(Op::SourcePath("/etc/profile")).unwrap_err(),
            ".: /etc/profile: restricted"
        );
        assert_eq!(
            p.check(Op::RedirectFile { path: "f" }).unwrap_err(),
            "f: restricted: cannot redirect output"
        );
    }

    /// Both policies share bash's vocabulary; they differ only in WHAT they deny.
    #[test]
    fn sandbox_uses_bash_wording_too() {
        let p = Policy::Sandbox;
        assert_eq!(p.check(Op::Cd).unwrap_err(), "cd: restricted");
        assert_eq!(
            p.check(Op::RedirectFile { path: "/tmp/x" }).unwrap_err(),
            "/tmp/x: restricted: cannot redirect output"
        );
    }

    #[test]
    fn is_restricted_reflects_policy() {
        assert!(!Policy::Unrestricted.is_restricted());
        assert!(Policy::Rbash.is_restricted());
        assert!(Policy::Sandbox.is_restricted());
    }

    #[test]
    fn readonly_var_set_matches_bash() {
        // bash marks exactly these five readonly when restriction engages.
        assert_eq!(
            RESTRICTED_READONLY_VARS,
            ["SHELL", "PATH", "HISTFILE", "ENV", "BASH_ENV"]
        );
    }
}
```

Add to `crates/huck-engine/src/lib.rs`, keeping the module list alphabetical:

```rust
pub mod policy;
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p huck-engine --jobs 1 --lib policy:: -- --test-threads 1
```

Expected: FAIL to compile — `cannot find type 'Policy' in this scope` (and the same for `Op`, `RESTRICTED_READONLY_VARS`).

- [ ] **Step 3: Write minimal implementation**

Prepend to `crates/huck-engine/src/policy.rs`, above the test module:

```rust
//! Restricted-mode policy. A `Policy` is the single authority on which
//! operations a shell may perform; enforcement sites ask it rather than
//! testing a flag and hand-rolling a refusal.
//!
//! Two restricted policies exist. `Rbash` mirrors bash's restricted shell
//! exactly (verified against 5.2.21). `Sandbox` is huck's embedding policy
//! (`ExecBuilder::restricted()`): it blocks escape from the working directory
//! but permits local work, so a hosted script can still write its own files.
//! Both speak bash's message vocabulary and differ only in what they deny.
//!
//! Messages are body-only (no invocation-name prefix) so the call site can
//! emit them through `sh_error!` / `sh_error_to!` — the "callers translate"
//! contract, same shape as `shell_state::declare_err_message`.
//!
//! Note what is NOT here: restricting `SHELL`/`PATH`/`HISTFILE`/`ENV`/
//! `BASH_ENV` is not an `Op`. bash marks those variables readonly when
//! restriction engages, so every write path (assignment, `+=`, `export`,
//! `read`, `declare`, `unset`) reports through ordinary readonly machinery
//! with that path's own wording. See `RESTRICTED_READONLY_VARS`.

use std::path::{Component, Path};

/// Variables bash marks readonly when restriction engages.
pub const RESTRICTED_READONLY_VARS: [&str; 5] = ["SHELL", "PATH", "HISTFILE", "ENV", "BASH_ENV"];

/// Which operations are permitted in this shell.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Policy {
    #[default]
    Unrestricted,
    /// bash's restricted shell (`rbash`, `-r`, `set -r`).
    Rbash,
    /// huck's embedding sandbox (`ExecBuilder::restricted()`).
    Sandbox,
}

/// An operation a policy may refuse. Borrowed, so checking allocates nothing
/// on the permitted path.
#[derive(Debug)]
pub enum Op<'a> {
    Cd,
    Exec,
    CommandName(&'a str),
    SourcePath(&'a str),
    /// A redirect whose target resolved to a FILE. Fd-duplication (`>&2`,
    /// `2>&1`) never constructs this, which is why bash permits it.
    RedirectFile { path: &'a str },
    /// `set +r` — asking to leave restricted mode.
    DisableRestricted,
}

impl Policy {
    #[inline]
    pub fn is_restricted(&self) -> bool {
        !matches!(self, Policy::Unrestricted)
    }

    /// Ask whether `op` is permitted. `Err` carries the body-only diagnostic.
    pub fn check(&self, op: Op<'_>) -> Result<(), String> {
        // The unrestricted fast path: one branch, no per-op work.
        let policy = match self {
            Policy::Unrestricted => return Ok(()),
            p => p,
        };
        match op {
            Op::Cd => Err("cd: restricted".to_string()),
            Op::Exec => Err("exec: restricted".to_string()),
            Op::CommandName(name) => {
                if name.contains('/') {
                    Err(format!(
                        "{name}: restricted: cannot specify `/' in command names"
                    ))
                } else {
                    Ok(())
                }
            }
            Op::SourcePath(path) => {
                if path.contains('/') {
                    Err(format!(".: {path}: restricted"))
                } else {
                    Ok(())
                }
            }
            Op::RedirectFile { path } => {
                // The one place the two policies genuinely differ in logic:
                // Rbash refuses every file target, Sandbox only escaping ones.
                let refuse = match policy {
                    Policy::Rbash => true,
                    Policy::Sandbox => escapes_cwd(path),
                    Policy::Unrestricted => unreachable!("handled above"),
                };
                if refuse {
                    Err(format!("{path}: restricted: cannot redirect output"))
                } else {
                    Ok(())
                }
            }
            // The caller routes this through `set`'s invalid-option path;
            // the body is unused but must be non-empty for symmetry.
            Op::DisableRestricted => Err("set: +r: invalid option".to_string()),
        }
    }
}

/// True when `path` could write outside the current directory tree.
fn escapes_cwd(path: &str) -> bool {
    path.starts_with('/')
        || Path::new(path)
            .components()
            .any(|c| matches!(c, Component::ParentDir))
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo fmt --all
cargo test -p huck-engine --jobs 1 --lib policy:: -- --test-threads 1
```

Expected: PASS, 9 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/huck-engine/src/policy.rs crates/huck-engine/src/lib.rs
git commit -m "$(cat <<'EOF'
v319 task 1: Policy/Op abstraction module (#222)

Adds policy.rs: a Policy enum (Unrestricted | Rbash | Sandbox) and an Op
enum of guarded operations, with a full decision matrix test. No call
sites converted yet — restricted.rs stays live until task 3.

Message bodies are bash 5.2.21's, verbatim.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: Swap `Shell.restricted: bool` for `Shell.policy: Policy`

**Files:**
- Modify: `crates/huck-engine/src/shell_state.rs:696` (field), `:1074` (init), `:1427` + `:2145` (drop the assign checks), `:527` (shopt entry stays, read in Task 5)
- Modify: `crates/huck-engine/src/exec_builder.rs:130,144,344,470-480`
- Modify: all 9 sites listed in "Codebase Orientation" to read `shell.policy` instead of `shell.restricted`

**Interfaces:**
- Consumes: `policy::Policy` from Task 1.
- Produces: `Shell.policy: Policy` (public field, replacing `pub restricted: bool`), and `Shell::apply_restricted_readonly(&mut self)` which marks `RESTRICTED_READONLY_VARS` readonly.

This is the mechanical swap plus the readonly-marking switch. Behavior changes here: the four missing variable write paths start being covered, and `HISTFILE` joins the set.

- [ ] **Step 1: Write the failing test**

Add to `crates/huck-engine/src/engine.rs`'s test module (alongside the existing `restricted_*` tests, which stay for now — Task 4 rewords them):

```rust
#[test]
fn restricted_marks_all_five_vars_readonly() {
    let e = Engine::new();
    // Every write path must report through readonly machinery, not a
    // restriction-specific message. HISTFILE is included (bash does).
    for name in ["SHELL", "PATH", "HISTFILE", "ENV", "BASH_ENV"] {
        let out = e
            .prepare(&format!("{name}=/tmp; echo done"))
            .restricted()
            .capture();
        assert!(
            out.stderr.contains(&format!("{name}: readonly variable")),
            "{name}: expected readonly diagnostic, got {:?}",
            out.stderr
        );
    }
}

#[test]
fn restricted_covers_non_assignment_write_paths() {
    let e = Engine::new();
    // These four paths escape the old check_special_assign sites entirely.
    let cases = [
        ("export PATH=/tmp", "PATH: readonly variable"),
        ("PATH+=/tmp", "PATH: readonly variable"),
        ("declare PATH=/tmp", "declare: PATH: readonly variable"),
        ("unset PATH", "unset: PATH: cannot unset: readonly variable"),
    ];
    for (src, want) in cases {
        let out = e.prepare(src).restricted().capture();
        assert!(
            out.stderr.contains(want),
            "{src}: expected {want:?}, got {:?}",
            out.stderr
        );
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p huck-engine --jobs 1 --lib restricted_ -- --test-threads 1
```

Expected: FAIL. `restricted_marks_all_five_vars_readonly` fails on `HISTFILE` (not in the old set) and on the wording (old message is `restricted: PATH: readonly variable`); `restricted_covers_non_assignment_write_paths` fails on all four.

- [ ] **Step 3: Write minimal implementation**

**3a.** In `shell_state.rs`, replace the field at `:696`:

```rust
    /// Which operations this shell may perform. Snapshot-and-restored by
    /// `ExecBuilder`; inherited by functions, subshells, and command
    /// substitutions because `Shell` carries it by value.
    pub policy: crate::policy::Policy,
```

and the initializer at `:1074`:

```rust
            policy: crate::policy::Policy::Unrestricted,
```

**3b.** Add the readonly-marking helper as a `Shell` method (place it next to `mark_readonly` at `:2350`):

```rust
    /// Mark the variables bash makes readonly under restriction. Called once
    /// when a restricted policy engages, NOT per write — every write path then
    /// reports through ordinary readonly machinery with its own wording.
    pub fn apply_restricted_readonly(&mut self) {
        for name in crate::policy::RESTRICTED_READONLY_VARS {
            self.mark_readonly(name);
        }
    }
```

**3c.** Delete the two `check_special_assign` blocks at `shell_state.rs:1427` and `:2145` entirely — readonly-marking replaces them. Each is the `if self.restricted && let Err(msg) = crate::restricted::check_special_assign(...)` conditional and its body.

**3d.** In `exec_builder.rs`, change the `restricted: bool` field (`:130`, `:144`, `:344`) to stay a `bool` at the builder level (the public `.restricted()` API is unchanged), but in `run_restricted_then_inner` (`:470`) set the policy and apply the marking:

```rust
fn run_restricted_then_inner(
    cell: &Rc<RefCell<Shell>>,
    restricted: bool,
    src: &str,
    out: ...,
    err: ...,
) -> ... {
    let prev_policy = cell.borrow().policy;
    if restricted {
        let mut sh = cell.borrow_mut();
        sh.policy = crate::policy::Policy::Sandbox;
        sh.apply_restricted_readonly();
    }
    // ... existing body, with the restore setting `policy = prev_policy`
}
```

Keep the existing RAII restore shape; only the snapshotted value changes from `bool` to `Policy`.

Note: the readonly marks are *not* unwound on restore. This matches the one-way property — a shell that has been restricted does not regain writability — and mirrors bash, where `set -r` marks are permanent for the shell's life.

**3e.** Update the remaining 7 sites to read the policy. Each keeps its current structure; only the condition changes. For example at `builtins.rs:468`:

```rust
    if let Err(msg) = shell.policy.check(crate::policy::Op::Cd) {
        crate::sh_error_to!(shell, err, None, "{msg}");
        return ExecOutcome::Continue(1);
    }
```

The `is_restricted` pre-test is gone — `check` returns `Ok` immediately for `Unrestricted`. Apply the same shape at:
- `builtins.rs:7367` → `Op::SourcePath(path)` (keep the `let Some(path) = args.first()` guard)
- `executor.rs:4619` → `Op::Exec`
- `executor.rs:4661` → `Op::CommandName(&resolved.program)`
- `executor.rs:55` (`check_restricted_redirect`) → see 3f
- `builtins.rs:6327` (`set +r`) → deferred to Task 5, leave as-is but change `crate::restricted::is_restricted(shell)` to `shell.policy.is_restricted()` and `check_set_plus_r()` to an inline `Err("...")` so the module can be deleted

**3f.** Rewrite `check_restricted_redirect` in `executor.rs:48`:

```rust
/// Refuse a file-target output redirect under a restricted policy. Input-only
/// (`ReadOnly`) is never refused, and fd-duplication never reaches here — both
/// match bash, where `<`, `>&2`, and `2>&1` stay permitted under `-r`.
#[inline]
fn check_restricted_redirect(
    mode: &FileMode,
    path: &str,
    shell: &Shell,
    sink: &mut StdoutSink<'_>,
    err_sink: &mut StderrSink<'_>,
) -> Result<(), ()> {
    if !matches!(
        mode,
        FileMode::Truncate | FileMode::Append | FileMode::Clobber | FileMode::ReadWrite
    ) {
        return Ok(());
    }
    if let Err(msg) = shell.policy.check(crate::policy::Op::RedirectFile { path }) {
        let mut err = err_writer(err_sink, sink);
        crate::sh_error_to!(shell, &mut *err, None, "{msg}");
        return Err(());
    }
    Ok(())
}
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cargo fmt --all
cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1
```

Expected: PASS for the two new tests. **Some existing `restricted_*` tests in `engine.rs` will now FAIL** on the changed wording (`restricted: cd` → `cd: restricted`) — that is correct and expected; Task 4 rewords them. Note which ones fail; do not "fix" them by reverting the wording.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
v319 task 2: Shell.restricted -> Shell.policy; readonly-marking (#222)

Swaps the bool for a Copy Policy enum (so fork/subshell/capture paths
inherit it unchanged) and converts 7 of the 9 enforcement sites to
`shell.policy.check(op)`.

Variable restriction becomes readonly-marking via the existing
Shell::mark_readonly, replacing two hand-placed check sites that missed
export/read/declare/unset/+= and omitted HISTFILE entirely.

engine.rs wording assertions fail here by design — task 4 rewords them.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Delete `restricted.rs`

**Files:**
- Delete: `crates/huck-engine/src/restricted.rs`
- Modify: `crates/huck-engine/src/lib.rs` (drop `pub mod restricted;`)

**Interfaces:**
- Consumes: Task 2 having converted every call site.
- Produces: nothing new — this is removal.

- [ ] **Step 1: Verify no references remain**

```bash
grep -rn "restricted::" --include=*.rs crates/ | grep -v "policy::"
```

Expected: no output. If any line prints, convert that site the same way Task 2 did before continuing.

- [ ] **Step 2: Delete the module**

```bash
git rm crates/huck-engine/src/restricted.rs
```

Remove the `pub mod restricted;` line from `crates/huck-engine/src/lib.rs`.

- [ ] **Step 3: Verify the build**

```bash
cargo build -p huck-engine 2>&1 | tail -20
```

Expected: builds clean, no `unresolved module` errors. Watch for newly-unmasked `dead_code` warnings — deleting a module can unmask them (a known trap in this codebase). Fix any that appear.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
v319 task 3: delete restricted.rs, fully replaced by policy.rs (#222)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Reword the existing `Sandbox` tests

**Files:**
- Modify: `crates/huck-engine/src/engine.rs:643-810` (the `RESTRICTED` test section)

**Interfaces:**
- Consumes: `Policy::Sandbox` behavior from Tasks 1-2.
- Produces: a green `engine.rs` test suite.

Task 2 deliberately left these red. Now make them assert bash's wording.

- [ ] **Step 1: Update each assertion**

In `crates/huck-engine/src/engine.rs`, update the `restricted_*` tests to the new message bodies:

| test | old assertion | new assertion |
|---|---|---|
| `restricted_refuses_cd` | `"restricted: cd"` | `"cd: restricted"` |
| `restricted_refuses_exec` | `"restricted: exec"` | `"exec: restricted"` |
| `restricted_refuses_command_name_with_slash` | `"restricted:"` | ``"/bin/echo: restricted: cannot specify `/' in command names"`` |
| `restricted_refuses_source_with_slash` | `"restricted: source"` | `".: /etc/profile: restricted"` |
| `restricted_refuses_absolute_redirect` | `"restricted:"` | `"/tmp/v206-restricted-test: restricted: cannot redirect output"` |
| `restricted_refuses_parent_dir_redirect` | `"restricted:"` | `"../escape: restricted: cannot redirect output"` |
| `restricted_refuses_path_assignment` | `"restricted: PATH"` | `"PATH: readonly variable"` |
| `restricted_refuses_shell_assignment` | `"restricted: SHELL"` | `"SHELL: readonly variable"` |

`restricted_refuses_set_plus_r` is left alone here — Task 5 changes its mechanism. `restricted_off_by_default`, `restricted_accepts_command_name_without_slash`, and `restricted_propagates_to_function` need no change beyond `restricted_propagates_to_function`'s `"restricted: cd"` → `"cd: restricted"`.

- [ ] **Step 2: Add a test pinning Sandbox's relative-write permission**

This is the behavioral difference from `Rbash` and must not silently drift:

```rust
#[test]
fn sandbox_permits_relative_redirect() {
    // Sandbox blocks ESCAPE, not local work — this is the one place it
    // deliberately diverges from bash's rbash, which refuses every file
    // target. See docs/superpowers/specs/2026-07-20-restricted-policy-design.md.
    let dir = std::env::temp_dir().join("huck-v319-sandbox-rel");
    let _ = std::fs::create_dir_all(&dir);
    let e = Engine::new();
    let out = e
        .prepare("echo hi > local_log; cat local_log")
        .cwd(&dir)
        .restricted()
        .capture();
    assert_eq!(out.stdout, "hi\n", "stderr: {:?}", out.stderr);
    let _ = std::fs::remove_dir_all(&dir);
}
```

- [ ] **Step 3: Run the tests**

```bash
cargo fmt --all
cargo test -p huck-engine --jobs 1 --lib restricted -- --test-threads 1
cargo test -p huck-engine --jobs 1 --lib sandbox -- --test-threads 1
```

Expected: PASS, except `restricted_refuses_set_plus_r` which Task 5 addresses.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
v319 task 4: reword Sandbox tests to bash's vocabulary (#222)

Both policies now speak bash's message vocabulary, differing only in what
they deny. Adds a test pinning Sandbox's deliberate divergence: relative
writes stay permitted where rbash refuses every file target.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Entry points — `rbash`, `-r`, `set -r`, and the one-way property

**Files:**
- Modify: `crates/huck-engine/src/shell.rs` (add `startup_restricted`)
- Modify: `crates/huck-cli/src/repl.rs:101` area (call it) and `parse_cli` (accept `-r`)
- Modify: `crates/huck-engine/src/builtins.rs:6327` (`set -r` / `set +r`)
- Modify: `crates/huck-engine/src/shell_state.rs:527` area (`shopt restricted_shell` reads the policy)

**Interfaces:**
- Consumes: `Policy` from Task 1, `Shell.policy` + `apply_restricted_readonly` from Task 2.
- Produces: `huck_engine::shell::startup_restricted(flag: bool, argv0: &str) -> bool`.

- [ ] **Step 1: Write the failing tests**

Add to `crates/huck-engine/src/shell.rs`'s test module:

```rust
#[test]
fn startup_restricted_from_flag_or_argv0() {
    // -r at invocation.
    assert!(startup_restricted(true, "/usr/bin/huck"));
    // argv[0] basename is rbash.
    assert!(startup_restricted(false, "rbash"));
    assert!(startup_restricted(false, "/usr/local/bin/rbash"));
    // Neither.
    assert!(!startup_restricted(false, "huck"));
    assert!(!startup_restricted(false, "/usr/bin/huck"));
    // A name merely CONTAINING rbash is not a match.
    assert!(!startup_restricted(false, "/usr/bin/rbash-wrapper"));
}
```

Add to `crates/huck-engine/src/engine.rs`'s test module:

```rust
#[test]
fn set_dash_r_enters_restricted_mode() {
    let e = Engine::new();
    let out = e.prepare("set -r; cd /tmp").capture();
    assert!(
        out.stderr.contains("cd: restricted"),
        "stderr: {:?}",
        out.stderr
    );
}

#[test]
fn set_plus_r_is_an_invalid_option_while_restricted() {
    // bash makes +r an INVALID OPTION under restriction (usage + rc 1),
    // rather than emitting a restriction-specific refusal.
    let e = Engine::new();
    let out = e.prepare("set -r; set +r; cd /tmp").capture();
    assert!(
        out.stderr.contains("set: +r: invalid option"),
        "stderr: {:?}",
        out.stderr
    );
    // ...and the restriction is still in force afterwards.
    assert!(
        out.stderr.contains("cd: restricted"),
        "restriction leaked off: {:?}",
        out.stderr
    );
}

#[test]
fn set_plus_r_succeeds_in_a_normal_shell() {
    // Verified against bash 5.2.21: rc 0, no diagnostic.
    let e = Engine::new();
    let out = e.prepare("set +r; echo ok").capture();
    assert_eq!(out.stdout, "ok\n", "stderr: {:?}", out.stderr);
    assert!(out.stderr.is_empty(), "stderr: {:?}", out.stderr);
}

#[test]
fn shopt_restricted_shell_is_a_readonly_indicator() {
    let e = Engine::new();
    // Reports on under restriction...
    let out = e.prepare("shopt restricted_shell").restricted().capture();
    assert!(out.stdout.contains("on"), "stdout: {:?}", out.stdout);
    // ...off otherwise...
    let out = e.prepare("shopt restricted_shell").capture();
    assert!(out.stdout.contains("off"), "stdout: {:?}", out.stdout);
    // ...and -s is a silent no-op that does NOT enter restricted mode.
    let out = e.prepare("shopt -s restricted_shell; echo rc=$?; cd /tmp && echo escaped").capture();
    assert!(out.stdout.contains("rc=0"), "stdout: {:?}", out.stdout);
    assert!(out.stdout.contains("escaped"), "shopt -s must not restrict: {:?}", out.stdout);
}

#[test]
fn shopt_minus_u_restricted_shell_cannot_escape() {
    // bash: silent no-op, rc 0, option stays on, restriction still enforced.
    let e = Engine::new();
    let out = e
        .prepare("shopt -u restricted_shell; echo rc=$?; cd /tmp")
        .restricted()
        .capture();
    assert!(out.stdout.contains("rc=0"), "stdout: {:?}", out.stdout);
    assert!(
        out.stderr.contains("cd: restricted"),
        "escaped via shopt -u: {:?}",
        out.stderr
    );
}
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p huck-engine --jobs 1 --lib startup_restricted -- --test-threads 1
cargo test -p huck-engine --jobs 1 --lib set_dash_r -- --test-threads 1
```

Expected: FAIL — `startup_restricted` not defined; `set -r` is an unknown option today.

- [ ] **Step 3: Write the implementation**

**3a.** In `crates/huck-engine/src/shell.rs`, beside `startup_posix`:

```rust
/// Whether the shell starts restricted: the `-r` invocation flag, or an
/// `argv[0]` whose basename is `rbash`. Same flag-or-argv0 shape as
/// `startup_posix`.
pub fn startup_restricted(flag: bool, argv0: &str) -> bool {
    if flag {
        return true;
    }
    std::path::Path::new(argv0)
        .file_name()
        .and_then(|f| f.to_str())
        .is_some_and(|base| base == "rbash")
}
```

**3b.** In `crates/huck-cli/src/repl.rs`, add a `restricted: bool` field to the CLI options struct and parse `-r` in `parse_cli` alongside the other single-letter flags. Then, immediately after the POSIX block at `:101`:

```rust
    // Restricted mode: -r, or invocation as `rbash`. Applied before any
    // program/interactive dispatch so it governs the whole session, and
    // before any user code runs so the readonly marks are already in place.
    {
        let argv0 = std::env::args().next().unwrap_or_default();
        if huck_engine::shell::startup_restricted(opts.restricted, &argv0) {
            let mut sh = shell_cell.borrow_mut();
            sh.policy = huck_engine::policy::Policy::Rbash;
            sh.apply_restricted_readonly();
        }
    }
```

**3c.** In `builtins.rs`, replace the `set +r` block at `:6327` with handling for both `-r` and `+r` in `set`'s option loop.

`-r` (enter restricted mode) — permitted from any policy:

```rust
b'r' if minus => {
    shell.policy = crate::policy::Policy::Rbash;
    shell.apply_restricted_readonly();
}
```

`+r` — succeeds as a no-op when unrestricted, invalid option when restricted:

```rust
b'r' if plus => {
    if shell.policy.is_restricted() {
        // bash routes this through the ordinary invalid-option path:
        // "set: +r: invalid option" plus the usage line, rc 1.
        crate::sh_error_to!(shell, err, None, "set: +r: invalid option");
        write_set_usage(err);
        return ExecOutcome::Continue(1);
    }
    // Unrestricted: bash accepts `set +r` silently at rc 0.
}
```

Match the surrounding loop's existing `minus`/`plus` idiom (see the `b'r' => want_readonly` arms at `:1972-1973` for the established shape in this file). If a usage-line helper does not already exist in `builtin_set_inner`, emit the usage text bash prints:

```
set: usage: set [-abefhkmnptuvxBCEHPT] [-o option-name] [--] [-] [arg ...]
```

Do **not** add a `restricted` long option to the `set -o` table: bash has none, and `set -o restricted` must keep failing with `restricted: invalid option name`, rc 2.

**3d.** Make `shopt restricted_shell` read the policy. In the shopt query path, special-case the name so it reports `shell.policy.is_restricted()` rather than a stored bit, and make `-s`/`-u` silent no-ops returning 0 for it. Leave the table entry at `shell_state.rs:527` in place — it keeps the name in the enumeration output.

- [ ] **Step 4: Run the tests**

```bash
cargo fmt --all
cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1
cargo build -p huck --bin huck
./target/debug/huck -r -c 'cd /tmp'          # expect: cd: restricted
./target/debug/huck -c 'set -r; cd /tmp'     # expect: cd: restricted
./target/debug/huck -c 'set +r; echo ok'     # expect: ok, rc 0
ln -sf huck target/debug/rbash && ./target/debug/rbash -c 'cd /tmp'  # expect: cd: restricted
```

Expected: all PASS; each binary probe matches its comment.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
v319 task 5: rbash/-r/set -r entry points + one-way property (#222)

startup_restricted mirrors startup_posix's flag-or-argv0 shape. `set -r`
enters restricted mode; `set +r` becomes an invalid option while
restricted (bash's actual mechanism: usage + rc 1) but keeps succeeding
at rc 0 in a normal shell.

shopt restricted_shell becomes a read-only indicator — -s and -u are
silent no-ops in both directions, matching bash. The man page is wrong
on this; see the spec's ground-truth section.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: The bash-diff harness

**Files:**
- Create: `tests/scripts/rbash_diff_check.sh`

**Interfaces:**
- Consumes: everything from Tasks 1-5.
- Produces: the durable compat gate.

This is the task that actually proves the work. It is what would have caught the six wrong messages that only surfaced by probing.

- [ ] **Step 1: Write the harness**

Create `tests/scripts/rbash_diff_check.sh`, modelled on the existing harnesses (read `tests/scripts/builtin_write_error_diff_check.sh` first for the established structure — argument handling, `norm`, pass/fail accounting, exit code).

```bash
#!/usr/bin/env bash
# Restricted-shell (rbash) bash-compat harness — issue #222.
#
# Runs each fragment through `bash -r -c` and `huck -r -c` and asserts
# byte-identical stdout, stderr, and exit status.
#
# NOT covered: `>& file`. huck rejects it as `bad fd` independently of
# restricted mode — see issue #223. The other five output-redirect
# operators ARE covered.

set -u
HUCK=${HUCK:-./target/debug/huck}
BASH_BIN=${BASH_BIN:-bash}

norm() { sed -e 's#^bash: line [0-9]*: #SH: #' -e 's#^bash: #SH: #' \
             -e "s#^$(basename "$HUCK"): line [0-9]*: #SH: #" \
             -e "s#^$(basename "$HUCK"): #SH: #"; }

pass=0; fail=0
check() {
  local desc=$1 src=$2
  local bout berr brc hout herr hrc
  bout=$("$BASH_BIN" -r -c "$src" 2>/tmp/rbash_be); brc=$?
  berr=$(norm </tmp/rbash_be)
  hout=$("$HUCK" -r -c "$src" 2>/tmp/rbash_he); hrc=$?
  herr=$(norm </tmp/rbash_he)
  if [[ "$bout" == "$hout" && "$berr" == "$herr" && "$brc" == "$hrc" ]]; then
    pass=$((pass+1))
  else
    fail=$((fail+1))
    printf 'FAIL: %s\n  src:  %s\n' "$desc" "$src"
    printf '  bash: rc=%s out=%q err=%q\n' "$brc" "$bout" "$berr"
    printf '  huck: rc=%s out=%q err=%q\n' "$hrc" "$hout" "$herr"
  fi
}

# --- denied operations -------------------------------------------------
check 'cd'                'cd /etc'
check 'exec'              'exec /bin/true'
check 'command with /'    '/bin/echo hi'
check 'relative cmd with /' './foo'
check 'source with /'     '. /etc/profile'
check 'source builtin'    'source /etc/profile'

# --- redirection: denied file targets ----------------------------------
check 'truncate'          'echo hi > f'
check 'append'            'echo hi >> f'
check 'clobber'           'echo hi >| f'
check 'readwrite'         'echo hi <> f'
check 'amp-gt'            'echo hi &> f'
# `>& f` omitted — issue #223.

# --- redirection: PERMITTED forms (must not regress) -------------------
check 'input redirect'    'read x < /etc/hostname; echo "$x"'
check 'dup to stderr'     'echo hi >&2'
check 'dup stderr to out' 'echo hi 2>&1'
check 'bare command name' 'echo hi'

# --- variable write paths ----------------------------------------------
for v in SHELL PATH HISTFILE ENV BASH_ENV; do
  check "assign $v"       "$v=/tmp"
done
check 'append assign'     'PATH+=/tmp'
check 'export assign'     'export PATH=/tmp'
check 'declare assign'    'declare PATH=/tmp'
check 'unset'             'unset PATH'
check 'read into PATH'    'echo x | read PATH'
check 'prefix assign'     'PATH=/tmp true'
check 'unrelated var ok'  'FOO=/tmp; echo "$FOO"'

# --- the one-way property ----------------------------------------------
check 'set +r'            'set +r; cd /etc'
check 'set -o restricted' 'set -o restricted'
check 'set +o restricted' 'set +o restricted'
check 'shopt query'       'shopt restricted_shell'
check 'shopt -s'          'shopt -s restricted_shell; echo rc=$?'
check 'shopt -u'          'shopt -u restricted_shell; echo rc=$?; cd /etc'

# --- propagation --------------------------------------------------------
check 'in function'       'f() { cd /etc; }; f'
check 'in subshell'       '( cd /etc )'
check 'in cmdsub'         'echo "$(cd /etc)"'

rm -f /tmp/rbash_be /tmp/rbash_he
printf 'rbash_diff_check: %d passed, %d failed\n' "$pass" "$fail"
[[ $fail -eq 0 ]]
```

Make it executable: `chmod +x tests/scripts/rbash_diff_check.sh`.

- [ ] **Step 2: Run it in a scratch directory**

The harness writes files (`f`) on the permitted-path cases, so run it somewhere disposable:

```bash
cargo build -p huck --bin huck
mkdir -p /tmp/rbash-scratch && cd /tmp/rbash-scratch
HUCK=/home/john/projects/huck/target/debug/huck \
  ulimit -v 1500000; timeout 120 /home/john/projects/huck/tests/scripts/rbash_diff_check.sh
```

Expected: `rbash_diff_check: N passed, 0 failed`.

- [ ] **Step 3: Fix any divergence**

Each failure is a real bash-fidelity gap. Fix the engine, not the harness — the only legitimate harness edit is adding a documented exclusion with an issue number, as `>& file` has. If a divergence turns out to be genuinely out of scope, file a `divergence` issue and note it in the harness comment block.

- [ ] **Step 4: Commit**

```bash
cd /home/john/projects/huck
git add tests/scripts/rbash_diff_check.sh
git commit -m "$(cat <<'EOF'
v319 task 6: rbash bash-diff harness (#222)

Asserts byte-identical stdout/stderr/rc against bash 5.2.21 for every
guarded op, all five testable output-redirect operators, the permitted
forms that must not regress (<, >&2, 2>&1, bare names), all six variable
write paths, the one-way property, and propagation into functions,
subshells, and command substitutions.

Excludes `>& file` — huck rejects it as `bad fd` independently of
restricted mode (#223).

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: Documentation and iteration record

**Files:**
- Modify: `docs/architecture.md:49-52`
- Modify: `crates/huck-engine/src/exec_builder.rs:180-188` (the `restricted()` doc comment)
- Modify: `/home/john/.claude/projects/-home-john-projects-huck/memory/project_huck_iterations.md` and `MEMORY.md`

**Interfaces:**
- Consumes: the finished implementation.
- Produces: accurate docs.

- [ ] **Step 1: Update `docs/architecture.md`**

Lines 49-52 currently describe `.restricted()` as an "rbash-subset policy ... via `restricted.rs`". Replace with a description of the policy model: `Policy::{Unrestricted, Rbash, Sandbox}` in `policy.rs`, one `shell.policy.check(op)` call form, `Rbash` bash-exact vs `Sandbox` escape-blocking, and variable restriction via readonly-marking. Mention the `rbash`/`-r`/`set -r` entry points.

Also check the "where to add common features" cheatsheet — if it has an entry for restricted-mode checks, point it at `policy.rs` and describe adding an `Op` variant.

- [ ] **Step 2: Update the `ExecBuilder::restricted()` doc comment**

It currently enumerates the old rules and names `restricted.rs`. Rewrite it to say the builder selects `Policy::Sandbox`, state what Sandbox denies (including that relative writes stay permitted — the deliberate divergence from rbash), and point at the spec.

- [ ] **Step 3: Sweep for stale references**

```bash
grep -rn "restricted\.rs\|check_special_assign\|is_restricted(shell)" docs/ crates/ README.md
```

Expected: no output. This codebase has a repeated history of comment-rot surviving task-scoped review — a doc comment asserting something the branch just changed. Fix anything this finds.

Leave `docs/bash-test-suite-baseline.md:214` alone: the `rsh` category is v320's, and the line stays accurate until then.

- [ ] **Step 4: Record the iteration in memory**

Add a v319 entry to `project_huck_iterations.md` (newest at top) and a one-line hook to `MEMORY.md`. Cover: the `Policy`/`Op` abstraction; that the variable restriction turned out to be readonly-marking rather than a check (and that this covered four write paths the old code missed); that `man bash` was wrong on three counts, caught only by probing the real shell; and the `>& file` follow-on (#223).

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
v319 task 7: docs + iteration record (#222)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: Full verification and PR

**Files:** none — this is the gate.

- [ ] **Step 1: Build both binaries**

```bash
cargo build --locked --bin huck
cargo build --release --locked --bin huck
```

- [ ] **Step 2: Run the full diff sweep**

```bash
ulimit -v 1500000
timeout 900 tests/scripts/run_diff_checks.sh 2>&1 | tail -30
```

Expected: green. Investigate any failure — do not assume it is a pre-existing flake without checking it against `main`.

- [ ] **Step 3: Run the per-crate test suites**

```bash
cargo test -p huck-syntax --jobs 1 --lib -- --test-threads 1
cargo test -p huck-engine --jobs 1 --lib -- --test-threads 1
```

- [ ] **Step 4: Run the `-p huck` integration binaries**

These run locally and have caught CI-only failures before (v289, v313) — a change to `Shell`'s fields is exactly the shape that bites here. Run each one:

```bash
for t in $(ls tests/*.rs | xargs -n1 basename | sed 's/\.rs$//'); do
  echo "=== $t ==="
  ulimit -v 1500000
  timeout 300 cargo test -p huck --test "$t" --jobs 1 -- --test-threads 1 2>&1 | tail -5
done
```

Expected: all pass.

- [ ] **Step 5: Confirm formatting**

```bash
cargo fmt --all --check
```

Expected: no output.

- [ ] **Step 6: Push and open the PR**

```bash
git push -u origin v319-restricted-policy
gh pr create --title "v319: restricted-shell policy abstraction + rbash fidelity" --body "$(cat <<'EOF'
Closes #222

Replaces the ~9 hand-placed `is_restricted() && check_X()` sites with a
`Policy`/`Op` enum pair on `Shell`, corrects restricted-mode semantics to
match bash 5.2.21, and adds the missing `rbash` / `-r` / `set -r` entry
points.

## What changed

- **New `policy.rs`** — `Policy` (`Unrestricted` | `Rbash` | `Sandbox`) and
  `Op`; every site is now `shell.policy.check(op)?`, with `Unrestricted`
  returning `Ok(())` from the first arm. Adding a restriction is
  compiler-checked: a new `Op` variant fails to build until every policy
  handles it. `restricted.rs` is deleted.
- **Variable restriction is readonly-marking, not a check.** bash marks
  SHELL/PATH/HISTFILE/ENV/BASH_ENV readonly; every write path then reports
  through ordinary readonly machinery. This replaces two hand-placed sites
  that missed `export`, `read`, `declare`, `unset`, and `+=`, and omitted
  HISTFILE entirely — less code, more coverage.
- **Redirect semantics corrected** — bash denies every *file-target* output
  redirect, not just escaping ones. Fd-duplication (`>&2`, `2>&1`) and input
  (`<`) stay permitted, which falls out of only constructing
  `Op::RedirectFile` for resolved file targets.
- **All six message shapes corrected** to bash's wording.
- **Entry points added** — `rbash` as argv[0], `-r`, `set -r`, via a
  `startup_restricted` helper mirroring the existing `startup_posix` shape.
- **`shopt restricted_shell` is a read-only indicator**; `set +r` becomes an
  invalid option while restricted (bash's actual mechanism) but keeps
  succeeding at rc 0 in a normal shell.

`Sandbox` preserves the existing `ExecBuilder::restricted()` behavior
exactly — escape-blocking, relative writes permitted — now under bash's
message vocabulary.

## Note on the man page

`man bash` is wrong on three counts, caught by probing 5.2.21 directly:
`shopt -u restricted_shell` is a silent no-op rather than disallowed, there
is no `restricted` long option, and `set +r` succeeds in a normal shell. The
spec records the measured behavior.

## Verification

`tests/scripts/rbash_diff_check.sh` asserts byte-identical stdout/stderr/rc
against bash for every op, all five testable redirect operators, the
permitted forms that must not regress, all six variable write paths, the
one-way property, and propagation into functions/subshells/cmdsubs. Plus a
policy matrix unit test and the full diff sweep.

## Follow-ons

- #223 — `>& file` rejected as `bad fd` (found while scoping the harness;
  independent of restricted mode, so excluded from the harness)
- v320 — the remaining restrictions: `history`/`hash -p` with `/`,
  `enable -f`/`-d`, `command -p`, `BASH_FUNC_` import, the script-spawn
  exemption, and the `rsh` bash-suite category

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 7: Wait for CI**

Poll the run until it **finishes**. Local green is not CI green — this box is 1-core (races cannot fire), CI is 4-core.

```bash
gh pr checks --watch
```

Only once CI has passed, hand the PR to the user to review and merge. **Do not merge it yourself.**

---

## Self-Review Notes

**Spec coverage.** Every spec section maps to a task: the abstraction → Task 1; `Op::Assign`'s rejection in favor of readonly-marking → Task 2; entry points, the one-way property, and the `shopt` indicator → Task 5; `Sandbox` → Tasks 1 and 4; verification's three layers → Tasks 1 (matrix), 4 (engine tests), 6 (harness); documentation → Task 7. The spec's deferred items stay deferred and are named in the PR body.

**One spec assumption corrected during planning.** The spec warned that redirect machinery is duplicated across fg/bg/subshell/capture paths and required an audit of all of them. The audit found enforcement already funnels through `lower_one_redirect` (two arms), with the child path reaching it via `build_child_redir_plan` → `lower_redirects`. Task 2 fixes both arms; there is no duplication to chase. This is recorded in "Codebase Orientation" so the implementer does not re-derive it.

**One scope reduction.** `>& file` is in the spec's harness coverage list but huck does not implement it at all (issue #223, filed during planning). Task 6 excludes it with a pointer; the other five operators are covered.
