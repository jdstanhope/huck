# shopt Builtin Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement bash's `shopt` builtin over the full 57-name shopt option table plus the 27-name `set -o` bridge, with byte-identical listings and real behavior for five glob/match options (`nullglob`, `dotglob`, `nocaseglob`, `failglob`, `nocasematch`).

**Architecture:** A new `ShoptOptions` state struct on `Shell` (a `[bool; 57]` seeded from a `SHOPT_TABLE` of names+defaults) holds shopt state; a new `builtin_shopt` provides the surface. The existing 3-entry `set -o` registry is expanded to the full 27-name `SETO_TABLE` (3 implemented + 24 inert) and its listing reformatted to bash's `%-15s\t%s`, shared by `set -o`/`set +o` and the `shopt -o` bridge. Behavior is wired by threading a `GlobOpts` into pathname expansion and reading `nocasematch` in the `[[`/`case` matchers. All five behavioral options default off ⇒ zero change to existing behavior.

**Tech Stack:** Rust; the `glob` crate (`MatchOptions`); the `regex` crate; huck's `ExecOutcome` / `Shell` / `Word` / `Field` types.

**Spec:** `docs/superpowers/specs/2026-06-04-shopt-builtin-design.md`

**Conventions:**
- huck is a **binary crate**: unit tests run with `cargo test --bin huck`; integration tests in `tests/` run with `cargo test --test <name>`.
- Every commit ends with the trailer:
  `Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>`
- Baseline before this plan: **2396** tests pass, clippy clean. Each task must keep `cargo clippy --all-targets` clean and the full suite green.

---

## File Structure

| File | Responsibility | Task |
|------|----------------|------|
| `src/builtins.rs` | Expand `set -o` registry to 27 names + reformat listing (T1); new `builtin_shopt` + helpers + registration (T3) | 1, 3 |
| `src/shell_state.rs` | `ShoptInfo` / `SHOPT_TABLE` / `ShoptOptions` / `Shell.shopt_options` / `Shell::glob_opts` / `Shell::nocasematch` | 2 |
| `src/expand.rs` | `GlobOpts` / `GlobExpansion` / `glob_expand_fields_opts` + back-compat `glob_expand_fields` | 4 |
| `src/executor.rs` | `glob_expand_word` helper + wire 4 glob call sites with failglob abort (T4); `nocasematch` in `eval_binary` / `eval_test_expr` / `case_item_matches` (T5) | 4, 5 |
| `tests/shopt_integration.rs` | NEW — binary-driven integration tests | 3, 4, 5 |
| `tests/scripts/shopt_diff_check.sh` | NEW — huck's 13th bash-diff harness | 6 |
| `docs/bash-divergences.md`, `README.md` | M-08d entry, M-08 update, changelog, README row | 6 |

---

### Task 1: Expand `set -o` to the full 27-name table + reformat listings

**Files:**
- Modify: `src/builtins.rs:3876-3922` (`OptionInfo`, `SHELL_OPTIONS`, `option_get`, `option_set`, `print_options_table`, `print_options_reinput`)
- Modify: `src/builtins.rs` ~3953, ~3968, ~3987, ~4017 (the four `option_set(...).is_err()` call sites inside `builtin_set`)
- Test: `src/builtins.rs` (existing `#[cfg(test)]` module — add new tests near the existing `set_o_listing_shows_state`)

This task makes `set -o` / `set +o` list bash's full 27-name table in `%-15s\t%s` format while keeping enable/disable restricted to the 3 implemented options. `shopt -o` (Task 3) reuses these helpers.

- [ ] **Step 1: Write failing unit tests**

Add to the `#[cfg(test)]` module in `src/builtins.rs` (the module that already defines `fn run(...)` for the `set` builtin and `set_o_listing_shows_state`):

```rust
#[test]
fn set_o_lists_full_27_name_table_tab_format() {
    let mut shell = Shell::new();
    let (oc, out) = run(&["-o"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 27, "set -o must list all 27 names; got {lines:?}");
    // bash format: name left-justified in 15, a TAB, then on/off.
    assert_eq!(lines[0], "allexport      \toff");
    assert_eq!(lines[3], "errexit        \toff");
    // long name (>=15 chars): no padding, just name + TAB + value.
    assert!(lines.iter().any(|l| *l == "interactive-comments\ton"));
    assert!(lines.iter().any(|l| *l == "braceexpand    \ton"));
    assert!(lines.iter().any(|l| *l == "hashall        \ton"));
}

#[test]
fn set_o_reflects_real_state_for_implemented() {
    let mut shell = Shell::new();
    shell.shell_options.errexit = true;
    let (_, out) = run(&["-o"], &mut shell);
    assert!(out.lines().any(|l| l == "errexit        \ton"));
}

#[test]
fn set_o_enable_unimplemented_says_not_supported() {
    let mut shell = Shell::new();
    let (oc, _) = run(&["-o", "xtrace"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(2)));
}

#[test]
fn set_o_enable_unknown_name_is_invalid() {
    let mut shell = Shell::new();
    let (oc, _) = run(&["-o", "nope_no_such_opt"], &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(2)));
}
```

- [ ] **Step 2: Run the tests, verify they fail**

Run: `cargo test --bin huck set_o_ 2>&1 | tail -15`
Expected: `set_o_lists_full_27_name_table_tab_format` fails (only 3 lines, wrong format). The others may fail or pass depending on current behavior — all four must pass after Step 3.

- [ ] **Step 3: Implement the registry + helpers**

Replace `OptionInfo` / `SHELL_OPTIONS` / `option_get` / `option_set` / `print_options_table` / `print_options_reinput` (`src/builtins.rs:3876-3922`) with:

```rust
struct OptionInfo {
    name: &'static str,
    default: bool,
}

/// bash 5.2's full `set -o` option table, in bash's display order.
/// `errexit`/`nounset`/`pipefail` are implemented (real state via
/// `Shell.shell_options`); the rest are recognized for listing + querying
/// only (their `default` is reported) and cannot be enabled.
const SETO_TABLE: &[OptionInfo] = &[
    OptionInfo { name: "allexport", default: false },
    OptionInfo { name: "braceexpand", default: true },
    OptionInfo { name: "emacs", default: false },
    OptionInfo { name: "errexit", default: false },
    OptionInfo { name: "errtrace", default: false },
    OptionInfo { name: "functrace", default: false },
    OptionInfo { name: "hashall", default: true },
    OptionInfo { name: "histexpand", default: false },
    OptionInfo { name: "history", default: false },
    OptionInfo { name: "ignoreeof", default: false },
    OptionInfo { name: "interactive-comments", default: true },
    OptionInfo { name: "keyword", default: false },
    OptionInfo { name: "monitor", default: false },
    OptionInfo { name: "noclobber", default: false },
    OptionInfo { name: "noexec", default: false },
    OptionInfo { name: "noglob", default: false },
    OptionInfo { name: "nolog", default: false },
    OptionInfo { name: "notify", default: false },
    OptionInfo { name: "nounset", default: false },
    OptionInfo { name: "onecmd", default: false },
    OptionInfo { name: "physical", default: false },
    OptionInfo { name: "pipefail", default: false },
    OptionInfo { name: "posix", default: false },
    OptionInfo { name: "privileged", default: false },
    OptionInfo { name: "verbose", default: false },
    OptionInfo { name: "vi", default: false },
    OptionInfo { name: "xtrace", default: false },
];

/// Error from `option_set` for a non-settable `set -o` name.
/// `Debug` is required because an existing test calls `option_set(...).unwrap()`.
#[derive(Debug)]
enum OptSetErr {
    /// Known bash option huck does not implement (e.g. `xtrace`, `posix`).
    Unimplemented,
    /// Not a recognized `set -o` option name at all.
    Unknown,
}

/// Reads a `set -o` option: real state for the 3 implemented, the table
/// default for any other recognized name, `None` for an unknown name.
fn option_get(shell: &Shell, name: &str) -> Option<bool> {
    match name {
        "errexit" => Some(shell.shell_options.errexit),
        "nounset" => Some(shell.shell_options.nounset),
        "pipefail" => Some(shell.shell_options.pipefail),
        other => SETO_TABLE.iter().find(|o| o.name == other).map(|o| o.default),
    }
}

/// Writes a `set -o` option. Only the 3 implemented options are settable.
fn option_set(shell: &mut Shell, name: &str, value: bool) -> Result<(), OptSetErr> {
    match name {
        "errexit" => { shell.shell_options.errexit = value; Ok(()) }
        "nounset" => { shell.shell_options.nounset = value; Ok(()) }
        "pipefail" => { shell.shell_options.pipefail = value; Ok(()) }
        other => {
            if SETO_TABLE.iter().any(|o| o.name == other) {
                Err(OptSetErr::Unimplemented)
            } else {
                Err(OptSetErr::Unknown)
            }
        }
    }
}

fn print_options_table(out: &mut dyn Write, shell: &Shell) -> ExecOutcome {
    for opt in SETO_TABLE {
        let val = option_get(shell, opt.name).unwrap_or(opt.default);
        let _ = writeln!(out, "{:<15}\t{}", opt.name, if val { "on" } else { "off" });
    }
    ExecOutcome::Continue(0)
}

fn print_options_reinput(out: &mut dyn Write, shell: &Shell) -> ExecOutcome {
    for opt in SETO_TABLE {
        let val = option_get(shell, opt.name).unwrap_or(opt.default);
        let sign = if val { '-' } else { '+' };
        let _ = writeln!(out, "set {sign}o {}", opt.name);
    }
    ExecOutcome::Continue(0)
}
```

- [ ] **Step 4: Update the four `option_set` call sites in `builtin_set`**

Each of the four sites currently reads (with `out`/`shell` and `args[i]` in scope):

```rust
if option_set(shell, &args[i], true).is_err() {
    eprintln!("huck: set: -o: invalid option name: {}", args[i]);
    return ExecOutcome::Continue(2);
}
```

Replace **each** of the four (the `-o` arm ~3953, the `+o` arm ~3968, the `-`-cluster `b'o'` arm ~3987, the `+`-cluster `b'o'` arm ~4017) with a `match`. For the two **enable** sites (`-o` and `-`-cluster `o`, value `true`):

```rust
match option_set(shell, &args[i], true) {
    Ok(()) => {}
    Err(OptSetErr::Unimplemented) => {
        eprintln!("huck: set: {}: not yet supported in this version", args[i]);
        return ExecOutcome::Continue(2);
    }
    Err(OptSetErr::Unknown) => {
        eprintln!("huck: set: -o: invalid option name: {}", args[i]);
        return ExecOutcome::Continue(2);
    }
}
```

For the two **disable** sites (`+o` and `+`-cluster `o`, value `false`): identical, but with `false` as the third argument and `+o` in the Unknown message.

- [ ] **Step 4b: Migrate the one existing test that referenced the removed `SHELL_OPTIONS`/`o.short`**

`src/builtins.rs:10090` has a test that no longer compiles after the rename + field removal:

```rust
    #[test]
    fn pipefail_listed_in_shell_options() {
        assert!(SHELL_OPTIONS.iter().any(|o| o.name == "pipefail" && o.short.is_none()));
    }
```

Replace its body to reference `SETO_TABLE` and the `default` field (pipefail is present and defaults off):

```rust
    #[test]
    fn pipefail_listed_in_shell_options() {
        assert!(SETO_TABLE.iter().any(|o| o.name == "pipefail" && !o.default));
    }
```

(The nearby `option_set(&mut sh, "pipefail", true).unwrap();` at ~10084 still compiles: `option_set` now returns `Result<(), OptSetErr>` and `OptSetErr` derives `Debug`, so `.unwrap()` on the `Ok(())` path works.)

- [ ] **Step 5: Run tests, verify they pass + nothing regressed**

Run: `cargo test --bin huck set 2>&1 | tail -15`
Expected: the 4 new tests pass; pre-existing `set_o_listing_shows_state`, `set_plus_o_listing_reinput_form`, `set_minus_o_lists_options`, the `nope_no_such_opt` test all still pass (they use loose `.any(...)` / rc-only assertions).
Run: `cargo test 2>&1 | grep -E "^test result" | awk '{p+=$4;f+=$6} END{print "PASS="p" FAIL="f}'`
Expected: `FAIL=0`.
Run: `cargo clippy --all-targets 2>&1 | tail -3` → clean.

- [ ] **Step 6: Commit**

```bash
git add src/builtins.rs
git commit -m "v86 task 1: expand set -o to full 27-name table + tab-format listing

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: `ShoptOptions` data model + `GlobOpts` plumbing

**Files:**
- Modify: `src/shell_state.rs` (add `ShoptInfo`, `SHOPT_TABLE`, `SHOPT_COUNT`, `ShoptOptions`, `Shell.shopt_options` field + init, `Shell::glob_opts`, `Shell::nocasematch`)
- Modify: `src/expand.rs` (add `GlobOpts` struct — used by `glob_opts`)
- Test: `src/shell_state.rs` (`#[cfg(test)]`)

- [ ] **Step 1: Add `GlobOpts` to `src/expand.rs`**

Near the top of `src/expand.rs` (after the existing `use` lines), add:

```rust
/// Pathname-expansion behavior toggles derived from `shopt` state.
/// All-false ⇒ huck's default (pre-v86) globbing behavior.
#[derive(Clone, Copy, Default, Debug)]
pub struct GlobOpts {
    pub nullglob: bool,
    pub dotglob: bool,
    pub nocaseglob: bool,
    pub failglob: bool,
}
```

- [ ] **Step 2: Write failing unit tests in `src/shell_state.rs`**

Add a test module (or extend the existing one) in `src/shell_state.rs`:

```rust
#[cfg(test)]
mod shopt_tests {
    use super::*;

    #[test]
    fn shopt_table_has_57_entries() {
        assert_eq!(SHOPT_TABLE.len(), 57);
    }

    #[test]
    fn shopt_defaults_match_bash() {
        let o = ShoptOptions::default();
        // default-off
        assert_eq!(o.get("nullglob"), Some(false));
        assert_eq!(o.get("dotglob"), Some(false));
        assert_eq!(o.get("extglob"), Some(false));
        // default-on
        assert_eq!(o.get("checkwinsize"), Some(true));
        assert_eq!(o.get("interactive_comments"), Some(true));
        assert_eq!(o.get("sourcepath"), Some(true));
        // exactly 13 default-on
        assert_eq!(SHOPT_TABLE.iter().filter(|e| e.default).count(), 13);
        // unknown
        assert_eq!(o.get("bogus"), None);
    }

    #[test]
    fn shopt_set_and_read_back() {
        let mut o = ShoptOptions::default();
        assert!(o.set("nullglob", true));
        assert_eq!(o.get("nullglob"), Some(true));
        assert!(!o.set("bogus", true)); // unknown → false (not applied)
    }

    #[test]
    fn shell_glob_opts_reflects_shopt() {
        let mut shell = Shell::new();
        shell.shopt_options.set("nullglob", true);
        shell.shopt_options.set("dotglob", true);
        let g = shell.glob_opts();
        assert!(g.nullglob && g.dotglob && !g.nocaseglob && !g.failglob);
        assert!(!shell.nocasematch());
        shell.shopt_options.set("nocasematch", true);
        assert!(shell.nocasematch());
    }
}
```

- [ ] **Step 3: Run tests, verify they fail to compile**

Run: `cargo test --bin huck shopt_table_has_57 2>&1 | tail -10`
Expected: compile error — `SHOPT_TABLE`, `ShoptOptions`, `glob_opts`, `nocasematch` don't exist yet.

- [ ] **Step 4: Implement the data model in `src/shell_state.rs`**

Add (near `ShellOptions`, before the `Shell` struct):

```rust
/// One row of the bash `shopt` option table.
pub struct ShoptInfo {
    pub name: &'static str,
    pub default: bool,
}

/// bash 5.2's complete `shopt` option table, in bash's display order, with
/// non-interactive default values. Bare `shopt` and `shopt -p` emit in this
/// order. Only `nullglob`/`dotglob`/`nocaseglob`/`failglob`/`nocasematch`
/// change huck's behavior; the rest are faithful inert toggles.
pub const SHOPT_TABLE: &[ShoptInfo] = &[
    ShoptInfo { name: "autocd", default: false },
    ShoptInfo { name: "assoc_expand_once", default: false },
    ShoptInfo { name: "cdable_vars", default: false },
    ShoptInfo { name: "cdspell", default: false },
    ShoptInfo { name: "checkhash", default: false },
    ShoptInfo { name: "checkjobs", default: false },
    ShoptInfo { name: "checkwinsize", default: true },
    ShoptInfo { name: "cmdhist", default: true },
    ShoptInfo { name: "compat31", default: false },
    ShoptInfo { name: "compat32", default: false },
    ShoptInfo { name: "compat40", default: false },
    ShoptInfo { name: "compat41", default: false },
    ShoptInfo { name: "compat42", default: false },
    ShoptInfo { name: "compat43", default: false },
    ShoptInfo { name: "compat44", default: false },
    ShoptInfo { name: "complete_fullquote", default: true },
    ShoptInfo { name: "direxpand", default: false },
    ShoptInfo { name: "dirspell", default: false },
    ShoptInfo { name: "dotglob", default: false },
    ShoptInfo { name: "execfail", default: false },
    ShoptInfo { name: "expand_aliases", default: false },
    ShoptInfo { name: "extdebug", default: false },
    ShoptInfo { name: "extglob", default: false },
    ShoptInfo { name: "extquote", default: true },
    ShoptInfo { name: "failglob", default: false },
    ShoptInfo { name: "force_fignore", default: true },
    ShoptInfo { name: "globasciiranges", default: true },
    ShoptInfo { name: "globskipdots", default: true },
    ShoptInfo { name: "globstar", default: false },
    ShoptInfo { name: "gnu_errfmt", default: false },
    ShoptInfo { name: "histappend", default: false },
    ShoptInfo { name: "histreedit", default: false },
    ShoptInfo { name: "histverify", default: false },
    ShoptInfo { name: "hostcomplete", default: true },
    ShoptInfo { name: "huponexit", default: false },
    ShoptInfo { name: "inherit_errexit", default: false },
    ShoptInfo { name: "interactive_comments", default: true },
    ShoptInfo { name: "lastpipe", default: false },
    ShoptInfo { name: "lithist", default: false },
    ShoptInfo { name: "localvar_inherit", default: false },
    ShoptInfo { name: "localvar_unset", default: false },
    ShoptInfo { name: "login_shell", default: false },
    ShoptInfo { name: "mailwarn", default: false },
    ShoptInfo { name: "no_empty_cmd_completion", default: false },
    ShoptInfo { name: "nocaseglob", default: false },
    ShoptInfo { name: "nocasematch", default: false },
    ShoptInfo { name: "noexpand_translation", default: false },
    ShoptInfo { name: "nullglob", default: false },
    ShoptInfo { name: "patsub_replacement", default: true },
    ShoptInfo { name: "progcomp", default: true },
    ShoptInfo { name: "progcomp_alias", default: false },
    ShoptInfo { name: "promptvars", default: true },
    ShoptInfo { name: "restricted_shell", default: false },
    ShoptInfo { name: "shift_verbose", default: false },
    ShoptInfo { name: "sourcepath", default: true },
    ShoptInfo { name: "varredir_close", default: false },
    ShoptInfo { name: "xpg_echo", default: false },
];

/// Number of `shopt` options (length of `SHOPT_TABLE`).
pub const SHOPT_COUNT: usize = SHOPT_TABLE.len();

/// Persistent `shopt` option state: one bool per `SHOPT_TABLE` entry,
/// indexed by table position. Seeded from each option's bash default.
#[derive(Debug, Clone)]
pub struct ShoptOptions {
    state: [bool; SHOPT_COUNT],
}

impl Default for ShoptOptions {
    fn default() -> Self {
        let mut state = [false; SHOPT_COUNT];
        let mut i = 0;
        while i < SHOPT_COUNT {
            state[i] = SHOPT_TABLE[i].default;
            i += 1;
        }
        Self { state }
    }
}

impl ShoptOptions {
    fn idx(name: &str) -> Option<usize> {
        SHOPT_TABLE.iter().position(|o| o.name == name)
    }

    /// `Some(value)` for a known option, `None` for an unknown name.
    pub fn get(&self, name: &str) -> Option<bool> {
        Self::idx(name).map(|i| self.state[i])
    }

    /// Sets a known option; returns `false` (no-op) for an unknown name.
    pub fn set(&mut self, name: &str, value: bool) -> bool {
        match Self::idx(name) {
            Some(i) => { self.state[i] = value; true }
            None => false,
        }
    }
}
```

Add the field to the `Shell` struct (next to `pub shell_options: ShellOptions,`):

```rust
    /// Persistent `shopt` option flags. See `ShoptOptions` / `SHOPT_TABLE`.
    pub shopt_options: ShoptOptions,
```

Add to `Shell`'s constructor (next to `shell_options: ShellOptions::default(),`):

```rust
            shopt_options: ShoptOptions::default(),
```

Add these methods in an `impl Shell` block (place near `dollar_dash_value`):

```rust
    /// Derives pathname-expansion toggles from current `shopt` state.
    pub fn glob_opts(&self) -> crate::expand::GlobOpts {
        crate::expand::GlobOpts {
            nullglob: self.shopt_options.get("nullglob").unwrap_or(false),
            dotglob: self.shopt_options.get("dotglob").unwrap_or(false),
            nocaseglob: self.shopt_options.get("nocaseglob").unwrap_or(false),
            failglob: self.shopt_options.get("failglob").unwrap_or(false),
        }
    }

    /// True when `shopt -s nocasematch` is in effect.
    pub fn nocasematch(&self) -> bool {
        self.shopt_options.get("nocasematch").unwrap_or(false)
    }
```

> Note: if `Shell` has more than one constructor or a `Clone`-by-field path, add `shopt_options` there too. `cargo build` will flag any missing initializer.

- [ ] **Step 5: Run tests, verify they pass**

Run: `cargo test --bin huck shopt_tests 2>&1 | tail -10` → 4 pass.
Run: `cargo build 2>&1 | tail -3` → clean (all `Shell { .. }` initializers satisfied).
Run: `cargo clippy --all-targets 2>&1 | tail -3` → clean.

- [ ] **Step 6: Commit**

```bash
git add src/shell_state.rs src/expand.rs
git commit -m "v86 task 2: ShoptOptions data model + GlobOpts + Shell accessors

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: The `shopt` builtin surface (+ `-o` bridge)

**Files:**
- Modify: `src/builtins.rs:19-31` (`BUILTIN_NAMES`) and `src/builtins.rs:95` area (dispatch `match`)
- Modify: `src/builtins.rs` (add `builtin_shopt` + helpers near `builtin_set`)
- Create: `tests/shopt_integration.rs`
- Test: `src/builtins.rs` (`#[cfg(test)]`)

No behavioral glob/match wiring here — just the builtin surface, listing, query, and the `-o` bridge.

- [ ] **Step 1: Register the builtin**

In `BUILTIN_NAMES` (`src/builtins.rs:19`), add `"shopt"` to the list (e.g. on the `"set", "shift", ...` line):

```rust
    "set", "shopt", "shift", ".", "source", "local",
```

In the builtin dispatch `match` (next to `"set" => builtin_set(args, out, shell),` at `src/builtins.rs:95`), add:

```rust
        "shopt" => builtin_shopt(args, out, shell),
```

- [ ] **Step 2: Write failing integration tests**

Create `tests/shopt_integration.rs`. Model the harness on `tests/pipefail_integration.rs` (spawn the built `huck` binary, feed a script on stdin, capture stdout + exit code):

```rust
//! Integration tests for v86 `shopt` builtin (M-08d).
use std::io::Write;
use std::process::{Command, Stdio};

fn huck_bin() -> &'static str { env!("CARGO_BIN_EXE_huck") }

/// Runs `script` through huck on stdin; returns (stdout, exit_code).
fn run(script: &str) -> (String, i32) {
    let mut child = Command::new(huck_bin())
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn huck");
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (String::from_utf8_lossy(&out.stdout).into_owned(),
     out.status.code().unwrap_or(-1))
}

#[test]
fn shopt_oq_posix_returns_one_silently() {
    // The stock-bashrc case: `if ! shopt -oq posix`.
    let (out, rc) = run("shopt -oq posix; echo rc=$?\n");
    assert_eq!(out, "rc=1\n");
    assert_eq!(rc, 0);
}

#[test]
fn shopt_set_query_roundtrip() {
    assert_eq!(run("shopt -q nullglob; echo $?\n").0, "1\n");
    assert_eq!(run("shopt -s nullglob; shopt -q nullglob; echo $?\n").0, "0\n");
}

#[test]
fn shopt_inert_option_tracks() {
    // extglob is inert in huck but must round-trip.
    assert_eq!(run("shopt -s extglob; shopt -q extglob; echo $?\n").0, "0\n");
}

#[test]
fn shopt_invalid_name_rc_one() {
    let (_, rc) = run("shopt -s definitely_not_an_option\n");
    assert_eq!(rc, 1);
}

#[test]
fn shopt_query_prints_state() {
    assert_eq!(run("shopt -s dotglob; shopt dotglob\n").0, "dotglob        \ton\n");
}

#[test]
fn shopt_multi_query_rc_is_all_set() {
    // one on, one off → rc 1; both printed in table order.
    let (out, _) = run("shopt -s dotglob; shopt dotglob nullglob; echo rc=$?\n");
    assert_eq!(out, "dotglob        \ton\nnullglob       \toff\nrc=1\n");
}
```

- [ ] **Step 3: Run tests, verify they fail**

Run: `cargo test --test shopt_integration 2>&1 | tail -15`
Expected: failures — `shopt` is registered but `builtin_shopt` is unimplemented (won't compile until Step 4), or "command not found". (If it doesn't compile because `builtin_shopt` is missing, that counts as the failing state; proceed to Step 4.)

- [ ] **Step 4: Implement `builtin_shopt` + helpers**

Add near `builtin_set` in `src/builtins.rs`. This uses `SHOPT_TABLE` and `SETO_TABLE`; import `SHOPT_TABLE` at the top of `builtins.rs` (`use crate::shell_state::{... , SHOPT_TABLE};` — match the existing `use` style).

```rust
/// Formats one option line in bash's `%-15s\t%s` shopt/`set -o` format.
fn fmt_opt_line(name: &str, on: bool) -> String {
    format!("{:<15}\t{}", name, if on { "on" } else { "off" })
}

/// `shopt` builtin. Operates on the `shopt` option namespace, or — with
/// `-o` — bridges to the `set -o` namespace (`SETO_TABLE`).
fn builtin_shopt(args: &[String], out: &mut dyn Write, shell: &mut Shell) -> ExecOutcome {
    let (mut set_f, mut unset_f, mut quiet, mut print_f, mut o_bridge) =
        (false, false, false, false, false);
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a == "--" { i += 1; break; }
        if a.len() >= 2 && a.starts_with('-') {
            for c in a[1..].chars() {
                match c {
                    's' => set_f = true,
                    'u' => unset_f = true,
                    'q' => quiet = true,
                    'p' => print_f = true,
                    'o' => o_bridge = true,
                    _ => {
                        eprintln!("huck: shopt: -{c}: invalid option");
                        eprintln!("shopt: usage: shopt [-pqsu] [-o] [optname ...]");
                        return ExecOutcome::Continue(2);
                    }
                }
            }
            i += 1;
        } else {
            break;
        }
    }
    if set_f && unset_f {
        eprintln!("huck: shopt: cannot set and unset shell options simultaneously");
        return ExecOutcome::Continue(1);
    }
    let names = &args[i..];

    if o_bridge {
        return shopt_o_bridge(names, set_f, unset_f, quiet, print_f, out, shell);
    }

    // ---- shopt namespace ----
    if names.is_empty() {
        if quiet {
            // "are all options set?" with no names → never true.
            return ExecOutcome::Continue(1);
        }
        for opt in SHOPT_TABLE {
            let on = shell.shopt_options.get(opt.name).unwrap_or(false);
            if set_f && !on { continue; }
            if unset_f && on { continue; }
            if print_f {
                let _ = writeln!(out, "shopt -{} {}", if on { 's' } else { 'u' }, opt.name);
            } else {
                let _ = writeln!(out, "{}", fmt_opt_line(opt.name, on));
            }
        }
        return ExecOutcome::Continue(0);
    }

    if set_f || unset_f {
        let mut rc = 0;
        for name in names {
            if !shell.shopt_options.set(name, set_f) {
                eprintln!("huck: shopt: {name}: invalid shell option name");
                rc = 1;
            }
        }
        return ExecOutcome::Continue(rc);
    }

    // query mode
    let mut all_set = true;
    for name in names {
        match shell.shopt_options.get(name) {
            Some(on) => {
                if !on { all_set = false; }
                if !quiet {
                    let _ = writeln!(out, "{}", fmt_opt_line(name, on));
                }
            }
            None => {
                eprintln!("huck: shopt: {name}: invalid shell option name");
                all_set = false;
            }
        }
    }
    ExecOutcome::Continue(if all_set { 0 } else { 1 })
}

/// The `-o` bridge: every `shopt` form operates on the `set -o` namespace.
fn shopt_o_bridge(
    names: &[String], set_f: bool, unset_f: bool, quiet: bool, print_f: bool,
    out: &mut dyn Write, shell: &mut Shell,
) -> ExecOutcome {
    if names.is_empty() {
        if quiet { return ExecOutcome::Continue(1); }
        for opt in SETO_TABLE {
            let on = option_get(shell, opt.name).unwrap_or(opt.default);
            if set_f && !on { continue; }
            if unset_f && on { continue; }
            if print_f {
                let _ = writeln!(out, "set {}o {}", if on { '-' } else { '+' }, opt.name);
            } else {
                let _ = writeln!(out, "{}", fmt_opt_line(opt.name, on));
            }
        }
        return ExecOutcome::Continue(0);
    }

    if set_f || unset_f {
        let mut rc = 0;
        for name in names {
            match option_set(shell, name, set_f) {
                Ok(()) => {}
                Err(OptSetErr::Unimplemented) => {
                    eprintln!("huck: shopt: {name}: not yet supported in this version");
                    rc = 1;
                }
                Err(OptSetErr::Unknown) => {
                    eprintln!("huck: shopt: {name}: invalid shell option name");
                    rc = 1;
                }
            }
        }
        return ExecOutcome::Continue(rc);
    }

    // query mode
    let mut all_set = true;
    for name in names {
        match option_get(shell, name) {
            Some(on) => {
                if !on { all_set = false; }
                if !quiet {
                    let _ = writeln!(out, "{}", fmt_opt_line(name, on));
                }
            }
            None => {
                eprintln!("huck: shopt: {name}: invalid shell option name");
                all_set = false;
            }
        }
    }
    ExecOutcome::Continue(if all_set { 0 } else { 1 })
}
```

- [ ] **Step 5: Run tests, verify they pass**

Run: `cargo test --test shopt_integration 2>&1 | tail -15` → all pass.
Add a couple of unit tests in the `src/builtins.rs` test module (use the existing `run` helper convention if it dispatches by builtin name, or call `builtin_shopt` directly with a fresh `Shell::new()` and a `Vec<u8>` writer):

```rust
#[test]
fn shopt_bare_lists_all_57() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let oc = builtin_shopt(&[], &mut buf, &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    let out = String::from_utf8(buf).unwrap();
    assert_eq!(out.lines().count(), 57);
    assert_eq!(out.lines().next().unwrap(), "autocd         \toff");
    assert!(out.lines().any(|l| l == "checkwinsize   \ton"));
    assert!(out.lines().any(|l| l == "assoc_expand_once\toff")); // long name, no pad
}

#[test]
fn shopt_o_lists_27() {
    let mut shell = Shell::new();
    let mut buf: Vec<u8> = Vec::new();
    let oc = builtin_shopt(&["-o".into()], &mut buf, &mut shell);
    assert!(matches!(oc, ExecOutcome::Continue(0)));
    assert_eq!(String::from_utf8(buf).unwrap().lines().count(), 27);
}
```

Run: `cargo test --bin huck shopt_bare 2>&1 | tail -5` and `cargo test --bin huck shopt_o_lists 2>&1 | tail -5` → pass.
Run full suite + clippy:
`cargo test 2>&1 | grep -E "^test result" | awk '{p+=$4;f+=$6} END{print "PASS="p" FAIL="f}'` → `FAIL=0`.
`cargo clippy --all-targets 2>&1 | tail -3` → clean.

- [ ] **Step 6: Commit**

```bash
git add src/builtins.rs tests/shopt_integration.rs
git commit -m "v86 task 3: shopt builtin surface + -o set-o bridge

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Behavioral wiring — `nullglob` / `dotglob` / `nocaseglob` / `failglob`

**Files:**
- Modify: `src/expand.rs:1012-1066` (`glob_expand_fields` → add `glob_expand_fields_opts`, keep wrapper)
- Modify: `src/executor.rs` (new `glob_expand_word` helper; wire call sites at 337, 605, 1594, 1636)
- Test: `tests/shopt_integration.rs` (append)

- [ ] **Step 1: Write failing integration tests (append to `tests/shopt_integration.rs`)**

These use a temp dir with known files. Add a fixture helper + tests:

```rust
use std::fs;

/// Runs `script` with cwd set to a fresh temp dir containing the given files.
/// Returns (stdout, exit_code).
fn run_in_dir(files: &[&str], script: &str) -> (String, i32) {
    let dir = std::env::temp_dir().join(format!("huck_shopt_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    for f in files { fs::write(dir.join(f), b"").unwrap(); }
    let mut child = Command::new(huck_bin())
        .current_dir(&dir)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().expect("spawn huck");
    child.stdin.take().unwrap().write_all(script.as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    let _ = fs::remove_dir_all(&dir);
    (String::from_utf8_lossy(&out.stdout).into_owned(),
     out.status.code().unwrap_or(-1))
}

#[test]
fn nullglob_no_match_expands_empty() {
    // default: literal pattern survives.
    assert_eq!(run_in_dir(&["a.txt"], "echo no*match\n").0, "no*match\n");
    // nullglob: no match → no word; echo prints just a newline.
    assert_eq!(run_in_dir(&["a.txt"], "shopt -s nullglob; echo no*match\n").0, "\n");
}

#[test]
fn dotglob_includes_dotfiles() {
    // default: `*` skips .hidden.
    assert_eq!(run_in_dir(&["a", ".hidden"], "echo *\n").0, "a\n");
    // dotglob: `*` includes .hidden (sorted).
    assert_eq!(run_in_dir(&["a", ".hidden"], "shopt -s dotglob; echo *\n").0, ".hidden a\n");
}

#[test]
fn nocaseglob_matches_case_insensitively() {
    assert_eq!(run_in_dir(&["Abc.txt"], "echo a*\n").0, "a*\n"); // default: no match → literal
    assert_eq!(run_in_dir(&["Abc.txt"], "shopt -s nocaseglob; echo a*\n").0, "Abc.txt\n");
}

#[test]
fn failglob_no_match_aborts_command() {
    // `echo no*match` aborts (status 1, no stdout); the shell continues, so
    // `echo after` still runs. (The process exit code is `echo after`'s 0,
    // so assert the abort via stdout + an explicit `$?` capture.)
    let (out, _) = run_in_dir(&["a.txt"], "shopt -s failglob; echo no*match\necho after\n");
    assert_eq!(out, "after\n");
    let (out2, _) = run_in_dir(&["a.txt"], "shopt -s failglob; echo no*match; echo rc=$?\n");
    assert_eq!(out2, "rc=1\n");
}
```

- [ ] **Step 2: Run tests, verify they fail**

Run: `cargo test --test shopt_integration nullglob 2>&1 | tail -10`
Expected: nullglob/dotglob/nocaseglob/failglob tests fail (behavior not wired — globs still use defaults).

- [ ] **Step 3: Add `glob_expand_fields_opts` in `src/expand.rs`**

Replace `glob_expand_fields` (`src/expand.rs:1012-1066`) with the opts-aware version + a back-compat wrapper:

```rust
/// Result of opts-aware pathname expansion. `words` are the expanded fields;
/// `failglob_unmatched` lists patterns that matched nothing under `failglob`
/// (the caller turns a non-empty list into a command abort with status 1).
pub struct GlobExpansion {
    pub words: Vec<String>,
    pub failglob_unmatched: Vec<String>,
}

/// Pathname expansion honoring `shopt` glob toggles. See `GlobOpts`.
pub fn glob_expand_fields_opts(fields: Vec<Field>, opts: GlobOpts) -> GlobExpansion {
    let mut words = Vec::new();
    let mut failglob_unmatched = Vec::new();
    for field in fields {
        if !has_unquoted_metachar(&field) {
            words.push(field.chars);
            continue;
        }
        let pattern = build_glob_pattern(&field);
        let literal_leading_dot =
            pattern.starts_with('.') || pattern.starts_with("[.]");
        let match_opts = MatchOptions {
            case_sensitive: !opts.nocaseglob,
            require_literal_separator: true,
            // dotglob forces `*`/`?` to match a leading dot; otherwise keep
            // the existing rule (literal-dot patterns match dotfiles, bare
            // metachar patterns do not).
            require_literal_leading_dot: !literal_leading_dot && !opts.dotglob,
        };
        match glob_with(&pattern, match_opts) {
            Ok(paths) => {
                let mut matched = Vec::new();
                for entry in paths {
                    let Ok(path) = entry else { continue };
                    match path.into_os_string().into_string() {
                        Ok(s) => matched.push(s),
                        Err(_) => eprintln!("huck: skipping non-UTF8 path"),
                    }
                }
                matched.retain(|p| {
                    let last = std::path::Path::new(p)
                        .file_name().and_then(|s| s.to_str());
                    !matches!(last, Some(".") | Some(".."))
                });
                if matched.is_empty() {
                    if opts.failglob {
                        failglob_unmatched.push(field.chars);
                    } else if opts.nullglob {
                        // contribute nothing
                    } else {
                        words.push(field.chars);
                    }
                } else {
                    words.extend(matched);
                }
            }
            Err(_) => words.push(field.chars), // invalid pattern → literal
        }
    }
    GlobExpansion { words, failglob_unmatched }
}

/// Back-compat: default (all-off) globbing. Used by existing call sites and
/// tests that don't need `shopt` behavior.
pub fn glob_expand_fields(fields: Vec<Field>) -> Vec<String> {
    glob_expand_fields_opts(fields, GlobOpts::default()).words
}
```

- [ ] **Step 4: Add the `glob_expand_word` helper + wire call sites in `src/executor.rs`**

Update the import at `src/executor.rs:12` to include the new function:

```rust
use crate::expand::{expand, expand_assignment, expand_pattern, glob_expand_fields, glob_expand_fields_opts};
```

Add this helper (place near `resolve`, before its first use):

```rust
/// Glob-expands one word honoring `shopt` flags. On a `failglob` no-match,
/// prints the bash-style "no match" error to stderr and returns `Err(())`,
/// signaling the caller to abort the command/loop with status 1.
fn glob_expand_word(word: &crate::lexer::Word, shell: &mut Shell) -> Result<Vec<String>, ()> {
    let opts = shell.glob_opts();
    let fields = expand(word, shell);
    let exp = glob_expand_fields_opts(fields, opts);
    if !exp.failglob_unmatched.is_empty() {
        eprintln!("huck: no match: {}", exp.failglob_unmatched.join(" "));
        return Err(());
    }
    Ok(exp.words)
}
```

Wire the four production call sites:

1. **For-loop word list (`src/executor.rs:336-338`):**
```rust
        for word in &clause.words {
            match glob_expand_word(word, shell) {
                Ok(v) => values.extend(v),
                Err(()) => return ExecOutcome::Continue(1),
            }
        }
```

2. **Select/other word list (`src/executor.rs:604-606`):**
```rust
            for w in words {
                match glob_expand_word(w, shell) {
                    Ok(v) => v_local.extend(v),
                    Err(()) => return ExecOutcome::Continue(1),
                }
            }
```
> Adjust the accumulator name to the one already in scope at that site (it is `v` in the current code: `let mut v = Vec::new(); ... v.extend(...)`). Keep using that existing `v`; the example renames it only to avoid shadowing confusion — match the real local.

3. **Program word in `resolve` (`src/executor.rs:1594`):** replace
```rust
    let prog_fields = glob_expand_fields(expand(&cmd.program, shell));
```
with
```rust
    let prog_fields = match glob_expand_word(&cmd.program, shell) {
        Ok(v) => v,
        Err(()) => return Err(1),
    };
```

4. **Argument words in `resolve` (`src/executor.rs:1636`):** replace
```rust
        let fields = glob_expand_fields(expand(word, shell));
```
with
```rust
        let fields = match glob_expand_word(word, shell) {
            Ok(v) => v,
            Err(()) => return Err(1),
        };
```
(Leave the immediately-following `if let Some(status) = shell.pending_fatal_pe_error { return Err(status); }` checks in place at both `resolve` sites.)

> `glob_expand_fields` is still imported and used by `expand.rs`'s own tests and any remaining default call site; keep the import. If `cargo` warns it is now unused in `executor.rs`, remove only the `glob_expand_fields` name from the `executor.rs` `use` (keep `glob_expand_fields_opts`).

- [ ] **Step 5: Run tests, verify they pass**

Run: `cargo test --test shopt_integration 2>&1 | tail -15` → all pass.
Run: `cargo test --bin huck glob_expand 2>&1 | tail -10` → existing `glob_expand_fields` tests still pass (wrapper preserves behavior).
Run full suite: `cargo test 2>&1 | grep -E "^test result" | awk '{p+=$4;f+=$6} END{print "PASS="p" FAIL="f}'` → `FAIL=0`.
Run: `cargo clippy --all-targets 2>&1 | tail -3` → clean.

- [ ] **Step 6: Commit**

```bash
git add src/expand.rs src/executor.rs tests/shopt_integration.rs
git commit -m "v86 task 4: wire nullglob/dotglob/nocaseglob/failglob into pathname expansion

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Behavioral wiring — `nocasematch` (`[[ ]]` + `case`)

**Files:**
- Modify: `src/executor.rs` — `eval_binary` (~889), `eval_test_expr` `Regex` arm (~848), `case_item_matches` (~710)
- Test: `tests/shopt_integration.rs` (append)

- [ ] **Step 1: Write failing integration tests (append)**

```rust
#[test]
fn nocasematch_double_bracket_eq() {
    assert_eq!(run("[[ ABC == abc ]] && echo m || echo n\n").0, "n\n");
    assert_eq!(run("shopt -s nocasematch; [[ ABC == abc ]] && echo m || echo n\n").0, "m\n");
}

#[test]
fn nocasematch_double_bracket_regex() {
    assert_eq!(run("[[ ABC =~ ^abc$ ]] && echo m || echo n\n").0, "n\n");
    assert_eq!(run("shopt -s nocasematch; [[ ABC =~ ^abc$ ]] && echo m || echo n\n").0, "m\n");
}

#[test]
fn nocasematch_case_statement() {
    assert_eq!(run("case ABC in abc) echo m;; *) echo n;; esac\n").0, "n\n");
    assert_eq!(run("shopt -s nocasematch; case ABC in abc) echo m;; *) echo n;; esac\n").0, "m\n");
}
```

- [ ] **Step 2: Run tests, verify they fail**

Run: `cargo test --test shopt_integration nocasematch 2>&1 | tail -10`
Expected: the three `nocasematch` tests fail on the `shopt -s nocasematch` line (matching still case-sensitive).

- [ ] **Step 3: Wire `eval_binary` (`[[ == ]]` / `[[ != ]]`)**

In `eval_binary` (`src/executor.rs:889`), the `StringEq | StringNe` arm currently is:

```rust
        TestBinaryOp::StringEq | TestBinaryOp::StringNe => {
            let pattern_str = expand_pattern(rhs_word, shell);
            let pat = glob::Pattern::new(&pattern_str)
                .map_err(|e| format!("bad pattern: {e}"))?;
            let matched = pat.matches(lhs);
            Ok(if matches!(op, TestBinaryOp::StringEq) { matched } else { !matched })
        }
```

Replace the `let matched = pat.matches(lhs);` line with:

```rust
            let mopts = glob::MatchOptions {
                case_sensitive: !shell.nocasematch(),
                require_literal_separator: false,
                require_literal_leading_dot: false,
            };
            let matched = pat.matches_with(lhs, mopts);
```

- [ ] **Step 4: Wire `eval_test_expr` (`[[ =~ ]]`)**

In the `TestExpr::Regex { lhs, pattern }` arm (`src/executor.rs:848`), the expanded pattern is bound to `p` and then `regex::Regex::new(&p)` is called. Insert a case-fold prefix before constructing the regex:

```rust
        TestExpr::Regex { lhs, pattern } => {
            // ... existing code that produces `let p = <expanded pattern>;` ...
            let p = if shell.nocasematch() { format!("(?i){p}") } else { p };
            let re = regex::Regex::new(&p).map_err(|e| format!("regex error: {e}"))?;
            // ... unchanged ...
        }
```
> Read the existing arm first: bind the `(?i)` rewrite to the variable actually passed to `Regex::new` (it is `p` in the current code). Keep everything else (the `lhs` match, `BASH_REMATCH` handling if any) unchanged.

- [ ] **Step 5: Wire `case_item_matches`**

In `case_item_matches` (`src/executor.rs:710`), change the `MatchOptions` literal's first field:

```rust
    let opts = glob::MatchOptions {
        case_sensitive: !shell.nocasematch(),
        require_literal_separator: false,
        require_literal_leading_dot: false,
    };
```

- [ ] **Step 6: Run tests, verify they pass**

Run: `cargo test --test shopt_integration 2>&1 | tail -15` → all pass (incl. the three new ones).
Run full suite: `cargo test 2>&1 | grep -E "^test result" | awk '{p+=$4;f+=$6} END{print "PASS="p" FAIL="f}'` → `FAIL=0` (existing `[[`/`case` tests unaffected — default off).
Run: `cargo clippy --all-targets 2>&1 | tail -3` → clean.

- [ ] **Step 7: Commit**

```bash
git add src/executor.rs tests/shopt_integration.rs
git commit -m "v86 task 5: wire nocasematch into [[ == / =~ ]] and case matching

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

### Task 6: bash-diff harness + docs

**Files:**
- Create: `tests/scripts/shopt_diff_check.sh` (huck's 13th harness)
- Modify: `docs/bash-divergences.md`, `README.md`

- [ ] **Step 1: Write the harness**

Create `tests/scripts/shopt_diff_check.sh`, modeled on `tests/scripts/param_operand_diff_check.sh` (same `set -u`, `HUCK_BIN` resolution, `check()` comparing bash vs huck with `EXIT:$?` appended, totals, `exit $((FAIL>0?1:0))`). Add a fixture temp dir for the glob cases. `chmod +x` it.

```bash
#!/usr/bin/env bash
# Byte-identical bash<->huck harness for v86: the shopt builtin (M-08d).
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
# A fixture dir for the glob fragments (a.txt, .hidden, Abc.txt).
FIX="$(mktemp -d)"; trap 'rm -rf "$FIX"' EXIT
: > "$FIX/a.txt"; : > "$FIX/.hidden"; : > "$FIX/Abc.txt"

# NOTE: `failglob`'s no-match error text is intentionally NOT diff-checked here:
# bash prints `bash: line N: no match: PAT` while huck uses its `huck: no match:`
# prefix, so the stderr line cannot be byte-identical. failglob's rc + empty
# stdout are covered by tests/shopt_integration.rs (failglob_no_match_aborts_command).

check "bare shopt lists all"   'shopt'
check "shopt -p reinput"       'shopt -p'
check "shopt -s lists on"      'shopt -s'
check "shopt -u lists off"     'shopt -u'
check "shopt -o lists set-o"   'shopt -o'
check "shopt -po reinput"      'shopt -po'
check "set -o table"           'set -o'
check "set +o reinput"         'set +o'
check "shopt -oq posix"        'shopt -oq posix; echo rc=$?'
check "shopt query multi rc"   'shopt -s dotglob; shopt dotglob nullglob; echo rc=$?'
check "shopt -q set then query" 'shopt -s nullglob; shopt -q nullglob; echo $?'
check "shopt invalid name"     'shopt -s totally_bogus_option; echo rc=$?'
check "shopt set unset excl"   'shopt -s -u nullglob; echo rc=$?'
check "nullglob empty"         "cd '$FIX'; shopt -s nullglob; echo no*match"
check "dotglob includes dot"   "cd '$FIX'; shopt -s dotglob; echo *"
check "dotglob off default"    "cd '$FIX'; echo *"
check "nocaseglob match"       "cd '$FIX'; shopt -s nocaseglob; echo a*"
check "nocasematch [[ eq"      'shopt -s nocasematch; [[ ABC == abc ]] && echo m || echo n'
check "nocasematch =~"         'shopt -s nocasematch; [[ ABC =~ ^abc$ ]] && echo m || echo n'
check "nocasematch case"       'shopt -s nocasematch; case ABC in abc) echo m;; *) echo n;; esac'
echo ""; echo "Total: $((PASS+FAIL)), Pass: $PASS, Fail: $FAIL"
exit $(( FAIL > 0 ? 1 : 0 ))
```

> The invalid-name and set/unset-exclusive fragments append `echo rc=$?`, so the stderr error line is followed by a stdout line both shells produce; the error TEXT differs in prefix (`bash: line N: shopt:` vs `huck: shopt:`), which WOULD break byte-equality. If `shopt invalid name` / `shopt set unset excl` FAIL purely on that prefix line, move them out of the harness and into integration tests (assert rc only), exactly like failglob — do NOT weaken the comparison. Run the harness and decide per Step 2.

- [ ] **Step 2: Run the harness, confirm all PASS**

```bash
cd /home/john/projects/shuck
cargo build 2>&1 | tail -1
chmod +x tests/scripts/shopt_diff_check.sh
bash tests/scripts/shopt_diff_check.sh; echo "rc=$?"
```
Expected: `Fail: 0`, `rc=0`. If `shopt invalid name` or `shopt set unset excl` fail on the `huck:` vs `bash: line N:` prefix, remove those two fragments from the harness (add a NOTE that they are integration-tested on rc) and add to `tests/shopt_integration.rs`:
```rust
#[test]
fn shopt_set_and_unset_together_rc_one() {
    assert_eq!(run("shopt -s -u nullglob; echo rc=$?\n").0, "rc=1\n");
}
```
Re-run the harness until `Fail: 0`. Report which fragments (if any) you relocated and why.

- [ ] **Step 3: Update `docs/bash-divergences.md`**

Read the current M-08 entry (`src/`… line ~134) and the Tier-2 count line (~25) first. Make these edits:

1. Add a new sub-entry immediately after M-08 (before M-08c), mirroring the style of M-08c:
```markdown
- **M-08d: `shopt` builtin** — `[fixed v86]` medium. Full `shopt` surface over
  bash 5.2's 57-name shopt table (byte-identical `%-15s\t%s` listing, table
  order, non-interactive defaults): `-s`/`-u` enable/disable, bare/`-p`/`-s`/`-u`
  listing, `NAME...` and `-q` query (rc 0 iff all set), invalid name →
  `invalid shell option name` rc 1 with valid names still applied. `-o` bridges
  to the `set -o` namespace over bash's 27-name table (so `shopt -oq posix` → 1
  silently); enabling an unimplemented set-o option still errors "not yet
  supported", keeping `$-` honest (v69 policy). Behavioral wiring: `nullglob`,
  `dotglob`, `nocaseglob`, `failglob` (pathname expansion) and `nocasematch`
  (`[[ == / != / =~ ]]` + `case`); all other options are faithful inert toggles.
  `set -o`/`set +o` listing reformatted to bash's 27-name `%-15s\t%s` table.
  Closes the last builtin gap for loading a stock Debian `~/.bashrc`
  (`shopt -s checkwinsize`, `shopt -s histappend`, `if ! shopt -oq posix`).
  huck's 13th bash-diff harness. **Minor divergence**: `failglob`'s no-match
  error text uses huck's `huck: no match:` prefix vs bash's `bash: line N:`
  (rc + empty stdout match; covered by integration tests).
```
   (Use `M-08d` — the next sequential letter after `M-08c`, which was assigned in v85; `b` is skipped to keep sub-letters monotonic by creation order.)

2. In the M-08 entry, change the "Still deferred" list to remove `shopt` from the picture — M-08 was about `set` flags, but its prose may reference shopt; if it does, append "(`shopt` shipped in v86, see M-08d)". If it does not mention shopt, no change beyond adding M-08d is required.

3. Update the Tier-2 summary count line (~25): bump the count by 1 and append `; M-08d fixed by v86, added as follow-on to M-08`.

4. Update the "Last updated" stamp (line 3) to `2026-06-04 (after v86 shopt builtin; M-08d fixed)`.

5. Add a changelog entry at the end of the file (match the v85 entry's format), dated `2026-06-04`, summarizing the implementation (the `ShoptOptions`/`SHOPT_TABLE` model, `builtin_shopt`, the `set -o` 27-name reformat, the five behavioral options, the 13th harness, the failglob stderr caveat).

- [ ] **Step 4: Update `README.md`**

Read the iteration table and the v85 row first. Add a v86 row in the same column format:
```markdown
| v86 | `shopt` builtin (M-08d) | Full 57-name shopt table + 27-name `set -o` bridge, byte-identical listings; behavioral `nullglob`/`dotglob`/`nocaseglob`/`failglob`/`nocasematch`; closes the last builtin gap for loading a real `~/.bashrc` |
```
(Match the exact columns/escaping of the existing rows — escape any literal `|`.)

- [ ] **Step 5: Verify the whole branch**

```bash
cargo test 2>&1 | grep -E "^test result" | awk '{p+=$4;f+=$6} END{print "PASS="p" FAIL="f}'   # FAIL=0
cargo clippy --all-targets 2>&1 | tail -3                                                       # clean
for f in tests/scripts/*_diff_check.sh; do printf '%s: ' "$f"; bash "$f" >/dev/null 2>&1 && echo OK || echo FAIL; done  # all OK (13 harnesses)
```

- [ ] **Step 6: Commit**

```bash
git add tests/scripts/shopt_diff_check.sh docs/bash-divergences.md README.md tests/shopt_integration.rs
git commit -m "v86 task 6: shopt bash-diff harness + docs (M-08d)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Notes for the implementer

- **Binary crate:** unit tests are `cargo test --bin huck <filter>`; integration tests are `cargo test --test shopt_integration`. The full suite is plain `cargo test`.
- **No behavior change when shopt is untouched:** all five behavioral flags default off, so every pre-existing glob/`case`/`[[` test must stay green. If one breaks, you've changed default behavior — revisit the `!opts.x` / `!shell.nocasematch()` polarity.
- **Borrow note** in `glob_expand_word`: call `shell.glob_opts()` (returns an owned `Copy` `GlobOpts`) into a `let` binding *before* `expand(word, shell)`, so the immutable borrow ends before the mutable one — as written in Task 4 Step 4.
- **M-number:** the divergence entry is **M-08d** — the next sequential letter after the existing M-08c (v85). `b` is unused, but we skip it so the sub-letters stay monotonic by creation order (matches the spec). Use **M-08d** consistently in the divergences doc, README, and commit messages.
- **Harness fidelity:** never weaken `check()` to force a pass. If a fragment can't be byte-identical (error-text prefix), relocate it to an integration test and document the exclusion in a NOTE comment, as the spec directs for `failglob`.
