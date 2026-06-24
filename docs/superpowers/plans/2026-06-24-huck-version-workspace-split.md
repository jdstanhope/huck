# v213 HUCK_VERSION across workspace split — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fix `$HUCK_VERSION` to report the root `huck` crate's `CARGO_PKG_VERSION` (the user-facing release version) instead of `huck-engine`'s pinned `0.1.0`. Add `huck --version` / `-V` flag. Restore the strict equality assertion the v212 test fixup loosened.

**Architecture:** `src/main.rs` (root huck crate) passes `env!("CARGO_PKG_VERSION")` to a new `huck_cli::run(args, version: &str)` signature. The CLI threads the version into the `Shell` via `shell.set("HUCK_VERSION", v)` at startup, OVERRIDING the engine's default. A new `RunMode::PrintVersion` variant + `--version`/`-V` arms in `parse_cli` route the print-and-exit path. `EngineBuilder::with_version(&str)` mirrors the same plumbing for external embedders.

**Tech Stack:** Rust 2024, no new deps.

**Branch:** `v213-huck-version-workspace-split`. Each task ends with a green-suite commit.

**Spec:** `docs/superpowers/specs/2026-06-24-huck-version-workspace-split-design.md`.

## Global Constraints

- Commit trailer (every commit): `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>` exact, last line of every commit.
- No source comments referencing `v213` / task numbers / iteration version.
- Workspace test command: `cargo test --workspace`.
- The huck-engine fallback at `shell_state.rs:1981` (`env!("CARGO_PKG_VERSION")` resolved to huck-engine's pinned `0.1.0`) MUST stay — it's the documented fallback for embedders that don't call `.with_version()` and for huck-engine's own internal tests.
- `huck_cli::run` signature change is a public-API breaking change, but the only known consumer is `src/main.rs` (one-line ripple). Other consumers do not exist.

**Key context — verified APIs:**

- `crates/huck-engine/src/engine.rs:90+`:
  - `pub fn run(&mut self, src: &str) -> i32`
  - `pub fn capture(&mut self, src: &str) -> Output` (line 116)
  - `pub fn exec(&mut self, src: &str) -> ExecBuilder<'_>` (line 124)
  - `pub fn set_var(&mut self, name: &str, value: &str)` (line 170)
  - `Output { stdout: String, stderr: String, exit_code: i32 }`
- `crates/huck-engine/src/engine.rs:237`:
  - `pub struct EngineBuilder { arg0: Option<String>, args: Vec<String>, env: Vec<(String, String)> }`
  - Existing methods: `.env(k, v)`, `.arg0(name)`, `.args(args)`, `.build() -> Engine`
- `crates/huck-engine/src/shell.rs:17`:
  - `pub enum RunMode { Interactive, Command { command, argv0, args }, File { path, args } }`
- `crates/huck-engine/src/shell.rs:47`:
  - `pub fn parse_cli(args: &[String]) -> Result<CliOptions, String>`
  - Handles `--norc`, `-n`, `--rcfile`, `-c`, `--`, rejects other `-`-prefixed args.
- `crates/huck-engine/src/shell_state.rs:957`:
  - `pub fn set(&mut self, name: &str, value: String)` (the scalar var setter the CLI will call).
- `crates/huck-engine/src/shell_state.rs:1981`:
  - `self.install_var("HUCK_VERSION", env!("CARGO_PKG_VERSION").to_string(), false);` — the FALLBACK.
- `crates/huck-cli/src/repl.rs:43`:
  - `pub fn run(args: &[String]) -> i32`. Re-exported as `huck_cli::run` via `crates/huck-cli/src/lib.rs`.
- `src/main.rs`:
  - `let args: Vec<String> = std::env::args().skip(1).collect(); std::process::exit(huck_cli::run(&args));`
- `tests/builtin_vars.rs:58-63` (post-v212 loosening):
  - `bash_version_and_huck_version` — currently asserts non-empty digit-led; needs strict assertion restored.

---

## File structure

**Modify:**
- `crates/huck-engine/src/shell.rs` — add `RunMode::PrintVersion` variant; add `--version`/`-V` arms to `parse_cli`; add 2 unit tests in `mod tests`.
- `crates/huck-engine/src/engine.rs` — add `version: Option<String>` field to `EngineBuilder`; add `.with_version(&str)` method; `.build()` applies via `set_var`; add 1 unit test in `mod tests`.
- `crates/huck-cli/src/repl.rs` — change `run` signature to `(args, version)`; handle `RunMode::PrintVersion`; set `HUCK_VERSION` on `shell_cell` at startup.
- `src/main.rs` — pass `env!("CARGO_PKG_VERSION")` to `huck_cli::run`.
- `tests/builtin_vars.rs` — restore strict assertion + remove v212 loosening comment.

No new files. No new crates. No public API addition beyond the documented `EngineBuilder::with_version` + `huck_cli::run` signature change.

---

## Task 1: `RunMode::PrintVersion` + `--version`/`-V` arms in `parse_cli` + tests

**Files:**
- Modify: `crates/huck-engine/src/shell.rs:17` — add variant.
- Modify: `crates/huck-engine/src/shell.rs:47-114` — add 2 match arms in the option scan.
- Modify: `crates/huck-engine/src/shell.rs::mod tests` — add 2 tests.

**Interfaces:**
- Produces:
  - `RunMode::PrintVersion` variant (no payload).
  - `parse_cli(&["--version".into()])` and `parse_cli(&["-V".into()])` both return `Ok(CliOptions { mode: RunMode::PrintVersion, .. })`.

- [ ] **Step 1: Create the branch**

```bash
git checkout main
git pull --ff-only
git checkout -b v213-huck-version-workspace-split
```

- [ ] **Step 2: Add `RunMode::PrintVersion` variant**

Find the enum at line 17:

```bash
grep -n "pub enum RunMode" crates/huck-engine/src/shell.rs
```

Add the new variant as the last entry:

```rust
pub enum RunMode {
    /// REPL (tty) or piped-stdin command reading — current behavior.
    Interactive,
    /// `-c COMMAND [NAME [ARG...]]`: argv0 = NAME (None → keep the shell's
    /// default $0), args = the rest.
    Command { command: String, argv0: Option<String>, args: Vec<String> },
    /// `SCRIPT [ARG...]`: $0 = path, args = the rest.
    File { path: PathBuf, args: Vec<String> },
    /// `--version` / `-V`: print "huck {version}" and exit 0.
    PrintVersion,
}
```

Match `pub` visibility and surrounding doc-comment style with the existing variants.

- [ ] **Step 3: Add `--version` / `-V` arms in `parse_cli`**

In `parse_cli` (line 47), inside the `while i < args.len()` loop, BEFORE the catchall `s if s.starts_with('-') && s.len() > 1 =>`, add:

```rust
            "--version" | "-V" => {
                return Ok(CliOptions {
                    rcfile_path: None,
                    norc: false,
                    noexec: false,
                    mode: RunMode::PrintVersion,
                });
            }
```

The early `return Ok(...)` means `--version`/`-V` short-circuits the rest of the scan — operands after the flag (e.g. `huck --version somefile`) are ignored.

- [ ] **Step 4: Add unit tests**

Find the existing `#[cfg(test)] mod tests` block (search for `mod tests` near the bottom of the file). Append at the end of the block:

```rust
    #[test]
    fn parse_cli_version_long() {
        let opts = parse_cli(&["--version".to_string()]).expect("parse");
        assert!(matches!(opts.mode, RunMode::PrintVersion));
    }

    #[test]
    fn parse_cli_version_short() {
        let opts = parse_cli(&["-V".to_string()]).expect("parse");
        assert!(matches!(opts.mode, RunMode::PrintVersion));
    }
```

- [ ] **Step 5: Build + run tests**

```bash
cargo build --workspace -q
cargo test -p huck-engine --lib parse_cli_version 2>&1 | tail -10
cargo test --workspace --quiet
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: 2 new tests pass; full suite green; clippy clean. The matches!() pattern in repl.rs's `match opts.mode { RunMode::Command {...} | RunMode::File {...} | RunMode::Interactive => ... }` will now be non-exhaustive because `RunMode::PrintVersion` is unhandled — Task 3 wires it. Until Task 3, add a `RunMode::PrintVersion => return 0` placeholder at the END of the match in `repl::run` (line ~68) to keep the build green between commits.

Find the existing match:

```bash
grep -n "match opts.mode" crates/huck-cli/src/repl.rs
```

Add inside the match BEFORE the closing brace:

```rust
        RunMode::PrintVersion => return 0, // wired in Task 3
```

This is a one-line scaffolding edit; Task 3 replaces it with the real `println!` + exit.

- [ ] **Step 6: Commit**

```bash
git add crates/huck-engine/src/shell.rs crates/huck-cli/src/repl.rs
git commit -m "$(cat <<'EOF'
v213 task 1: RunMode::PrintVersion + --version/-V in parse_cli

New variant for the print-and-exit flag path. parse_cli returns it
when --version or -V appears in the leading option scan (operands
after the flag are ignored — short-circuit return). Two unit tests
pin the parse result. repl.rs gets a temporary RunMode::PrintVersion
=> return 0 arm so the match stays exhaustive between commits; Task
3 replaces it with the real println!.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: `EngineBuilder::with_version` + unit test

**Files:**
- Modify: `crates/huck-engine/src/engine.rs:237-271` — add field + method; apply in `build()`.
- Modify: `crates/huck-engine/src/engine.rs::mod tests` — add 1 unit test.

**Interfaces:**
- Produces:
  - `EngineBuilder::with_version(self, v: &str) -> Self` — stores `Some(v.to_string())` in a new `version: Option<String>` field.
  - `EngineBuilder::build()` — if `version.is_some()`, calls `engine.set_var("HUCK_VERSION", &v)` AFTER `Engine::new()` returns. Order: existing `set_arg0` / `set_args` / `env` first, then `set_var("HUCK_VERSION", ...)` if set. Order between version and env doesn't matter; choose any.

- [ ] **Step 1: Add `version` field + `with_version` method**

Find the struct at line 237:

```bash
grep -n "pub struct EngineBuilder" crates/huck-engine/src/engine.rs
```

Replace the struct + impl with the extended version:

```rust
#[derive(Default)]
pub struct EngineBuilder {
    arg0: Option<String>,
    args: Vec<String>,
    env: Vec<(String, String)>,
    version: Option<String>,
}

impl EngineBuilder {
    /// Seed a shell variable.
    pub fn env(mut self, key: &str, value: &str) -> Self {
        self.env.push((key.to_string(), value.to_string()));
        self
    }
    /// Set `$0`.
    pub fn arg0(mut self, name: &str) -> Self {
        self.arg0 = Some(name.to_string());
        self
    }
    /// Set the positional parameters.
    pub fn args(mut self, args: Vec<String>) -> Self {
        self.args = args;
        self
    }
    /// Override `$HUCK_VERSION` with the embedder's release-version
    /// string. When unset, the engine's default (its own crate version)
    /// is used.
    pub fn with_version(mut self, version: &str) -> Self {
        self.version = Some(version.to_string());
        self
    }
    /// Build the engine.
    pub fn build(self) -> Engine {
        let mut e = Engine::new();
        if let Some(a0) = self.arg0 {
            e.set_arg0(&a0);
        }
        e.set_args(self.args);
        for (k, v) in self.env {
            e.set_var(&k, &v);
        }
        if let Some(v) = self.version {
            e.set_var("HUCK_VERSION", &v);
        }
        e
    }
}
```

- [ ] **Step 2: Add unit test**

Find the existing `#[cfg(test)] mod tests` block in `engine.rs`:

```bash
grep -n "#\[cfg(test)\]" crates/huck-engine/src/engine.rs | head -3
```

Append (adjust placement to match the block's style — `use super::*;` is likely already in scope):

```rust
    #[test]
    fn builder_with_version_sets_huck_version() {
        let mut e = Engine::builder().with_version("9.9.9").build();
        let out = e.capture("echo $HUCK_VERSION");
        assert_eq!(out.stdout.trim(), "9.9.9");
        assert_eq!(out.exit_code, 0);
    }
```

- [ ] **Step 3: Build + run tests**

```bash
cargo build --workspace -q
cargo test -p huck-engine --lib builder_with_version_sets_huck_version 2>&1 | tail -5
cargo test --workspace --quiet
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: 1 new test passes; full suite green; clippy clean.

- [ ] **Step 4: Commit**

```bash
git add crates/huck-engine/src/engine.rs
git commit -m "$(cat <<'EOF'
v213 task 2: EngineBuilder::with_version for embedder version override

Adds version: Option<String> field + with_version(&str) builder method.
build() applies via set_var("HUCK_VERSION", v) after Engine::new()
constructs the default (env!("CARGO_PKG_VERSION") of huck-engine).
One unit test pins the override end-to-end via Engine::capture.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: `huck_cli::run(args, version)` + `RunMode::PrintVersion` handling + HUCK_VERSION override

**Files:**
- Modify: `crates/huck-cli/src/repl.rs:43` — change `run` signature; handle `RunMode::PrintVersion`; set `HUCK_VERSION` on `shell_cell`.

**Interfaces:**
- Consumes (from Task 1): `RunMode::PrintVersion` variant.
- Produces: `huck_cli::run(args: &[String], version: &str) -> i32`. Breaking signature change; `src/main.rs` rippled in Task 4.

- [ ] **Step 1: Update `run` signature**

Find:

```bash
grep -n "pub fn run" crates/huck-cli/src/repl.rs
```

Replace the signature:

```rust
pub fn run(args: &[String], version: &str) -> i32 {
```

(The function is re-exported via `crates/huck-cli/src/lib.rs:5` `pub use repl::run;` — the re-export picks up the new signature automatically.)

- [ ] **Step 2: Handle `RunMode::PrintVersion` and set `HUCK_VERSION` on shell_cell**

After the `parse_cli` result handling and BEFORE the `install_job_control_signals()` call (or anywhere before the shell_cell mutation begins), short-circuit on `PrintVersion`:

```rust
    let opts = match parse_cli(args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("huck: {e}");
            return 2;
        }
    };

    // Short-circuit: --version / -V.
    if matches!(opts.mode, RunMode::PrintVersion) {
        println!("huck {version}");
        return 0;
    }

    install_job_control_signals();

    let shell_cell = Rc::new(RefCell::new(Shell::new()));
    {
        // Override $HUCK_VERSION with the embedder-provided release
        // version. shell.set is a scalar setter that bypasses the
        // engine's default install_var.
        let mut shell = shell_cell.borrow_mut();
        shell.set("HUCK_VERSION", version.to_string());
    }
    /* … the rest of run() continues unchanged from this point … */
```

Now REMOVE the temporary `RunMode::PrintVersion => return 0,` arm added in Task 1 from the `match opts.mode` block (because we already short-circuited above, the match no longer needs the variant). Wait — the match will be non-exhaustive if we remove the arm and don't short-circuit. Two safe paths:

**Path A (recommended)**: Keep the short-circuit at the top of `run` AND keep the `RunMode::PrintVersion => unreachable!("short-circuited above")` arm OR `RunMode::PrintVersion => return 0,` (defensive). Pick `return 0` — costs nothing and survives accidental refactor.

**Path B**: Remove the short-circuit and handle PrintVersion inside the match. But the `RunMode::Command` / `RunMode::File` arms run engine logic AFTER `shell_cell` setup, while PrintVersion shouldn't need a shell_cell at all. Path A keeps it simple.

Keep Path A: top-of-`run` short-circuit + leave the `RunMode::PrintVersion => return 0,` defensive arm in the match.

- [ ] **Step 3: Build (signature ripple expected)**

```bash
cargo build --workspace -q 2>&1 | head -20
```

Expected: ONE error from `src/main.rs` (the root crate calls `huck_cli::run(&args)` with no version). Task 4 fixes it.

Acceptable to commit Task 3 with a broken root-crate build because the next task's first step fixes it; OR fix `src/main.rs` inline here and merge with Task 4. Decision: leave `src/main.rs` broken at end of Task 3, fix in Task 4. The branch's incremental commits being momentarily broken is acceptable since this is subagent-driven development on a feature branch — main is never broken.

(If the implementer prefers to land Task 3 + Task 4 as one commit because the build is broken between them, that's fine too — combine them. The plan splits because the changes are conceptually distinct.)

- [ ] **Step 4: Build + run huck-cli tests**

```bash
cargo build -p huck-cli -q
cargo test -p huck-cli --lib --quiet
cargo clippy -p huck-cli --all-targets -- -D warnings
```

Expected: huck-cli alone builds + tests green + clippy clean. The root `huck` crate is broken; that's expected and Task 4 fixes it.

- [ ] **Step 5: Commit**

```bash
git add crates/huck-cli/src/repl.rs
git commit -m "$(cat <<'EOF'
v213 task 3: huck_cli::run takes version; handles PrintVersion

Signature: pub fn run(args: &[String], version: &str) -> i32. The
embedder-provided version is short-circuited via println! + return 0
when RunMode::PrintVersion is selected; otherwise it's written to
$HUCK_VERSION on the shell_cell at startup (overriding the engine's
default 0.1.0 fallback).

Root huck crate's src/main.rs ripple lands in Task 4 — the workspace
build is intentionally broken between this commit and the next.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: `src/main.rs` ripple + workspace build green

**Files:**
- Modify: `src/main.rs` — pass `env!("CARGO_PKG_VERSION")` to `huck_cli::run`.

**Interfaces:**
- Consumes (from Task 3): `huck_cli::run(args: &[String], version: &str) -> i32`.

- [ ] **Step 1: Update the call site**

Replace the contents of `src/main.rs`:

```rust
//! huck — thin binary shim. All logic lives in `huck-cli` (REPL) over
//! `huck-engine` (execution) over `huck-syntax` (frontend).
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(huck_cli::run(&args, env!("CARGO_PKG_VERSION")));
}
```

`env!("CARGO_PKG_VERSION")` resolves at compile time to the root `huck` crate's `Cargo.toml` `version` field — currently `"0.3.0-dev"`, bumped per `docs/RELEASING.md` on each release.

- [ ] **Step 2: Build + test**

```bash
cargo build --workspace -q
cargo test --workspace --quiet
cargo clippy --workspace --all-targets -- -D warnings
cargo build --release --workspace --quiet
```

Expected: green; clippy clean; release builds.

- [ ] **Step 3: Smoke test the end-to-end fix**

```bash
./target/release/huck -c 'echo $HUCK_VERSION'
# expect: 0.3.0-dev
./target/release/huck --version
# expect: huck 0.3.0-dev
./target/release/huck -V
# expect: huck 0.3.0-dev
echo "exit=$?"
# expect: exit=0
```

If `huck --version` still prints `unrecognized option`, the wiring is incomplete — re-check Tasks 1+3. If `$HUCK_VERSION` prints `0.1.0`, the override in Task 3 step 2 didn't fire.

- [ ] **Step 4: Commit**

```bash
git add src/main.rs
git commit -m "$(cat <<'EOF'
v213 task 4: src/main.rs ripple — pass root CARGO_PKG_VERSION to huck_cli::run

One-line change: huck_cli::run(&args) -> huck_cli::run(&args, env!("CARGO_PKG_VERSION")).
env!() resolves at compile time to the ROOT huck crate's Cargo.toml
version (today: "0.3.0-dev"; bumped per docs/RELEASING.md on each
release). Restores the workspace build that Task 3 intentionally left
broken.

End-to-end smoke verified: $HUCK_VERSION and `huck --version` both
report the root crate's version.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: Restore strict assertion + final sweep

**Files:**
- Modify: `tests/builtin_vars.rs:58-67` — restore strict assertion.

**Interfaces:**
- Consumes (from Tasks 1-4): the working end-to-end fix. `huck -c 'echo $HUCK_VERSION'` now reports the root crate's `CARGO_PKG_VERSION`, which IS the test crate's `CARGO_PKG_VERSION` (tests in `tests/` live under the root `huck` crate).

- [ ] **Step 1: Restore the strict assertion**

Find:

```bash
grep -n "bash_version_and_huck_version" tests/builtin_vars.rs
```

Replace the body (the post-v212 loosening + comment) with the strict v211-and-earlier shape:

```rust
#[test]
fn bash_version_and_huck_version() {
    assert_eq!(huck("[ -n \"$BASH_VERSION\" ] && echo yes").trim(), "yes");
    assert_eq!(huck("echo ${BASH_VERSINFO[0]}").trim(), "5");
    assert_eq!(huck("echo $HUCK_VERSION").trim(), env!("CARGO_PKG_VERSION"));
}
```

Specifically, REMOVE these post-v212 lines (the loosening + 4-line comment):

```rust
    // HUCK_VERSION currently reports huck-engine's crate version, not the
    // root huck package version. Diverges after the v202 workspace split;
    // tracked as a follow-on. Pin against the engine value the shell really
    // produces, not env!("CARGO_PKG_VERSION") of this test crate.
    let v = huck("echo $HUCK_VERSION").trim().to_string();
    assert!(!v.is_empty(), "$HUCK_VERSION should be non-empty");
    assert!(v.chars().next().unwrap().is_ascii_digit(), "$HUCK_VERSION = {v:?}");
```

And put the original strict assertion back in place.

- [ ] **Step 2: Run the restored test**

```bash
cargo test --test builtin_vars bash_version_and_huck_version 2>&1 | tail -10
```

Expected: PASS.

If FAIL with "0.1.0 != 0.3.0-dev", Tasks 1-4 didn't wire the override correctly — `shell.set("HUCK_VERSION", version)` in `repl::run` isn't running before the test invokes the binary. Re-check Task 3 step 2.

- [ ] **Step 3: Full final sweep**

```bash
cargo test --workspace --quiet
cargo test --workspace --doc --quiet
cargo clippy --workspace --all-targets -- -D warnings
cargo build --release --workspace --quiet

# All existing harnesses:
for h in tests/scripts/*_diff_check.sh; do
    bash "$h" > /tmp/h.out 2>&1
    rc=$?
    if [ $rc -ne 0 ]; then
        echo "FAIL: $h (exit $rc)"
        tail -10 /tmp/h.out
    fi
done

# Headless CLI smoke:
./target/release/huck -c 'echo hello'
echo "exit=$?"

# End-to-end version smoke:
./target/release/huck -c 'echo $HUCK_VERSION'
./target/release/huck --version
./target/release/huck -V
```

Expected: all green; release build clean; all 131 harnesses pass; smoke prints `hello` + `exit=0`; version smokes all print the root crate's CARGO_PKG_VERSION.

- [ ] **Step 4: Commit**

```bash
git add tests/builtin_vars.rs
git commit -m "$(cat <<'EOF'
v213 task 5: restore strict $HUCK_VERSION assertion

Reverts the v212 (commit 2f86197) loosening. With Tasks 1-4 wiring
the root-crate CARGO_PKG_VERSION through huck_cli::run, the test
crate's env!("CARGO_PKG_VERSION") matches what $HUCK_VERSION
reports (both ARE the root huck crate's Cargo.toml version), so the
strict equality assertion holds. Removes the explanatory comment
about the workspace-split divergence.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 5: Stop — do NOT merge**

The final whole-branch code review is the controller's call. Stop after this commit.

---

## Self-review

**Spec coverage:**
- `RunMode::PrintVersion` + `--version`/`-V` parse arms: Task 1.
- `EngineBuilder::with_version`: Task 2.
- `huck_cli::run(args, version)` signature change: Task 3.
- `src/main.rs` ripple: Task 4.
- `RunMode::PrintVersion` handling (println + exit 0): Task 3 step 2.
- `shell.set("HUCK_VERSION", v)` override in CLI startup: Task 3 step 2.
- Strict assertion restored: Task 5.
- 4 unit tests: parse_cli_version_long (Task 1), parse_cli_version_short (Task 1), builder_with_version_sets_huck_version (Task 2). The optional `version_round_trips_through_run` test from the spec is DROPPED — capturing stdout from a function that calls `println!` is awkward in Rust without redirecting fds; the integration test in tests/builtin_vars.rs covers the end-to-end shape via the smoke harness binary.

**Placeholder scan:**
- No "TBD" / "implement later" / "fill in details" anywhere.
- Task 3's "Path A vs Path B" is a documented design decision with a clear recommendation, not a placeholder.
- Task 3's "build intentionally broken between commits" is a real conditional, not vagueness — the implementer is told what to do in either case (commit broken state OR combine tasks).
- Task 1 step 5's `RunMode::PrintVersion => return 0` scaffolding line is concrete code, removed/upgraded in Task 3.

**Type consistency:**
- `RunMode::PrintVersion` (variant name): same in Tasks 1, 3.
- `EngineBuilder::with_version(self, &str) -> Self`: same in Tasks 2, plan global constraints.
- `huck_cli::run(args: &[String], version: &str) -> i32`: same in Tasks 3, 4.
- `shell.set(name: &str, value: String)`: matches the verified `shell_state.rs:957` signature.
- `e.set_var(name: &str, value: &str)`: matches the verified `engine.rs:170` signature.
- `Engine::capture(src: &str) -> Output` with `Output { stdout, stderr, exit_code }`: matches the verified API.

**5 tasks. ~25 LOC production + ~25 LOC tests. The smallest iteration in the project so far — comparable to v194/v195 single-line fixes but with the breaking-signature-change + new-flag scope creep adding a few more touches.**
